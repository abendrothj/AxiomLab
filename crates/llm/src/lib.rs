//! The thin orchestrator: the LLM proposes, the pipeline enforces.
//!
//! No hypothesis tracking, no finding counts, no convergence gates, no journal.
//! Each iteration rebuilds the mandate from the audit chain, asks for one
//! proposal, and either runs it through the [`Pipeline`] or concludes.
//!
//! A gate rejection does **not** end the run: the reason is fed back into the
//! next mandate so the model can revise, bounded by a rejection budget
//! (`AXIOMLAB_MAX_REJECTIONS`). This is orthogonal to hardware fail-closed — the
//! gates still reject every unsafe action; the budget only limits how long the
//! model may flail before the orchestrator gives up.

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
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("llm: {0}")]
    Llm(#[from] LlmError),
    #[error("aborted after {count} rejections; last: {last}")]
    TooManyRejections { count: u32, last: String },
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

    /// Run until the LLM concludes (returns its summary), the rejection budget
    /// is exhausted, or the iteration limit is reached.
    ///
    /// A gate rejection is fed back into the next mandate (`last_rejection`)
    /// rather than ending the run; `AXIOMLAB_MAX_REJECTIONS` bounds the flailing.
    pub async fn run(&self, directive: &str, ctx: &GateContext) -> Result<String, OrchestratorError> {
        let max_iter = max_iterations();
        let max_rej = max_rejections();
        let mut last_rejection: Option<String> = None;
        let mut rejections = 0u32;

        // Records a rejection; returns Err to abort once the budget is spent.
        macro_rules! note_rejection {
            ($reason:expr) => {{
                let reason: String = $reason;
                rejections += 1;
                if rejections >= max_rej {
                    return Err(OrchestratorError::TooManyRejections { count: rejections, last: reason });
                }
                last_rejection = Some(reason);
            }};
        }

        for _ in 0..max_iter {
            let mandate = build_mandate(directive, ctx, last_rejection.as_deref());
            match self.llm.propose(&mandate).await? {
                Proposal::Protocol(steps) => {
                    let mut rejected = false;
                    for step in steps {
                        // A rejected step abandons the rest of this proposal; the
                        // reason is fed back so the model can revise next turn.
                        if let Err(rej) = self.pipeline.run(step, ctx).await {
                            rejected = true;
                            note_rejection!(rej.to_string());
                            break;
                        }
                    }
                    if !rejected {
                        last_rejection = None;
                    }
                }
                Proposal::Analyze(req) => {
                    let standards = ctx.lab_state.lock().unwrap().registered_reference_materials();
                    match analyze_series(&req, &standards) {
                        Ok(outcome) => {
                            last_rejection = None;
                            // A calibration unlocks the measurement tools, so it
                            // requires operator sign-off before it is recorded.
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
                                            .map_err(|e| OrchestratorError::TooManyRejections { count: rejections, last: e.to_string() })?;
                                    }
                                    // Declining to calibrate is not a hard failure.
                                    Err(reason) => tracing::warn!(%reason, "calibration not approved — not recorded"),
                                }
                            }
                        }
                        Err(e) => note_rejection!(format!("analyze_series rejected: {e}")),
                    }
                }
                Proposal::Done { summary } => {
                    // Rekor anchoring failures are non-fatal (logged inside).
                    let _ = record_conclusion(ctx, &summary).await;
                    return Ok(summary);
                }
            }
        }
        Err(OrchestratorError::MaxIterations(max_iter))
    }
}

fn max_iterations() -> u32 {
    std::env::var("AXIOMLAB_MAX_ITERATIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(50)
}

fn max_rejections() -> u32 {
    std::env::var("AXIOMLAB_MAX_REJECTIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(5)
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
    async fn gate_rejection_is_recoverable() {
        unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
        let (ctx, _d) = ctx();
        let llm = Arc::new(ScriptedClient::new(vec![
            // Over-capacity → rejected; the model then corrects and finishes.
            r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":99999.0}}]}"#.into(),
            r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":50.0}}]}"#.into(),
            r#"{"tool":"done","summary":"recovered"}"#.into(),
        ]));
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        assert_eq!(orch.run("dispense safely", &ctx).await.unwrap(), "recovered");
        // The chain shows the deny followed by the successful allow.
        let entries = ctx.audit_chain.entries().unwrap();
        assert!(entries.iter().any(|e| e.decision == "deny"));
        assert!(entries.iter().any(|e| e.decision == "allow" && e.action == "dispense"));
    }

    #[tokio::test]
    async fn exhausts_rejection_budget() {
        unsafe { std::env::set_var("AXIOMLAB_MAX_REJECTIONS", "3") };
        let (ctx, _d) = ctx();
        let bad = r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":99999.0}}]}"#;
        let llm = Arc::new(ScriptedClient::new(vec![bad.into(), bad.into(), bad.into()]));
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        let err = orch.run("x", &ctx).await.unwrap_err();
        unsafe { std::env::remove_var("AXIOMLAB_MAX_REJECTIONS") };
        assert!(matches!(err, OrchestratorError::TooManyRejections { count: 3, .. }), "got {err:?}");
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
