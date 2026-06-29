//! The thin orchestrator: the LLM proposes, the pipeline enforces.
//!
//! No hypothesis tracking, no finding counts, no convergence gates, no journal.
//! Each iteration rebuilds the mandate from the audit chain, asks for one
//! proposal, and either runs it through the [`Pipeline`] or concludes. A gate
//! rejection ends the run (there is no LLM retry loop) — "reject and report".

mod client;
mod mandate;
mod proposal;

pub use client::{HttpLlmClient, LlmClient, LlmError, ScriptedClient};
pub use mandate::build_mandate;
pub use proposal::{ParseError, Proposal, infer_risk};

use axiom_gate::{
    GateContext, Pipeline, analyze_series, record_calibration, record_conclusion,
    require_operator_approval,
};
use axiom_types::Rejection;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("llm: {0}")]
    Llm(#[from] LlmError),
    #[error("gate rejection: {0}")]
    Rejected(#[from] Rejection),
    #[error("analyze_series failed: {0}")]
    Analyze(String),
    #[error("reached iteration limit ({0}) without a conclusion")]
    MaxIterations(u32),
}

/// Drives an [`LlmClient`] against the gate [`Pipeline`].
pub struct Orchestrator {
    llm: Arc<dyn LlmClient>,
    pipeline: Arc<Pipeline>,
}

impl Orchestrator {
    pub fn new(llm: Arc<dyn LlmClient>, pipeline: Arc<Pipeline>) -> Self {
        Self { llm, pipeline }
    }

    /// Run until the LLM concludes (returns its summary), a gate rejects an
    /// action, or the iteration limit is reached.
    pub async fn run(&self, directive: &str, ctx: &GateContext) -> Result<String, OrchestratorError> {
        let max = max_iterations();
        for _ in 0..max {
            let mandate = build_mandate(directive, ctx);
            match self.llm.propose(&mandate).await? {
                Proposal::Protocol(steps) => {
                    for step in steps {
                        // First Err hard-stops the whole run.
                        self.pipeline.run(step, ctx).await?;
                    }
                }
                Proposal::Analyze(req) => {
                    let standards = ctx.lab_state.lock().unwrap().registered_reference_materials();
                    let outcome =
                        analyze_series(&req, &standards).map_err(OrchestratorError::Analyze)?;

                    // A calibration unlocks the measurement tools, so it requires
                    // operator sign-off before it is recorded.
                    if let Some(cal) = outcome.proposed_calibration {
                        let params = serde_json::json!({
                            "instrument": cal.instrument,
                            "standards": cal.standard_ids,
                            "n_levels": cal.n_levels,
                            "r_squared": cal.r_squared,
                            "model": cal.model,
                        });
                        match require_operator_approval(ctx, "record_calibration", &params).await {
                            Ok(approver) => {
                                record_calibration(&ctx.audit_chain, ctx.signer.as_ref(), &cal, &approver)
                                    .map_err(|e| OrchestratorError::Analyze(e.to_string()))?;
                            }
                            // Declining to calibrate is not a hard failure — the
                            // measurement tools simply stay locked.
                            Err(reason) => tracing::warn!(%reason, "calibration not approved — not recorded"),
                        }
                    }
                }
                Proposal::Done { summary } => {
                    // Rekor anchoring failures are non-fatal (logged inside).
                    let _ = record_conclusion(ctx, &summary).await;
                    return Ok(summary);
                }
            }
        }
        Err(OrchestratorError::MaxIterations(max))
    }
}

fn max_iterations() -> u32 {
    std::env::var("AXIOMLAB_MAX_ITERATIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(50)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_audit::{Chain, LocalSigner, RevocationList, Signer};
    use axiom_gate::{ApprovalQueue, CapabilityPolicy};
    use axiom_proofs::{
        ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofChecker, ProofManifest,
        VerusArtifact,
    };
    use axiom_sila::SilaClients;
    use axiom_types::{LabState, RiskClass};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    fn manifest() -> ProofManifest {
        ProofManifest {
            schema_version: 1,
            generated_unix_secs: 0,
            build: BuildIdentity {
                git_commit: "g".into(),
                binary_hash: "b".into(),
                workspace_hash: "w".into(),
                container_image_digest: None,
                device_id: None,
                firmware_version: None,
            },
            artifacts: vec![ProofArtifact {
                id: "lab_safety_verus".into(),
                source_path: "verus_verified/lab_safety.rs".into(),
                source_hash: "h".into(),
                mir_path: None,
                mir_hash: None,
                lean: vec![],
                verus: Some(VerusArtifact { path: "p".into(), hash: "h".into(), status: ArtifactStatus::Passed }),
                theorem_count: 0,
                sorry_count: 0,
                status: ArtifactStatus::Passed,
                metadata: BTreeMap::new(),
            }],
            actions: vec![ActionPolicy {
                action: "dispense".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "test".into(),
            }],
        }
    }

    fn ctx() -> (GateContext, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate());
        let ctx = GateContext::new(
            "exp-1",
            0,
            Arc::new(Mutex::new(LabState::default())),
            Arc::new(Chain::open(dir.path().join("audit.jsonl"))),
            signer,
            Arc::new(SilaClients::simulator()),
            Arc::new(ProofChecker::from_manifest_trusted(manifest())),
            Arc::new(CapabilityPolicy::default_lab()),
            Arc::new(ApprovalQueue::new()),
            Arc::new(RevocationList::new()),
            Some(Duration::from_millis(100)),
        );
        (ctx, dir)
    }

    #[tokio::test]
    async fn runs_protocol_then_concludes() {
        unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
        let (ctx, _d) = ctx();
        let llm = Arc::new(ScriptedClient::new(vec![
            r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":50.0},"risk_class":"LiquidHandling"}]}"#.into(),
            r#"{"tool":"done","summary":"dispensed 50 µL"}"#.into(),
        ]));
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        let summary = orch.run("Dispense into tube_1", &ctx).await.unwrap();
        assert_eq!(summary, "dispensed 50 µL");
        // dispense (allow) + conclusion entries; chain verifies.
        let r = ctx.audit_chain.verify().unwrap();
        assert!(r.entries_checked >= 2);
    }

    #[tokio::test]
    async fn gate_rejection_ends_run() {
        let (ctx, _d) = ctx();
        let llm = Arc::new(ScriptedClient::new(vec![
            // Over-capacity volume → CapabilityGate rejects.
            r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":99999.0},"risk_class":"LiquidHandling"}]}"#.into(),
        ]));
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        let err = orch.run("x", &ctx).await.unwrap_err();
        assert!(matches!(err, OrchestratorError::Rejected(r) if r.gate == "CapabilityGate"));
    }

    #[tokio::test]
    async fn analyze_records_calibration_with_standards_and_approval() {
        let (ctx, _d) = ctx();
        // Register 5 certified reference materials (the calibration x-axis).
        {
            let mut lab = ctx.lab_state.lock().unwrap();
            for i in 0..5 {
                lab.register_reagent(reference_material(&format!("std-{i}")));
            }
        }
        // Approve the calibration as soon as it is requested.
        let approvals = ctx.approvals.clone();
        let approver = tokio::spawn(async move {
            for _ in 0..200 {
                if let Some(req) = approvals.list_pending().into_iter().next() {
                    approvals
                        .resolve(&req.id, axiom_gate::Decision { approved: true, notes: "ok".into(), approver_id: "alice".into() })
                        .unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });

        let llm = Arc::new(ScriptedClient::new(vec![
            r#"{"tool":"analyze_series","x":[1,2,3,4,5],"y":[2,4,6,8,10],"model":"linear","instrument":"spectrophotometer","reference_material_ids":["std-0","std-1","std-2","std-3","std-4"]}"#.into(),
            r#"{"tool":"done","summary":"calibrated"}"#.into(),
        ]));
        unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        orch.run("Calibrate", &ctx).await.unwrap();
        approver.await.unwrap();
        assert!(axiom_gate::latest_valid_until(&ctx.audit_chain, "spectrophotometer").unwrap().is_some());
    }

    #[tokio::test]
    async fn analyze_without_standards_does_not_calibrate() {
        let (ctx, _d) = ctx();
        let llm = Arc::new(ScriptedClient::new(vec![
            // No reference_material_ids → calibration is skipped, run still concludes.
            r#"{"tool":"analyze_series","x":[1,2,3,4,5],"y":[2,4,6,8,10],"model":"linear","instrument":"spectrophotometer"}"#.into(),
            r#"{"tool":"done","summary":"no calibration"}"#.into(),
        ]));
        unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        orch.run("Try to calibrate without standards", &ctx).await.unwrap();
        assert!(axiom_gate::latest_valid_until(&ctx.audit_chain, "spectrophotometer").unwrap().is_none());
    }

    fn reference_material(id: &str) -> axiom_types::Reagent {
        axiom_types::Reagent {
            id: id.into(),
            name: format!("Standard {id}"),
            cas_number: None,
            lot_number: "L".into(),
            concentration: None,
            concentration_unit: None,
            volume_ul: 1000.0,
            expiry_secs: None,
            ghs_hazard_codes: vec![],
            reference_material_id: Some(id.into()),
            nominal_ph: None,
            concentration_m: None,
            pka: None,
            is_buffer: false,
        }
    }
}
