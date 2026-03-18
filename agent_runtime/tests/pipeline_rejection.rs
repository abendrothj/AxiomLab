//! Full pipeline rejection integration tests.
//!
//! One `#[tokio::test]` per stage in the 5-stage validation pipeline.
//! Each test constructs a real `Orchestrator` with a real `ToolRegistry` and
//! calls `execute_tool_direct`.  Assertions check `result.success == false`
//! and that the output message contains stage-specific evidence.
//!
//! Stages exercised:
//! - Stage 0   (sandbox allowlist)
//! - Stage 0.5 (JSON schema parameter validation)
//! - Stage 1   (two-person approval — instant-deny path)
//! - Stage 2   (capability bounds)
//! - Stage 3   (fail-closed for high-risk without policy)
//! - Stage 4   (proof-artifact policy)
//! - Dispatch  (all stages pass → success)

use agent_runtime::{
    approvals::ApprovalPolicy,
    capabilities::CapabilityPolicy,
    llm::MockLlm,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
    sandbox::{ResourceLimits, Sandbox},
    tools::{ToolRegistry, register_lab_tools},
};
use proof_artifacts::{
    manifest::{
        ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest, RiskClass,
        VerusArtifact,
    },
    policy::{ExecutionContext, RuntimePolicyEngine},
};
use std::{collections::BTreeMap, path::PathBuf};
use tempfile::TempDir;

// ── Shared helpers ────────────────────────────────────────────────────────────

fn test_sandbox() -> Sandbox {
    Sandbox::new(
        vec![PathBuf::from("/lab/workspace")],
        vec!["move_arm".into(), "read_sensor".into(), "dispense".into()],
        ResourceLimits::default(),
    )
}

fn test_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    register_lab_tools(&mut r);
    r
}

fn test_build_id() -> BuildIdentity {
    BuildIdentity {
        git_commit: "abc123test".into(),
        binary_hash: "def456test".into(),
        workspace_hash: "ghi789test".into(),
        container_image_digest: None,
        device_id: None,
        firmware_version: None,
    }
}

fn test_exec_ctx() -> ExecutionContext {
    ExecutionContext {
        git_commit: "abc123test".into(),
        binary_hash: "def456test".into(),
        container_image_digest: None,
        device_id: None,
        firmware_version: None,
    }
}

/// Build a minimal passing manifest that maps `dispense` → `LiquidHandling`
/// with one Verus-backed Passed artifact.
fn passing_manifest() -> ProofManifest {
    ProofManifest {
        schema_version: 1,
        generated_unix_secs: 0,
        build: test_build_id(),
        artifacts: vec![ProofArtifact {
            id: "dispense-safety".into(),
            source_path: "vessel_physics/dispense.rs".into(),
            source_hash: "abc".into(),
            mir_path: None,
            mir_hash: None,
            lean: vec![],
            verus: Some(VerusArtifact {
                path: "verus_verified/dispense.rs".into(),
                hash: "abc".into(),
                status: ArtifactStatus::Passed,
            }),
            theorem_count: 1,
            sorry_count: 0,
            status: ArtifactStatus::Passed,
            metadata: BTreeMap::new(),
        }],
        actions: vec![ActionPolicy {
            action: "dispense".into(),
            risk_class: RiskClass::LiquidHandling,
            required_artifacts: vec!["dispense-safety".into()],
            rationale: "dispense safety verified by Verus".into(),
        }],
    }
}

fn minimal_config(audit_path: String) -> OrchestratorConfig {
    OrchestratorConfig {
        audit_log_path: Some(audit_path),
        approval_policy: None,
        capability_policy: None,
        session_nonce: None,
        audit_signer: None,
        revocation_list: RevocationList::default(),
        event_sink: None,
        approval_queue: None,
        ..OrchestratorConfig::default()
    }
}

// ── Stage 0: Sandbox ──────────────────────────────────────────────────────────

#[tokio::test]
async fn sandbox_rejects_unknown_tool() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    let orch = Orchestrator::new(MockLlm, test_sandbox(), test_registry(), minimal_config(audit_path));

    let result = orch.execute_tool_direct("nuke_lab", serde_json::json!({}), None).await;

    assert!(!result.success, "sandbox should have denied nuke_lab");
    let msg = result.output.to_string();
    assert!(
        msg.contains("nuke_lab") || msg.contains("not allowed") || msg.contains("allowlist"),
        "expected sandbox rejection message, got: {msg}"
    );
}

// ── Stage 0.5: Schema validation ──────────────────────────────────────────────

#[tokio::test]
async fn schema_rejects_invalid_param_type() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    let orch = Orchestrator::new(MockLlm, test_sandbox(), test_registry(), minimal_config(audit_path));

    // dispense.pump_id must be one of ["pump-A","pump-B","pump-C"] —
    // passing 42 (number) violates the "type: string" constraint.
    let result = orch
        .execute_tool_direct(
            "dispense",
            serde_json::json!({"pump_id": 42, "volume_ul": 100.0}),
            None,
        )
        .await;

    assert!(!result.success, "schema validation should have denied the call");
    let msg = result.output.to_string();
    assert!(
        msg.contains("parameter validation failed") || msg.contains("pump_id"),
        "expected schema rejection message, got: {msg}"
    );
}

// ── Stage 1: Approval (instant-deny path) ─────────────────────────────────────

#[tokio::test]
async fn approval_rejects_unapproved_high_risk() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    // Make dispense Actuation so it requires two-person approval.
    let mut manifest = passing_manifest();
    manifest.actions[0].risk_class = RiskClass::Actuation;

    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();

    let orch = Orchestrator::new(
        MockLlm,
        test_sandbox(),
        test_registry(),
        OrchestratorConfig {
            audit_log_path: Some(audit_path),
            approval_policy: Some(ApprovalPolicy::default_high_risk()),
            approval_queue: None, // no interactive queue → instant deny
            capability_policy: None,
            session_nonce: None,
            audit_signer: None,
            revocation_list: RevocationList::default(),
            event_sink: None,
            ..OrchestratorConfig::default()
        },
    )
    .with_runtime_policy(engine, test_exec_ctx());

    let result = orch
        .execute_tool_direct(
            "dispense",
            serde_json::json!({"pump_id": "pump-A", "volume_ul": 100.0}),
            None,
        )
        .await;

    assert!(!result.success, "approval stage should have denied unapproved actuation");
    let msg = result.output.to_string();
    assert!(
        msg.contains("approval"),
        "expected approval rejection message, got: {msg}"
    );
}

// ── Stage 2: Capability bounds ────────────────────────────────────────────────

#[tokio::test]
async fn capability_rejects_out_of_bounds() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    // Default lab policy: dispense.volume_ul max = 1000 µL.
    let orch = Orchestrator::new(
        MockLlm,
        test_sandbox(),
        test_registry(),
        OrchestratorConfig {
            audit_log_path: Some(audit_path),
            approval_policy: None,
            capability_policy: Some(CapabilityPolicy::default_lab()),
            session_nonce: None,
            audit_signer: None,
            revocation_list: RevocationList::default(),
            event_sink: None,
            approval_queue: None,
            ..OrchestratorConfig::default()
        },
    );

    let result = orch
        .execute_tool_direct(
            "dispense",
            serde_json::json!({"pump_id": "pump-A", "volume_ul": 9999.0}),
            None,
        )
        .await;

    assert!(!result.success, "capability stage should have denied out-of-bounds volume");
    let msg = result.output.to_string();
    assert!(
        msg.contains("volume_ul") || msg.contains("9999") || msg.contains("exceeds"),
        "expected capability rejection message, got: {msg}"
    );
}

// ── Stage 3: Fail-closed (actuation without policy engine) ────────────────────

#[tokio::test]
async fn failclosed_rejects_actuation_without_policy() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    // Inject a risk index that marks move_arm as Actuation, but provide NO
    // policy engine.  The orchestrator must fail-closed.
    let orch = Orchestrator::new(MockLlm, test_sandbox(), test_registry(), minimal_config(audit_path))
        .with_risk_index_only(
            [("move_arm".into(), RiskClass::Actuation)]
                .into_iter()
                .collect(),
        );

    let result = orch
        .execute_tool_direct(
            "move_arm",
            serde_json::json!({"x": 10.0, "y": 10.0, "z": 10.0}),
            None,
        )
        .await;

    assert!(!result.success, "fail-closed stage should have denied actuation without policy");
    let msg = result.output.to_string();
    assert!(
        msg.contains("proof policy") || msg.contains("high-risk"),
        "expected fail-closed rejection message, got: {msg}"
    );
}

// ── Stage 4: Proof-artifact policy ───────────────────────────────────────────

#[tokio::test]
async fn proof_policy_rejects_failed_artifact() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    // Manifest: dispense requires an artifact that has ArtifactStatus::Failed.
    let mut manifest = passing_manifest();
    manifest.artifacts[0].status = ArtifactStatus::Failed;

    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();

    let orch = Orchestrator::new(MockLlm, test_sandbox(), test_registry(), minimal_config(audit_path))
        .with_runtime_policy(engine, test_exec_ctx());

    let result = orch
        .execute_tool_direct(
            "dispense",
            serde_json::json!({"pump_id": "pump-A", "volume_ul": 100.0}),
            None,
        )
        .await;

    assert!(!result.success, "proof policy stage should have denied failed artifact");
    let msg = result.output.to_string();
    assert!(
        msg.contains("artifact") || msg.contains("Failed") || msg.contains("status"),
        "expected proof policy rejection message, got: {msg}"
    );
}

// ── Dispatch: all stages pass ─────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_succeeds_through_all_stages() {
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl").to_string_lossy().into_owned();

    // read_sensor is ReadOnly, needs no approval, no capability limit, no
    // proof policy → should pass every stage and succeed.
    let orch = Orchestrator::new(MockLlm, test_sandbox(), test_registry(), minimal_config(audit_path));

    let result = orch
        .execute_tool_direct(
            "read_sensor",
            serde_json::json!({"sensor_id": "pH-1"}),
            None,
        )
        .await;

    assert!(
        result.success,
        "dispatch should succeed for valid read_sensor call; output: {}",
        result.output
    );
}
