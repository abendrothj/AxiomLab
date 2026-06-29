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

use axiom_gate::{GateContext, Pipeline, analyze_series, record_conclusion};
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
                    analyze_series(&req, &ctx.audit_chain, ctx.signer.as_ref())
                        .map_err(OrchestratorError::Analyze)?;
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
    async fn analyze_records_calibration() {
        let (ctx, _d) = ctx();
        let llm = Arc::new(ScriptedClient::new(vec![
            r#"{"tool":"analyze_series","x":[1,2,3,4,5],"y":[2,4,6,8,10],"model":"linear","instrument":"spectrophotometer"}"#.into(),
            r#"{"tool":"done","summary":"calibrated"}"#.into(),
        ]));
        unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
        let orch = Orchestrator::new(llm, Arc::new(Pipeline::standard()));
        orch.run("Calibrate", &ctx).await.unwrap();
        assert!(axiom_gate::latest_valid_until(&ctx.audit_chain, "spectrophotometer").unwrap().is_some());
    }
}
