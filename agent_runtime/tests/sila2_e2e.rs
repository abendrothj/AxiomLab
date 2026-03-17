//! End-to-end integration test: full AxiomLab pipeline
//!
//! Validates: LLM generates idea (tool call) → capability policy proves
//! it is within bounds → sandbox allowlist permits it → Rust dispatches
//! gRPC to Python SiLA 2 mock → hardware executes and returns result.
//!
//! Prerequisites: the SiLA 2 mock server must be running on :50052:
//!   cd sila_mock && python -m axiomlab_mock
//!
//! Run: cargo test -p agent_runtime --test sila2_e2e -- --ignored

use std::collections::BTreeMap;
use std::path::PathBuf;
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

/// Build a sandbox that allows the lab tool commands.
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

/// Register tool handlers that delegate to real SiLA 2 gRPC clients.
fn register_sila2_tools(registry: &mut ToolRegistry, clients: Arc<SiLA2Clients>) {
    // ── dispense ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "dispense".into(),
            description: "Dispense liquid via SiLA 2 LiquidHandler".into(),
            parameters_schema: serde_json::json!({}),
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

    // ── move_arm ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "move_arm".into(),
            description: "Move robotic arm via SiLA 2".into(),
            parameters_schema: serde_json::json!({}),
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let x = params["x"].as_f64().ok_or("missing x")?;
                let y = params["y"].as_f64().ok_or("missing y")?;
                let z = params["z"].as_f64().ok_or("missing z")?;
                c.move_arm(x, y, z).await
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

    // ── set_temperature ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "set_temperature".into(),
            description: "Set incubator temperature via SiLA 2".into(),
            parameters_schema: serde_json::json!({}),
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let temp = params["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?;
                c.set_temperature(temp).await
            })
        }),
    );

    // ── spin_centrifuge ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "spin_centrifuge".into(),
            description: "Spin centrifuge via SiLA 2".into(),
            parameters_schema: serde_json::json!({}),
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let rcf = params["rcf"].as_f64().ok_or("missing rcf")?;
                let dur = params["duration_seconds"].as_f64().ok_or("missing duration_seconds")?;
                let temp = params["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?;
                c.spin_centrifuge(rcf, dur, temp).await
            })
        }),
    );

    // ── calibrate_ph ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "calibrate_ph".into(),
            description: "Calibrate pH meter via SiLA 2".into(),
            parameters_schema: serde_json::json!({}),
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let b1 = params["buffer_ph1"].as_f64().ok_or("missing buffer_ph1")?;
                let b2 = params["buffer_ph2"].as_f64().ok_or("missing buffer_ph2")?;
                c.calibrate_ph(b1, b2).await
            })
        }),
    );

    // ── read_ph ──
    let c = clients.clone();
    registry.register(
        ToolSpec {
            name: "read_ph".into(),
            description: "Read pH via SiLA 2".into(),
            parameters_schema: serde_json::json!({}),
        },
        Box::new(move |params| {
            let c = c.clone();
            Box::pin(async move {
                let sample = params["sample_id"].as_str().ok_or("missing sample_id")?;
                c.read_ph(sample).await
            })
        }),
    );
}

// ── Full pipeline: sandbox ✓ → capability ✓ → gRPC → response ──

/// Simulate the orchestrator pipeline for a single tool call:
/// 1. Sandbox allowlist check
/// 2. Capability bounds validation
/// 3. Tool dispatch (gRPC to Python SiLA 2 mock)
/// 4. Response assertion
async fn run_pipeline(
    sandbox: &Sandbox,
    policy: &CapabilityPolicy,
    registry: &ToolRegistry,
    call: &ToolCall,
) -> Result<serde_json::Value, String> {
    // Stage 1: sandbox allowlist
    sandbox
        .check_command(&call.name)
        .map_err(|e| format!("SANDBOX: {e}"))?;

    // Stage 2: capability bounds
    policy
        .validate(&call.name, &call.params)
        .map_err(|e| format!("CAPABILITY: {e}"))?;

    // Stage 3: dispatch via gRPC
    let result = registry.dispatch(call).await;
    if result.success {
        Ok(result.output)
    } else {
        Err(format!("DISPATCH: {}", result.output))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Tests — require SiLA 2 mock: cd sila_mock && python -m axiomlab_mock
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore] // requires running SiLA 2 mock on :50052
async fn e2e_dispense_through_pipeline() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, clients);

    // Simulate LLM-generated tool call: dispense 250 µL into well A1
    let call = ToolCall {
        name: "dispense".into(),
        params: serde_json::json!({
            "pump_id": "well_A1",
            "volume_ul": 250.0,
        }),
    };

    let result = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .expect("pipeline should succeed");

    // Mock simulates pipetting noise, so volume won't be exact
    let dispensed = result["dispensed_volume_ul"].as_f64().unwrap();
    assert!((dispensed - 250.0).abs() < 5.0, "dispensed volume ~250: {dispensed}");
    assert_eq!(result["target_vessel"], "well_A1");
}

#[tokio::test]
#[ignore]
async fn e2e_move_arm_through_pipeline() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, clients);

    let call = ToolCall {
        name: "move_arm".into(),
        params: serde_json::json!({ "x": 100.0, "y": 150.0, "z": 80.0 }),
    };

    let result = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .expect("pipeline should succeed");

    assert_eq!(result["reached_x"], 100.0);
    assert_eq!(result["reached_y"], 150.0);
    assert_eq!(result["reached_z"], 80.0);
}

#[tokio::test]
#[ignore]
async fn e2e_read_absorbance_through_pipeline() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, clients);

    let call = ToolCall {
        name: "read_absorbance".into(),
        params: serde_json::json!({
            "vessel_id": "cuvette_1",
            "wavelength_nm": 450.0,
        }),
    };

    let result = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .expect("pipeline should succeed");

    let abs = result["absorbance"].as_f64().unwrap();
    assert!(abs > 0.0 && abs < 5.0, "absorbance should be realistic: {abs}");
    assert_eq!(result["wavelength_nm"], 450.0);
}

#[tokio::test]
#[ignore]
async fn e2e_spin_centrifuge_through_pipeline() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, clients);

    let call = ToolCall {
        name: "spin_centrifuge".into(),
        params: serde_json::json!({
            "rcf": 1000.0,
            "duration_seconds": 30.0,
            "temperature_celsius": 20.0,
        }),
    };

    let result = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .expect("pipeline should succeed");

    let actual_rcf = result["actual_rcf"].as_f64().unwrap();
    assert!(actual_rcf > 900.0 && actual_rcf < 1100.0, "RCF should be ~1000: {actual_rcf}");
}

#[tokio::test]
#[ignore]
async fn e2e_ph_calibrate_then_read() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, clients);

    // Step 1: calibrate
    let cal = ToolCall {
        name: "calibrate_ph".into(),
        params: serde_json::json!({ "buffer_ph1": 4.0, "buffer_ph2": 7.0 }),
    };
    let cal_result = run_pipeline(&sandbox, &policy, &registry, &cal)
        .await
        .expect("calibration should succeed");
    let status = cal_result["calibration_status"].as_str().unwrap();
    assert!(
        status == "OK" || status == "Success",
        "calibration status should indicate success: {status}"
    );

    // Step 2: read
    let read = ToolCall {
        name: "read_ph".into(),
        params: serde_json::json!({ "sample_id": "sample_42" }),
    };
    let ph_result = run_pipeline(&sandbox, &policy, &registry, &read)
        .await
        .expect("pH read should succeed after calibration");

    let ph_val = ph_result["ph_value"].as_f64().unwrap();
    assert!(ph_val > 0.0 && ph_val < 14.0, "pH should be in [0,14]: {ph_val}");
}

// ═══════════════════════════════════════════════════════════════════
// Rejection tests — these do NOT need the SiLA 2 mock (pure Rust)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn capability_rejects_out_of_bounds_dispense() {
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new(); // empty — should never reach dispatch

    // Volume exceeds max (1000 µL)
    let call = ToolCall {
        name: "dispense".into(),
        params: serde_json::json!({ "pump_id": "A1", "volume_ul": 5000.0 }),
    };

    let err = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability violation"),
        "should fail capability bounds: {err}"
    );
}

#[tokio::test]
async fn capability_rejects_out_of_bounds_move_arm() {
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    let call = ToolCall {
        name: "move_arm".into(),
        params: serde_json::json!({ "x": 999.0, "y": 10.0, "z": 10.0 }),
    };

    let err = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability violation"),
        "should fail capability bounds: {err}"
    );
}

#[tokio::test]
async fn sandbox_rejects_disallowed_command() {
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    // "rm" is not in the sandbox allowlist
    let call = ToolCall {
        name: "rm".into(),
        params: serde_json::json!({ "path": "/etc/passwd" }),
    };

    let err = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .unwrap_err();
    assert!(
        err.contains("SANDBOX"),
        "should fail sandbox check: {err}"
    );
}

#[tokio::test]
async fn capability_rejects_negative_volume() {
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    let call = ToolCall {
        name: "dispense".into(),
        params: serde_json::json!({ "pump_id": "A1", "volume_ul": -10.0 }),
    };

    let err = run_pipeline(&sandbox, &policy, &registry, &call)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability violation"),
        "negative volume should fail: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Proof policy tests — validates Verus artifact gating (pure Rust)
// ═══════════════════════════════════════════════════════════════════

fn test_execution_context() -> ExecutionContext {
    ExecutionContext {
        git_commit: "test123".into(),
        binary_hash: "bin123".into(),
        container_image_digest: None,
        device_id: None,
        firmware_version: None,
    }
}

fn test_manifest(verus_status: ArtifactStatus) -> ProofManifest {
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
                action: "move_arm".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Arm actuation requires Verus safety proof".into(),
            },
            ActionPolicy {
                action: "dispense".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Liquid handling requires volume proof".into(),
            },
            ActionPolicy {
                action: "read_absorbance".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only, no proof needed".into(),
            },
        ],
    }
}

/// Full pipeline including proof policy:
/// sandbox → capability → proof policy → gRPC dispatch
async fn run_full_pipeline(
    sandbox: &Sandbox,
    policy: &CapabilityPolicy,
    engine: &RuntimePolicyEngine,
    ctx: &ExecutionContext,
    registry: &ToolRegistry,
    call: &ToolCall,
) -> Result<serde_json::Value, String> {
    // Stage 1: sandbox
    sandbox
        .check_command(&call.name)
        .map_err(|e| format!("SANDBOX: {e}"))?;

    // Stage 2: capability bounds
    policy
        .validate(&call.name, &call.params)
        .map_err(|e| format!("CAPABILITY: {e}"))?;

    // Stage 3: proof policy
    engine
        .authorize(&call.name, ctx)
        .map_err(|e| format!("PROOF_POLICY: {e}"))?;

    // Stage 4: dispatch
    let result = registry.dispatch(call).await;
    if result.success {
        Ok(result.output)
    } else {
        Err(format!("DISPATCH: {}", result.output))
    }
}

#[tokio::test]
async fn proof_policy_allows_read_only_without_verus() {
    // Even with failed Verus artifacts, read-only actions should pass
    let engine = RuntimePolicyEngine::new(test_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = test_execution_context();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    let call = ToolCall {
        name: "read_absorbance".into(),
        params: serde_json::json!({"vessel_id": "c1", "wavelength_nm": 450.0}),
    };

    // Should pass proof policy (read-only, no artifacts required)
    // Will fail at dispatch (no handler), but that's expected
    let result = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call).await;
    // Dispatch fails because registry is empty, but proof policy passed
    assert!(
        result.is_err() && result.as_ref().unwrap_err().contains("DISPATCH"),
        "should pass proof policy but fail dispatch: {result:?}"
    );
}

#[tokio::test]
async fn proof_policy_blocks_actuation_with_failed_verus() {
    let engine = RuntimePolicyEngine::new(test_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = test_execution_context();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    let call = ToolCall {
        name: "move_arm".into(),
        params: serde_json::json!({"x": 100.0, "y": 100.0, "z": 50.0}),
    };

    let err = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .unwrap_err();
    assert!(
        err.contains("PROOF_POLICY"),
        "actuation should be blocked by failed Verus artifact: {err}"
    );
}

#[tokio::test]
async fn proof_policy_blocks_unsigned_manifest() {
    // Engine without mark_signature_verified should reject everything
    let engine = RuntimePolicyEngine::new(test_manifest(ArtifactStatus::Passed));
    let ctx = test_execution_context();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let registry = ToolRegistry::new();

    let call = ToolCall {
        name: "read_absorbance".into(),
        params: serde_json::json!({"vessel_id": "c1", "wavelength_nm": 450.0}),
    };

    let err = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .unwrap_err();
    assert!(
        err.contains("PROOF_POLICY") && err.contains("signature"),
        "unsigned manifest should block all actions: {err}"
    );
}

#[tokio::test]
#[ignore] // requires running SiLA 2 mock on :50052
async fn e2e_full_pipeline_with_proof_policy() {
    // Full pipeline: sandbox → capability → proof (passed) → gRPC → response
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let engine = RuntimePolicyEngine::new(test_manifest(ArtifactStatus::Passed))
        .mark_signature_verified();
    let ctx = test_execution_context();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, Arc::clone(&clients));

    // Actuation: move_arm — requires Verus proof, which is Passed
    let call = ToolCall {
        name: "move_arm".into(),
        params: serde_json::json!({"x": 50.0, "y": 60.0, "z": 70.0}),
    };
    let result = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .expect("full pipeline should succeed with passed Verus artifacts");
    assert_eq!(result["reached_x"], 50.0);
    assert_eq!(result["reached_y"], 60.0);
    assert_eq!(result["reached_z"], 70.0);

    // Liquid handling: dispense — also requires Verus proof
    let call = ToolCall {
        name: "dispense".into(),
        params: serde_json::json!({"pump_id": "well_B2", "volume_ul": 100.0}),
    };
    let result = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .expect("dispense should succeed with passed Verus artifacts");
    let dispensed = result["dispensed_volume_ul"].as_f64().unwrap();
    assert!((dispensed - 100.0).abs() < 5.0, "dispensed ~100: {dispensed}");

    // Read-only: absorbance — no proof needed
    let call = ToolCall {
        name: "read_absorbance".into(),
        params: serde_json::json!({"vessel_id": "cuvette_3", "wavelength_nm": 600.0}),
    };
    let result = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .expect("read-only should succeed without proof requirement");
    let abs = result["absorbance"].as_f64().unwrap();
    assert!(abs > 0.0 && abs < 5.0, "absorbance realistic: {abs}");
}

#[tokio::test]
#[ignore] // requires running SiLA 2 mock on :50052
async fn e2e_proof_policy_blocks_actuation_but_allows_reads() {
    // Failed Verus: actuation blocked, reads still work through gRPC
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );
    let engine = RuntimePolicyEngine::new(test_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = test_execution_context();
    let sandbox = lab_sandbox();
    let policy = CapabilityPolicy::default_lab();
    let mut registry = ToolRegistry::new();
    register_sila2_tools(&mut registry, Arc::clone(&clients));

    // Actuation: should be BLOCKED
    let call = ToolCall {
        name: "move_arm".into(),
        params: serde_json::json!({"x": 50.0, "y": 60.0, "z": 70.0}),
    };
    let err = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .unwrap_err();
    assert!(err.contains("PROOF_POLICY"), "actuation blocked: {err}");

    // Read-only: should PASS through gRPC
    let call = ToolCall {
        name: "read_absorbance".into(),
        params: serde_json::json!({"vessel_id": "cuvette_3", "wavelength_nm": 550.0}),
    };
    let result = run_full_pipeline(&sandbox, &policy, &engine, &ctx, &registry, &call)
        .await
        .expect("read-only should still work even with failed Verus");
    let abs = result["absorbance"].as_f64().unwrap();
    assert!(abs > 0.0 && abs < 5.0, "absorbance realistic: {abs}");
}
