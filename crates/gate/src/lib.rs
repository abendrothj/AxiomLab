//! The fail-closed gate pipeline — the entire safety story.
//!
//! An LLM-proposed [`Action`] flows through an ordered chain of gates. Every gate
//! returns `Result<(), Rejection>`; the first `Err` hard-stops the action and is
//! itself audited. Nothing touches hardware until every gate before
//! [`gates::ExecuteGate`] has passed.
//!
//! ```text
//! Capability → Chemistry → Calibration → Proof → Approval → Execute → Audit
//! ```

mod analyze;
mod approvals;
mod calibration;
mod capability;
mod context;
mod fitting;
mod gates;

pub use analyze::{AnalyzeRequest, CALIBRATION_R2_THRESHOLD, analyze_series};
pub use approvals::{ApprovalQueue, ApprovalRequest, Decision};
pub use calibration::{latest_valid_until, measurement_instrument, record_calibration};
pub use capability::{ActionCapability, CapabilityPolicy, NumericRange};
pub use context::GateContext;

use async_trait::async_trait;
use axiom_audit::{EntryData, RekorClient};
use axiom_types::{Action, Rejection};
use std::sync::Arc;

/// One stage of the pipeline.
#[async_trait]
pub trait Gate: Send + Sync {
    fn name(&self) -> &'static str;
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection>;
}

/// The ordered chain of gates.
pub struct Pipeline {
    gates: Vec<Arc<dyn Gate>>,
}

impl Pipeline {
    /// Build the standard 7-stage pipeline in the fixed safety order.
    pub fn standard() -> Self {
        Self {
            gates: vec![
                Arc::new(gates::CapabilityGate),
                Arc::new(gates::ChemistryGate),
                Arc::new(gates::CalibrationGate),
                Arc::new(gates::ProofGate),
                Arc::new(gates::ApprovalGate),
                Arc::new(gates::ExecuteGate),
                Arc::new(gates::AuditGate),
            ],
        }
    }

    /// Construct from an explicit gate list (tests / custom topologies).
    pub fn from_gates(gates: Vec<Arc<dyn Gate>>) -> Self {
        Self { gates }
    }

    pub fn gate_names(&self) -> Vec<&'static str> {
        self.gates.iter().map(|g| g.name()).collect()
    }

    /// Run `action` through every gate. On the first rejection, audit it and
    /// return the [`Rejection`]; otherwise return the (unchanged) action.
    pub async fn run(&self, action: Action, ctx: &GateContext) -> Result<Action, Rejection> {
        ctx.reset_scratch();
        for gate in &self.gates {
            if let Err(rej) = gate.check(&action, ctx).await {
                gates::audit_rejection(ctx, &rej);
                return Err(rej);
            }
        }
        Ok(action)
    }
}

/// Record a protocol conclusion in the chain and anchor the chain tip in Rekor
/// (unless `AXIOMLAB_REKOR_DISABLED=1`). Called by the orchestrator on `Done`.
pub async fn record_conclusion(ctx: &GateContext, summary: &str) -> Result<(), String> {
    let reason = serde_json::json!({
        "experiment_id": ctx.experiment_id,
        "summary": summary,
    })
    .to_string();
    let entry = EntryData::new("protocol_conclusion", "allow", reason, true)
        .with_reasoning_text(summary.chars().take(4096).collect::<String>());
    ctx.audit_chain
        .append(entry, ctx.signer.as_ref())
        .map_err(|e| format!("append conclusion: {e}"))?;

    if let Some(tip) = ctx.audit_chain.tip_hash().map_err(|e| format!("tip hash: {e}"))? {
        match RekorClient::from_env().checkpoint(&tip, ctx.signer.as_ref()).await {
            Ok(Some(log)) => {
                let entry = EntryData::new(
                    "rekor_checkpoint",
                    "allow",
                    serde_json::json!({ "uuid": log.uuid, "integrated_time": log.integrated_time }).to_string(),
                    true,
                )
                .with_rekor_uuid(log.uuid.clone());
                ctx.audit_chain.append(entry, ctx.signer.as_ref()).map_err(|e| format!("append rekor: {e}"))?;
            }
            Ok(None) => tracing::info!("Rekor anchoring disabled; conclusion recorded locally"),
            // A Rekor failure never retroactively fails the run.
            Err(e) => tracing::warn!(error = %e, "Rekor anchoring failed (non-fatal)"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::Decision;
    use axiom_audit::{Chain, LocalSigner, RevocationList};
    use axiom_proofs::{
        ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofChecker, ProofManifest,
        VerusArtifact,
    };
    use axiom_sila::SilaClients;
    use axiom_types::{LabState, RiskClass};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    fn test_manifest() -> ProofManifest {
        let verus = ProofArtifact {
            id: "lab_safety_verus".into(),
            source_path: "verus_verified/lab_safety.rs".into(),
            source_hash: "h".into(),
            mir_path: None,
            mir_hash: None,
            lean: vec![],
            verus: Some(VerusArtifact {
                path: "verus_verified/lab_safety.rs".into(),
                hash: "h".into(),
                status: ArtifactStatus::Passed,
            }),
            theorem_count: 0,
            sorry_count: 0,
            status: ArtifactStatus::Passed,
            metadata: BTreeMap::new(),
        };
        let policy = |action: &str, risk: RiskClass| ActionPolicy {
            action: action.into(),
            risk_class: risk,
            required_artifacts: vec!["lab_safety_verus".into()],
            rationale: "test".into(),
        };
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
            artifacts: vec![verus],
            actions: vec![
                policy("dispense", RiskClass::LiquidHandling),
                policy("move_arm", RiskClass::Actuation),
            ],
        }
    }

    fn ctx() -> (GateContext, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let chain = Arc::new(Chain::open(dir.path().join("audit.jsonl")));
        let signer: Arc<dyn axiom_audit::Signer> = Arc::new(LocalSigner::generate());
        let ctx = GateContext::new(
            "exp-1",
            0,
            Arc::new(Mutex::new(LabState::default())),
            chain,
            signer,
            Arc::new(SilaClients::simulator()),
            Arc::new(ProofChecker::from_manifest_trusted(test_manifest())),
            Arc::new(CapabilityPolicy::default_lab()),
            Arc::new(ApprovalQueue::new()),
            Arc::new(RevocationList::new()),
            Some(Duration::from_millis(200)),
        );
        (ctx, dir)
    }

    #[tokio::test]
    async fn liquid_handling_passes_and_is_audited() {
        let (ctx, _d) = ctx();
        let action = Action::new(
            "dispense",
            serde_json::json!({"vessel_id": "tube_1", "volume_ul": 100.0}),
            RiskClass::LiquidHandling,
        );
        let pipeline = Pipeline::standard();
        assert!(pipeline.run(action, &ctx).await.is_ok());
        // One allow entry recorded; chain verifies.
        let r = ctx.audit_chain.verify().unwrap();
        assert_eq!(r.entries_checked, 1);
    }

    #[tokio::test]
    async fn capability_violation_rejected_and_audited() {
        let (ctx, _d) = ctx();
        let action = Action::new(
            "dispense",
            serde_json::json!({"vessel_id": "tube_1", "volume_ul": 99999.0}),
            RiskClass::LiquidHandling,
        );
        let err = Pipeline::standard().run(action, &ctx).await.unwrap_err();
        assert_eq!(err.gate, "CapabilityGate");
        // The rejection was audited as a deny.
        let entries = ctx.audit_chain.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "deny");
    }

    #[tokio::test]
    async fn measurement_blocked_without_calibration() {
        let (ctx, _d) = ctx();
        // read_absorbance has no proof policy in the manifest, but CalibrationGate
        // runs before ProofGate and should reject first.
        let action = Action::new(
            "read_absorbance",
            serde_json::json!({"vessel_id": "tube_1", "wavelength_nm": 500.0}),
            RiskClass::ReadOnly,
        );
        let err = Pipeline::standard().run(action, &ctx).await.unwrap_err();
        assert_eq!(err.gate, "CalibrationGate");
    }

    #[tokio::test]
    async fn measurement_allowed_after_calibration() {
        let (ctx, _d) = ctx();
        // Record a calibration via analyze_series, then add a proof policy for the read.
        let req = AnalyzeRequest {
            x: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            y: vec![2.0, 4.0, 6.0, 8.0, 10.0],
            model: Some("linear".into()),
            instrument: Some("spectrophotometer".into()),
        };
        analyze_series(&req, &ctx.audit_chain, ctx.signer.as_ref()).unwrap();

        // Build a context whose manifest also authorises read_absorbance.
        let mut m = test_manifest();
        m.actions.push(ActionPolicy {
            action: "read_absorbance".into(),
            risk_class: RiskClass::ReadOnly,
            required_artifacts: vec!["lab_safety_verus".into()],
            rationale: "test".into(),
        });
        let ctx = GateContext { proofs: Arc::new(ProofChecker::from_manifest_trusted(m)), ..ctx };

        let action = Action::new(
            "read_absorbance",
            serde_json::json!({"vessel_id": "tube_1", "wavelength_nm": 500.0}),
            RiskClass::ReadOnly,
        );
        assert!(Pipeline::standard().run(action, &ctx).await.is_ok());
    }

    #[tokio::test]
    async fn actuation_times_out_without_approval() {
        let (ctx, _d) = ctx();
        let action = Action::new(
            "move_arm",
            serde_json::json!({"x": 100.0, "y": 100.0, "z": 50.0}),
            RiskClass::Actuation,
        );
        let err = Pipeline::standard().run(action, &ctx).await.unwrap_err();
        assert_eq!(err.gate, "ApprovalGate");
    }

    #[tokio::test]
    async fn actuation_proceeds_when_approved() {
        let (ctx, _d) = ctx();
        let approvals = ctx.approvals.clone();
        let action = Action::new(
            "move_arm",
            serde_json::json!({"x": 100.0, "y": 100.0, "z": 50.0}),
            RiskClass::Actuation,
        );
        // Approve as soon as the request appears.
        let approver = tokio::spawn(async move {
            for _ in 0..50 {
                let pending = approvals.list_pending();
                if let Some(req) = pending.first() {
                    approvals
                        .resolve(&req.id, Decision { approved: true, notes: "ok".into(), approver_id: "op1".into() })
                        .unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let res = Pipeline::standard().run(action, &ctx).await;
        approver.await.unwrap();
        assert!(res.is_ok(), "approved actuation should pass: {res:?}");
    }

    #[tokio::test]
    async fn chemistry_incompatibility_rejected() {
        let (ctx, _d) = ctx();
        {
            let mut lab = ctx.lab_state.lock().unwrap();
            lab.register_reagent(axiom_types::Reagent {
                id: "r-hcl".into(),
                name: "HCl".into(),
                cas_number: None,
                lot_number: "L".into(),
                concentration: None,
                concentration_unit: None,
                volume_ul: 1000.0,
                expiry_secs: None,
                ghs_hazard_codes: vec![],
                reference_material_id: None,
                nominal_ph: None,
                concentration_m: None,
                pka: None,
                is_buffer: false,
            });
            lab.add_to_vessel("tube_1", "r-hcl", 100.0);
        }
        let action = Action::new(
            "dispense",
            serde_json::json!({"vessel_id": "tube_1", "reagent": "NaOH", "volume_ul": 100.0}),
            RiskClass::LiquidHandling,
        );
        let err = Pipeline::standard().run(action, &ctx).await.unwrap_err();
        assert_eq!(err.gate, "ChemistryGate");
    }
}
