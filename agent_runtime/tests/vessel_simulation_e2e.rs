//! End-to-end vessel simulation tests — full safety pipeline.
//!
//! Every operation passes through the complete AxiomLab safety chain before
//! reaching the SiLA 2 gRPC layer:
//!
//!   sandbox allowlist → capability bounds → proof policy → gRPC → physics
//!
//! The proof policy requires a Verus-backed `lab_safety_verus` artifact with
//! `ArtifactStatus::Passed` for LiquidHandling actions (dispense, aspirate).
//! ReadOnly actions (read_absorbance) carry no artifact requirement and pass
//! even when the Verus artifact has failed.
//!
//! The physics assertions are only possible because the Python SiLA 2 mock
//! now maintains real vessel state via `VesselRegistry`:
//!  • Absorbance is Beer-Lambert (A = ε × fill × l × spectral_factor), not random.
//!  • Dispensing beyond capacity returns a gRPC error from Python.
//!  • Aspirating more than present returns a gRPC error from Python.
//!  • Sequential dispenses produce strictly monotonic absorbance.
//!
//! Prerequisites: SiLA 2 mock must be running on :50052
//!   cd sila_mock && python -m axiomlab_mock --insecure
//!
//! Run:
//!   cargo test -p agent_runtime --test vessel_simulation_e2e -- --ignored --test-threads=1

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_runtime::capabilities::CapabilityPolicy;
use agent_runtime::hardware::SiLA2Clients;
use agent_runtime::sandbox::{ResourceLimits, Sandbox};
use agent_runtime::tools::{ToolCall, ToolRegistry, ToolSpec};
use proof_artifacts::manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest, RiskClass,
    VerusArtifact,
};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};

const ADDR: &str = "http://127.0.0.1:50052";

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline helpers
// ─────────────────────────────────────────────────────────────────────────────

fn lab_sandbox() -> Sandbox {
    Sandbox::new(
        vec![PathBuf::from("/lab/workspace")],
        vec![
            "dispense".into(),
            "aspirate".into(),
            "move_arm".into(),
            "read_absorbance".into(),
            "set_temperature".into(),
            "read_temperature".into(),
            "incubate".into(),
            "spin_centrifuge".into(),
            "read_centrifuge_temperature".into(),
            "calibrate_ph".into(),
            "read_ph".into(),
        ],
        ResourceLimits::default(),
    )
}

fn register_lab_tools(registry: &mut ToolRegistry, clients: Arc<SiLA2Clients>) {
    // ── dispense ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "dispense".into(),
            description: "Dispense liquid via SiLA 2 LiquidHandler".into(),
            parameters_schema: serde_json::json!({}),
            ..Default::default()
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let vessel = params["pump_id"].as_str().ok_or("missing pump_id")?;
                let vol = params["volume_ul"].as_f64().ok_or("missing volume_ul")?;
                c.dispense(vessel, vol).await
            })
        }),
    );

    // ── aspirate ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "aspirate".into(),
            description: "Aspirate liquid via SiLA 2 LiquidHandler".into(),
            parameters_schema: serde_json::json!({}),
            ..Default::default()
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let vessel = params["source_vessel"].as_str().ok_or("missing source_vessel")?;
                let vol = params["volume_ul"].as_f64().ok_or("missing volume_ul")?;
                c.aspirate(vessel, vol).await
            })
        }),
    );

    // ── read_absorbance ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "read_absorbance".into(),
            description: "Read spectrophotometer via SiLA 2".into(),
            parameters_schema: serde_json::json!({}),
            ..Default::default()
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let vessel = params["vessel_id"].as_str().ok_or("missing vessel_id")?;
                let wl = params["wavelength_nm"].as_f64().ok_or("missing wavelength_nm")?;
                c.read_absorbance(vessel, wl).await
            })
        }),
    );
}

fn vessel_ctx() -> ExecutionContext {
    ExecutionContext {
        git_commit: "test123".into(),
        binary_hash: "bin123".into(),
        container_image_digest: None,
        device_id: None,
        firmware_version: None,
    }
}

/// Build a proof manifest covering the three liquid-lab actions:
///  - dispense   — LiquidHandling, requires `lab_safety_verus`
///  - aspirate   — LiquidHandling, requires `lab_safety_verus`
///  - read_absorbance — ReadOnly, no artifact required
///
/// Pass `ArtifactStatus::Passed` for normal operation;
/// pass `ArtifactStatus::Failed` to test that the policy engine blocks
/// LiquidHandling operations while still allowing ReadOnly reads.
fn vessel_manifest(verus_status: ArtifactStatus) -> ProofManifest {
    ProofManifest {
        schema_version: 1,
        generated_unix_secs: 0,
        build: BuildIdentity {
            git_commit: "test123".into(),
            binary_hash: "bin123".into(),
            workspace_hash: "ws".into(),
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
            verus: Some(VerusArtifact {
                path: "verus_verified/lab_safety.rs".into(),
                hash: "h".into(),
                status: verus_status.clone(),
            }),
            theorem_count: 6,
            sorry_count: 0,
            status: verus_status,
            metadata: BTreeMap::new(),
        }],
        actions: vec![
            ActionPolicy {
                action: "dispense".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Liquid dispensing requires volume safety proof".into(),
            },
            ActionPolicy {
                action: "aspirate".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Liquid aspiration requires volume safety proof".into(),
            },
            ActionPolicy {
                action: "read_absorbance".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only measurement, no proof required".into(),
            },
        ],
    }
}

/// Build an `ExecutionContext` whose identity fields match a given manifest's
/// `BuildIdentity`.  Used so physics tests don't hard-code values that diverge
/// from the real manifest generated by Verus.
fn ctx_from_manifest(manifest: &ProofManifest) -> ExecutionContext {
    ExecutionContext {
        git_commit: manifest.build.git_commit.clone(),
        binary_hash: manifest.build.binary_hash.clone(),
        container_image_digest: manifest.build.container_image_digest.clone(),
        device_id: manifest.build.device_id.clone(),
        firmware_version: manifest.build.firmware_version.clone(),
    }
}

/// Load the real `ProofManifest` generated by `vessel_physics/generate_manifest.py`.
///
/// This manifest has `ArtifactStatus::Passed` iff `verus verus_verified/vessel_registry.rs`
/// exited 0 — i.e., all 11 theorems in the Verus proof were verified by Z3.
/// Physics tests (`sim_*`) use this so that `ArtifactStatus::Passed` is set by
/// the Verus compiler, not by a test fixture.
fn load_lab_manifest() -> ProofManifest {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("proof_artifacts/vessel_physics_manifest.json");
    let json = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "Could not read {}: {e}\n\
             Run: python3 vessel_physics/generate_manifest.py",
            manifest_path.display()
        )
    });
    serde_json::from_str(&json).unwrap_or_else(|e| {
        panic!("Invalid manifest JSON in {}: {e}", manifest_path.display())
    })
}

/// Full four-stage pipeline:
///   1. Sandbox allowlist
///   2. Capability bounds
///   3. Proof policy (Verus artifact gating)
///   4. gRPC dispatch → Python SiLA 2 mock → VesselRegistry physics
async fn run_guarded(
    sandbox: &Sandbox,
    policy: &CapabilityPolicy,
    engine: &RuntimePolicyEngine,
    ctx: &ExecutionContext,
    registry: &ToolRegistry,
    call: &ToolCall,
) -> Result<serde_json::Value, String> {
    sandbox
        .check_command(&call.name)
        .map_err(|e| format!("SANDBOX: {e}"))?;

    policy
        .validate(&call.name, &call.params, None)
        .map_err(|e| format!("CAPABILITY: {e}"))?;

    engine
        .authorize(&call.name, ctx)
        .map_err(|e| format!("PROOF_POLICY: {e}"))?;

    let result = registry.dispatch(call).await;
    if result.success {
        Ok(result.output)
    } else {
        Err(format!("DISPATCH: {}", result.output))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ToolCall constructors
// ─────────────────────────────────────────────────────────────────────────────

fn dispense_call(vessel: &str, vol: f64) -> ToolCall {
    ToolCall {
        name: "dispense".into(),
        params: serde_json::json!({ "pump_id": vessel, "volume_ul": vol }),
    }
}

fn aspirate_call(vessel: &str, vol: f64) -> ToolCall {
    ToolCall {
        name: "aspirate".into(),
        params: serde_json::json!({ "source_vessel": vessel, "volume_ul": vol }),
    }
}

fn read_call(vessel: &str, wavelength_nm: f64) -> ToolCall {
    ToolCall {
        name: "read_absorbance".into(),
        params: serde_json::json!({ "vessel_id": vessel, "wavelength_nm": wavelength_nm }),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Proof-policy gating tests — pure Rust, no SiLA 2 mock required
// ═══════════════════════════════════════════════════════════════════════════

/// Dispense is LiquidHandling and requires `lab_safety_verus` to be Passed.
/// A Failed artifact must cause the engine to reject the call before dispatch.
#[tokio::test]
async fn proof_dispense_blocked_with_failed_verus() {
    let engine = RuntimePolicyEngine::new(vessel_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = vessel_ctx();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new(); // empty — must never reach dispatch

    let err = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &dispense_call("any_vessel", 100.0),
    )
    .await
    .unwrap_err();

    assert!(
        err.contains("PROOF_POLICY"),
        "dispense must be blocked by failed Verus artifact: {err}"
    );
}

/// Aspirate is LiquidHandling and also requires `lab_safety_verus` to be Passed.
#[tokio::test]
async fn proof_aspirate_blocked_with_failed_verus() {
    let engine = RuntimePolicyEngine::new(vessel_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = vessel_ctx();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    let err = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &aspirate_call("any_vessel", 100.0),
    )
    .await
    .unwrap_err();

    assert!(
        err.contains("PROOF_POLICY"),
        "aspirate must be blocked by failed Verus artifact: {err}"
    );
}

/// read_absorbance is ReadOnly with no required artifacts.  Even a Failed
/// Verus artifact must not prevent the proof engine from authorising this call.
/// The call will fail at DISPATCH (no handler in empty registry), but never at
/// PROOF_POLICY — confirming the engine does not over-gate read-only operations.
#[tokio::test]
async fn proof_failed_verus_does_not_block_read_absorbance() {
    let engine = RuntimePolicyEngine::new(vessel_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = vessel_ctx();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new(); // empty — will fail at DISPATCH, not PROOF_POLICY

    let err = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &read_call("any_vessel", 500.0),
    )
    .await
    .unwrap_err();

    assert!(
        !err.contains("PROOF_POLICY"),
        "read_absorbance must not be blocked by proof policy (got: {err})"
    );
    assert!(
        err.contains("DISPATCH"),
        "failure must occur at dispatch (no handler registered): {err}"
    );
}

/// An unsigned manifest must block all actions — even read_absorbance.
#[tokio::test]
async fn proof_unsigned_manifest_blocks_all_operations() {
    let engine = RuntimePolicyEngine::new(vessel_manifest(ArtifactStatus::Passed));
    // NOT calling .mark_signature_verified() — manifest is unsigned
    let ctx = vessel_ctx();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    for call in [
        dispense_call("v", 100.0),
        aspirate_call("v", 100.0),
        read_call("v", 500.0),
    ] {
        let err = run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &call)
            .await
            .unwrap_err();
        assert!(
            err.contains("PROOF_POLICY") && err.contains("signature"),
            "unsigned manifest must block '{}': {err}",
            call.name
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Physics simulation tests — require SiLA 2 mock on :50052
// Every test uses the full 4-stage pipeline with ArtifactStatus::Passed.
// ═══════════════════════════════════════════════════════════════════════════

// Each test uses a unique vessel ID so tests never share state even when run
// sequentially against the same server process.  Pre-registered vessels
// (beaker_A, tube_1…tube_3, plate_well_A1/B1, reservoir) are used only
// where their pre-configured properties (capacity, ε, path length) are
// load-bearing for the assertion.

// ─────────────────────────────────────────────────────────────────────────────
// 1. Baseline: empty vessel → near-zero absorbance
// ─────────────────────────────────────────────────────────────────────────────

/// An auto-registered vessel starts at 0 µL.  The Spectrophotometer must
/// return the instrument baseline (0.001 AU) because fill fraction is zero.
/// This call is ReadOnly — no Verus proof required; confirmed by the engine
/// authorising it even though the Passed manifest is present.
#[tokio::test]
#[ignore]
async fn sim_empty_vessel_absorbance_is_baseline() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, clients);

    let result = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &read_call("sim_empty_001", 500.0),
    )
    .await
    .expect("read_absorbance on empty vessel must succeed through full pipeline");

    let abs = result["absorbance"].as_f64().unwrap();
    // fill=0 → A_base=0 → max(0.001, 0) = 0.001 AU exactly
    assert!(
        abs >= 0.001 && abs <= 0.0011,
        "empty vessel absorbance must equal instrument baseline (~0.001 AU): {abs}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Single dispense raises absorbance above baseline
// ─────────────────────────────────────────────────────────────────────────────

/// After dispensing 1 000 µL into a fresh auto-registered vessel (capacity
/// 50 000 µL, ε=1.0, l=1.0), fill=2 %.  At λ=500 nm:
///   A = 1.0 × 0.02 × 1.0 × 1.0 = 0.02 AU  ± 2 % → [0.0196, 0.0204]
#[tokio::test]
#[ignore]
async fn sim_dispense_raises_absorbance_above_baseline() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let vessel = "sim_raise_002";

    let before = run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
        .await
        .unwrap()["absorbance"]
        .as_f64()
        .unwrap();

    run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &dispense_call(vessel, 1000.0))
        .await
        .expect("dispense must succeed through full pipeline");

    let after = run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
        .await
        .unwrap()["absorbance"]
        .as_f64()
        .unwrap();

    assert!(
        after > before,
        "absorbance must rise after dispensing: {before} → {after}"
    );
    assert!(
        after >= 0.0196 && after <= 0.0204,
        "absorbance at 2 % fill should be ~0.02 AU ± 2 %: {after}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Sequential dispenses produce monotonically increasing absorbance
// ─────────────────────────────────────────────────────────────────────────────

/// Three successive 1 000 µL dispenses fill the vessel to 2 %, 4 %, 6 %.
/// Each reading must exceed the previous (2 % noise band is far smaller than
/// the 100 % step increase in signal between readings).
#[tokio::test]
#[ignore]
async fn sim_absorbance_monotonically_increases_with_sequential_fills() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let vessel = "sim_mono_003";

    let mut readings = Vec::new();
    for _ in 0..3 {
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &dispense_call(vessel, 1000.0))
            .await
            .expect("dispense must succeed");

        let abs =
            run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
                .await
                .unwrap()["absorbance"]
                .as_f64()
                .unwrap();
        readings.push(abs);
    }

    assert!(
        readings[1] > readings[0],
        "second fill must raise absorbance: {:.5} → {:.5}",
        readings[0],
        readings[1]
    );
    assert!(
        readings[2] > readings[1],
        "third fill must raise absorbance: {:.5} → {:.5}",
        readings[1],
        readings[2]
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Aspirating liquid lowers absorbance
// ─────────────────────────────────────────────────────────────────────────────

/// Fill to ~40 % (20 × 1 000 µL), record absorbance.  Remove half
/// (10 × 1 000 µL, fill drops to ~20 %).  Absorbance must halve.
/// The signal halves so the ±2 % noise band cannot mask the change.
#[tokio::test]
#[ignore]
async fn sim_absorbance_drops_after_aspirate() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let vessel = "sim_aspirate_004";

    for _ in 0..20 {
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &dispense_call(vessel, 1000.0))
            .await
            .expect("dispense must succeed");
    }
    let abs_full =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
            .await
            .unwrap()["absorbance"]
            .as_f64()
            .unwrap();

    for _ in 0..10 {
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &aspirate_call(vessel, 1000.0))
            .await
            .expect("aspirate must succeed");
    }
    let abs_half =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
            .await
            .unwrap()["absorbance"]
            .as_f64()
            .unwrap();

    assert!(
        abs_half < abs_full,
        "absorbance must drop after removing half the liquid: {abs_full:.5} → {abs_half:.5}"
    );
    let ratio = abs_half / abs_full;
    assert!(
        ratio > 0.45 && ratio < 0.55,
        "halving liquid should halve absorbance (ratio={ratio:.3}, expected ~0.5)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Wavelength modulates absorbance (Beer-Lambert spectral response)
// ─────────────────────────────────────────────────────────────────────────────

/// Fill to 50 % and read at three wavelengths.  Gaussian peak at 500 nm:
///   A(500 nm) > A(350 nm) > A(200 nm)
/// At 200 nm: exp(-0.5 × (300/150)²) = exp(-2) ≈ 0.135, ratio within ±10 %.
#[tokio::test]
#[ignore]
async fn sim_wavelength_modulates_absorbance() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let vessel = "sim_wavelength_005";

    // Fill to 50 % (25 × 1 000 µL in a 50 000 µL vessel)
    for _ in 0..25 {
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &dispense_call(vessel, 1000.0))
            .await
            .expect("dispense must succeed");
    }

    let a_500 = run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
        .await
        .unwrap()["absorbance"]
        .as_f64()
        .unwrap();
    let a_350 = run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 350.0))
        .await
        .unwrap()["absorbance"]
        .as_f64()
        .unwrap();
    let a_200 = run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 200.0))
        .await
        .unwrap()["absorbance"]
        .as_f64()
        .unwrap();

    assert!(
        a_500 > a_350,
        "peak wavelength (500 nm) must give higher absorbance than 350 nm: {a_500:.5} vs {a_350:.5}"
    );
    assert!(
        a_350 > a_200,
        "350 nm must give higher absorbance than 200 nm: {a_350:.5} vs {a_200:.5}"
    );
    let ratio = a_200 / a_500;
    assert!(
        ratio > 0.10 && ratio < 0.18,
        "UV-to-peak absorbance ratio should be ~0.135: {ratio:.4}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Overflow rejected by Python server via gRPC error
// ─────────────────────────────────────────────────────────────────────────────

/// plate_well_A1 is pre-registered with max = 300 µL.
/// Dispensing 280 µL succeeds; dispensing 30 more (total 310 > 300) must
/// return a gRPC error propagated from Python `VesselRegistry`.
/// This is the Python physics layer enforcing capacity — distinct from
/// the Rust capability policy check that occurs earlier in the pipeline.
#[tokio::test]
#[ignore]
async fn sim_overflow_rejected_by_python_server() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    // Fill plate_well_A1 to 280 µL (93 % of 300 µL)
    run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &dispense_call("plate_well_A1", 280.0),
    )
    .await
    .expect("first dispense into plate_well_A1 must succeed");

    // Overflow: 280 + 30 = 310 > 300 µL
    let err = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &dispense_call("plate_well_A1", 30.0),
    )
    .await
    .unwrap_err();

    // The failure must be at DISPATCH (Python physics layer), not at an earlier
    // Rust stage (SANDBOX / CAPABILITY / PROOF_POLICY).  The SiLA 2 gRPC error
    // detail travels as a proto-encoded status and may appear base64-encoded in
    // the string — we assert on the stage prefix, not the decoded message body.
    assert!(
        err.starts_with("DISPATCH"),
        "overflow must be rejected by the Python server (DISPATCH stage), not Rust pipeline: {err}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Underflow rejected by Python server via gRPC error
// ─────────────────────────────────────────────────────────────────────────────

/// Aspirating from a vessel that has never been filled must fail.
/// The error must come from the Python `VesselRegistry`, not the Rust pipeline.
#[tokio::test]
#[ignore]
async fn sim_underflow_rejected_by_python_server() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let err = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &aspirate_call("sim_underflow_007", 100.0),
    )
    .await
    .unwrap_err();

    // The failure must be at DISPATCH (Python physics layer).  The SiLA 2 gRPC
    // error detail may appear base64-encoded — we assert on the stage prefix.
    assert!(
        err.starts_with("DISPATCH"),
        "underflow must be rejected by the Python server (DISPATCH stage), not Rust pipeline: {err}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. Partial underflow rejected — cannot aspirate more than is present
// ─────────────────────────────────────────────────────────────────────────────

/// Dispense 400 µL, then try to aspirate 600 µL — must fail at the Python layer.
#[tokio::test]
#[ignore]
async fn sim_partial_underflow_rejected() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let vessel = "sim_partial_under_008";

    run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &dispense_call(vessel, 400.0))
        .await
        .expect("dispense must succeed");

    let err = run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &aspirate_call(vessel, 600.0),
    )
    .await
    .unwrap_err();

    assert!(
        err.starts_with("DISPATCH"),
        "partial underflow must be rejected at the Python physics layer (DISPATCH stage): {err}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. Reservoir: pre-filled at startup, multiple aspirations lower absorbance
// ─────────────────────────────────────────────────────────────────────────────

/// The reservoir is pre-registered at startup with 100 000 µL initial volume.
/// Aspirating a large enough volume (10 × 1 000 µL = 10 000 µL, 5 % fill
/// change) produces a measurable absorbance drop that exceeds the ±2 % noise
/// band on a ~0.15 AU baseline (noise ≈ ±0.003, signal ≈ 0.015 AU).
#[tokio::test]
#[ignore]
async fn sim_aspirate_from_reservoir_lowers_its_absorbance() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let abs_before =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call("reservoir", 500.0))
            .await
            .expect("reading reservoir before aspirate must succeed")["absorbance"]
            .as_f64()
            .unwrap();

    // Remove 10 000 µL from the 100 000 µL reservoir in 1 000 µL chunks.
    // This 5 % fill reduction produces A_drop ≈ 0.015 AU >> ±2 % noise band.
    for _ in 0..10 {
        run_guarded(
            &sandbox,
            &policy,
            &engine,
            &ctx,
            &registry,
            &aspirate_call("reservoir", 1000.0),
        )
        .await
        .expect("aspirating from a pre-filled reservoir must succeed");
    }

    let abs_after =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call("reservoir", 500.0))
            .await
            .expect("reading reservoir after aspirate must succeed")["absorbance"]
            .as_f64()
            .unwrap();

    assert!(
        abs_after < abs_before,
        "reservoir absorbance must decrease after 10 000 µL removed: {abs_before:.6} → {abs_after:.6}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 10. Cross-instrument state coupling: dispense → spectrophotometer confirms
// ─────────────────────────────────────────────────────────────────────────────

/// VesselRegistry is shared between LiquidHandler and Spectrophotometer.
/// A dispense via LiquidHandler must be immediately visible to the
/// Spectrophotometer, and an aspirate must undo it.
///
///   1. Read → baseline (~0.001 AU)
///   2. Dispense 1 000 µL → absorbance rises
///   3. Aspirate the confirmed dispensed volume → absorbance returns to baseline
#[tokio::test]
#[ignore]
async fn sim_cross_instrument_state_coupling() {
    let clients = Arc::new(SiLA2Clients::connect(ADDR).await.expect(ADDR));
    let manifest = load_lab_manifest();
    let ctx = ctx_from_manifest(&manifest);
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_lab_tools(&mut registry, Arc::clone(&clients));

    let vessel = "sim_coupling_010";

    // ── Step 1: baseline ──────────────────────────────────────────────────
    let a_empty =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
            .await
            .unwrap()["absorbance"]
            .as_f64()
            .unwrap();
    assert!(a_empty <= 0.0011, "fresh vessel must read near baseline: {a_empty}");

    // ── Step 2: dispense ──────────────────────────────────────────────────
    let disp_result =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &dispense_call(vessel, 1000.0))
            .await
            .expect("dispense must pass the full safety pipeline");
    let dispensed = disp_result["dispensed_volume_ul"].as_f64().unwrap();
    assert!((dispensed - 1000.0).abs() < 15.0, "dispensed ~1000 µL: {dispensed}");

    // ── Step 3: Spectrophotometer sees the LiquidHandler's state change ───
    let a_filled =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
            .await
            .unwrap()["absorbance"]
            .as_f64()
            .unwrap();
    assert!(
        a_filled > a_empty,
        "absorbance must rise immediately after dispense: {a_empty:.5} → {a_filled:.5}"
    );

    // ── Step 4: aspirate the confirmed volume back ─────────────────────────
    // Clamp to the 1000 µL hardware cap: the ±1% dispense variance can push
    // the confirmed volume just above 1000 µL.  The residual (≤ 10 µL in a
    // 50 000 µL vessel, fill ≤ 0.02 %) produces A < 0.0002 AU which the
    // instrument floor clamps to the 0.001 AU baseline — the Step 5 assertion
    // still holds.
    let safe_aspirate = dispensed.min(1000.0);
    run_guarded(
        &sandbox,
        &policy,
        &engine,
        &ctx,
        &registry,
        &aspirate_call(vessel, safe_aspirate),
    )
    .await
    .expect("aspirating the confirmed dispensed volume must succeed");

    // ── Step 5: absorbance returns to baseline ────────────────────────────
    let a_drained =
        run_guarded(&sandbox, &policy, &engine, &ctx, &registry, &read_call(vessel, 500.0))
            .await
            .unwrap()["absorbance"]
            .as_f64()
            .unwrap();
    assert!(
        a_drained < a_filled,
        "absorbance must fall after draining: {a_filled:.5} → {a_drained:.5}"
    );
    assert!(
        a_drained <= 0.0011,
        "drained vessel must read near baseline again: {a_drained}"
    );
}
