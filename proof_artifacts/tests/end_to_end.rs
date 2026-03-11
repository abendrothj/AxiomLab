use proof_artifacts::cache::ProofCache;
use proof_artifacts::ci::{CiGatePolicy, evaluate_ci_gate};
use proof_artifacts::generator::{ArtifactInput, GenerateRequest, ManifestGenerator};
use proof_artifacts::manifest::{ActionPolicy, ArtifactStatus, BuildIdentity};
use proof_artifacts::policy::{ExecutionContext, PolicyDecision, RuntimePolicyEngine};
use std::collections::BTreeMap;
use std::fs;

#[test]
fn full_pipeline_manifest_gate_policy_explain_cache() {
    let tmp = tempfile::tempdir().unwrap();

    let src = tmp.path().join("arm.rs");
    let mir = tmp.path().join("arm.mir");
    let lean = tmp.path().join("ArmSafety.lean");
    let verus = tmp.path().join("arm_safety.vrs");

    fs::write(&src, "pub fn move_arm(mm: u64) -> u64 { mm }\n").unwrap();
    fs::write(&mir, "MIR body for move_arm\n").unwrap();
    fs::write(
        &lean,
        "theorem arm_safe_bound : True := by\n  trivial\nexample : True := by trivial\n",
    )
    .unwrap();
    fs::write(&verus, "proof arm_safe(mm)\n").unwrap();

    let req = GenerateRequest {
        build: BuildIdentity {
            git_commit: "abc123".into(),
            binary_hash: "binhash".into(),
            workspace_hash: "wshash".into(),
        },
        artifacts: vec![ArtifactInput {
            id: "arm_safety".into(),
            source_path: src.clone(),
            mir_path: Some(mir.clone()),
            lean_paths: vec![lean.clone()],
            verus_proof_path: Some(verus.clone()),
            metadata: BTreeMap::from([("domain".into(), "hardware".into())]),
        }],
        actions: vec![ActionPolicy {
            action: "move_arm".into(),
            required_artifacts: vec!["arm_safety".into()],
            rationale: "Arm movement requires verified bound proofs".into(),
        }],
    };

    let mut cache = ProofCache::default();
    let manifest = ManifestGenerator::generate(&req, Some(&mut cache)).unwrap();

    assert_eq!(manifest.artifacts.len(), 1);
    let art = &manifest.artifacts[0];
    assert_eq!(art.id, "arm_safety");
    assert_eq!(art.status, ArtifactStatus::Passed);
    assert_eq!(art.theorem_count, 2);
    assert_eq!(art.sorry_count, 0);
    assert_eq!(art.lean[0].theorem_count, 2);
    assert_eq!(art.lean[0].sorry_count, 0);

    let policy = CiGatePolicy {
        required_artifacts: vec!["arm_safety".into()],
        require_zero_sorry: true,
        expected_git_commit: Some("abc123".into()),
        expected_binary_hash: Some("binhash".into()),
    };
    let gate = evaluate_ci_gate(&manifest, &policy);
    assert!(gate.passed, "violations: {:?}", gate.violations);

    let engine = RuntimePolicyEngine::new(manifest.clone());
    let ctx = ExecutionContext {
        git_commit: "abc123".into(),
        binary_hash: "binhash".into(),
    };
    engine.authorize("move_arm", &ctx).unwrap();

    let report = engine.explain("move_arm");
    assert_eq!(report.decision, PolicyDecision::Allow);
    assert_eq!(report.artifacts_checked.len(), 1);
    assert!(report.reason.contains("passed"));

    // Incremental cache proof: second generation keeps same counts/status.
    let manifest2 = ManifestGenerator::generate(&req, Some(&mut cache)).unwrap();
    assert_eq!(manifest2.artifacts[0].theorem_count, manifest.artifacts[0].theorem_count);
    assert_eq!(manifest2.artifacts[0].sorry_count, manifest.artifacts[0].sorry_count);
    assert_eq!(manifest2.artifacts[0].status, manifest.artifacts[0].status);
}
