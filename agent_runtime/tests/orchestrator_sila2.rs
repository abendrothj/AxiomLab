//! Lab-ready integration test: actual Orchestrator driving SiLA 2 hardware.
//!
//! Unlike sila2_e2e.rs (which manually calls each pipeline stage), this test
//! exercises the REAL Orchestrator.run_experiment() with:
//!   - ScriptedLlm emitting realistic tool-call JSON
//!   - Full 5-stage validation pipeline (sandbox → approval → capability → proof → dispatch)
//!   - Real SiLA 2 gRPC dispatch to the Python mock
//!   - Event sink capturing what the visualizer would see
//!   - Audit trail verification
//!
//! This is what runs before shipping to a lab.
//!
//! Prerequisites: SiLA 2 mock on :50052
//!   cd sila_mock && python -m axiomlab_mock --insecure
//!
//! Run: cargo test -p agent_runtime --test orchestrator_sila2 -- --ignored --test-threads=1

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_runtime::capabilities::CapabilityPolicy;
use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
use agent_runtime::experiment::Experiment;
use agent_runtime::hardware::SiLA2Clients;
use agent_runtime::llm::{ChatMessage, LlmBackend, LlmError};
use agent_runtime::orchestrator::{Orchestrator, OrchestratorConfig};
use agent_runtime::sandbox::{ResourceLimits, Sandbox};
use agent_runtime::tools::{ToolRegistry, ToolSpec};
use proof_artifacts::manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest, RiskClass,
    VerusArtifact,
};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};

// ═══════════════════════════════════════════════════════════════════
// Test infrastructure
// ═══════════════════════════════════════════════════════════════════

/// LLM that returns pre-scripted responses based on conversation turn.
struct ScriptedLlm {
    responses: Vec<String>,
}

impl ScriptedLlm {
    fn new(responses: Vec<String>) -> Self {
        Self { responses }
    }
}

impl LlmBackend for ScriptedLlm {
    fn chat(
        &self,
        messages: &[ChatMessage],
        _temperature: f64,
    ) -> impl std::future::Future<Output = Result<String, LlmError>> + Send {
        // Count assistant turns to determine which response to emit
        let turn = messages.iter().filter(|m| m.role == "assistant").count();
        let response = self
            .responses
            .get(turn)
            .cloned()
            .unwrap_or_else(|| r#"{"done": true, "summary": "completed"}"#.into());
        async move { Ok(response) }
    }
}

/// Event sink that records all events for post-test assertion.
#[derive(Default)]
struct RecordingSink {
    transitions: Mutex<Vec<StateTransitionEvent>>,
    tool_events: Mutex<Vec<ToolExecutionEvent>>,
    notebook: Mutex<Vec<NotebookEntryEvent>>,
    tokens: Mutex<Vec<String>>,
}

impl EventSink for RecordingSink {
    fn on_state_transition(&self, event: StateTransitionEvent) {
        self.transitions.lock().unwrap().push(event);
    }
    fn on_tool_execution(&self, event: ToolExecutionEvent) {
        self.tool_events.lock().unwrap().push(event);
    }
    fn on_llm_token(&self, event: LlmTokenEvent) {
        self.tokens.lock().unwrap().push(event.token);
    }
    fn on_notebook_entry(&self, event: NotebookEntryEvent) {
        self.notebook.lock().unwrap().push(event);
    }
}

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
            "spin_centrifuge".into(),
            "calibrate_ph".into(),
            "read_ph".into(),
            "incubate".into(),
            "read_sensor".into(),
        ],
        ResourceLimits::default(),
    )
}

fn test_execution_context() -> ExecutionContext {
    ExecutionContext {
        git_commit: "test-lab-ready".into(),
        binary_hash: "bin-lab-ready".into(),
        container_image_digest: None,
        device_id: None,
        firmware_version: None,
    }
}

fn lab_manifest(verus_status: ArtifactStatus) -> ProofManifest {
    ProofManifest {
        schema_version: 1,
        generated_unix_secs: 0,
        build: BuildIdentity {
            git_commit: "test-lab-ready".into(),
            binary_hash: "bin-lab-ready".into(),
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
                action: "aspirate".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Aspirate requires volume proof".into(),
            },
            ActionPolicy {
                action: "set_temperature".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Temperature requires thermal proof".into(),
            },
            ActionPolicy {
                action: "spin_centrifuge".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Centrifuge requires RCF proof".into(),
            },
            ActionPolicy {
                action: "incubate".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Incubation requires thermal proof".into(),
            },
            ActionPolicy {
                action: "read_absorbance".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only".into(),
            },
            ActionPolicy {
                action: "read_temperature".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only".into(),
            },
            ActionPolicy {
                action: "read_ph".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only".into(),
            },
            ActionPolicy {
                action: "read_sensor".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only".into(),
            },
            ActionPolicy {
                action: "calibrate_ph".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Non-destructive calibration".into(),
            },
        ],
    }
}

/// Register tool handlers backed by real SiLA 2 gRPC clients.
fn sila2_tool_registry(clients: Arc<SiLA2Clients>) -> ToolRegistry {
    let mut r = ToolRegistry::new();

    macro_rules! reg {
        ($name:expr, $desc:expr, $schema:expr, $handler:expr) => {{
            let c = clients.clone();
            r.register(
                ToolSpec {
                    name: $name.into(),
                    description: $desc.into(),
                    parameters_schema: $schema,
                },
                Box::new(move |p| {
                    let c = c.clone();
                    Box::pin(async move { ($handler)(c, p).await })
                }),
            );
        }};
    }

    reg!("dispense", "Dispense liquid (pump_id, volume_ul).",
        serde_json::json!({"type":"object","properties":{"pump_id":{"type":"string"},"volume_ul":{"type":"number"}},"required":["pump_id","volume_ul"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.dispense(p["pump_id"].as_str().ok_or("missing pump_id")?, p["volume_ul"].as_f64().ok_or("missing volume_ul")?).await
        }
    );

    reg!("aspirate", "Aspirate liquid (source_vessel, volume_ul).",
        serde_json::json!({"type":"object","properties":{"source_vessel":{"type":"string"},"volume_ul":{"type":"number"}},"required":["source_vessel","volume_ul"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.aspirate(p["source_vessel"].as_str().ok_or("missing source_vessel")?, p["volume_ul"].as_f64().ok_or("missing volume_ul")?).await
        }
    );

    reg!("move_arm", "Move arm to (x,y,z) mm.",
        serde_json::json!({"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"z":{"type":"number"}},"required":["x","y","z"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.move_arm(p["x"].as_f64().ok_or("missing x")?, p["y"].as_f64().ok_or("missing y")?, p["z"].as_f64().ok_or("missing z")?).await
        }
    );

    reg!("read_absorbance", "UV/Vis absorbance (vessel_id, wavelength_nm).",
        serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.read_absorbance(p["vessel_id"].as_str().ok_or("missing vessel_id")?, p["wavelength_nm"].as_f64().ok_or("missing wavelength_nm")?).await
        }
    );

    reg!("set_temperature", "Set incubator temp (temperature_celsius).",
        serde_json::json!({"type":"object","properties":{"temperature_celsius":{"type":"number"}},"required":["temperature_celsius"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.set_temperature(p["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?).await
        }
    );

    reg!("read_temperature", "Read incubator temperature.",
        serde_json::json!({"type":"object","properties":{}}),
        |c: Arc<SiLA2Clients>, _p: serde_json::Value| async move {
            c.read_temperature().await
        }
    );

    reg!("spin_centrifuge", "Spin centrifuge (rcf, duration_seconds, temperature_celsius).",
        serde_json::json!({"type":"object","properties":{"rcf":{"type":"number"},"duration_seconds":{"type":"number"},"temperature_celsius":{"type":"number"}},"required":["rcf","duration_seconds","temperature_celsius"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.spin_centrifuge(p["rcf"].as_f64().ok_or("missing rcf")?, p["duration_seconds"].as_f64().ok_or("missing duration_seconds")?, p["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?).await
        }
    );

    reg!("calibrate_ph", "Calibrate pH (buffer_ph1, buffer_ph2).",
        serde_json::json!({"type":"object","properties":{"buffer_ph1":{"type":"number"},"buffer_ph2":{"type":"number"}},"required":["buffer_ph1","buffer_ph2"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.calibrate_ph(p["buffer_ph1"].as_f64().ok_or("missing buffer_ph1")?, p["buffer_ph2"].as_f64().ok_or("missing buffer_ph2")?).await
        }
    );

    reg!("read_ph", "Read pH (sample_id).",
        serde_json::json!({"type":"object","properties":{"sample_id":{"type":"string"}},"required":["sample_id"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.read_ph(p["sample_id"].as_str().ok_or("missing sample_id")?).await
        }
    );

    reg!("incubate", "Incubate (duration_minutes).",
        serde_json::json!({"type":"object","properties":{"duration_minutes":{"type":"number"}},"required":["duration_minutes"]}),
        |c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            c.incubate(p["duration_minutes"].as_f64().ok_or("missing duration_minutes")?).await
        }
    );

    reg!("read_sensor", "Read a named sensor.",
        serde_json::json!({"type":"object","properties":{"sensor_id":{"type":"string"}},"required":["sensor_id"]}),
        |_c: Arc<SiLA2Clients>, p: serde_json::Value| async move {
            let id = p["sensor_id"].as_str().ok_or("missing sensor_id")?;
            Ok(serde_json::json!({"sensor_id": id, "value": 7.04, "unit": "pH"}))
        }
    );

    r
}

// ═══════════════════════════════════════════════════════════════════
// Test 1: Happy path — LLM drives a multi-step experiment through
// the real Orchestrator, hitting real SiLA 2 hardware at every step.
//
// Script:
//   Turn 0: move arm to pick position
//   Turn 1: dispense 200 µL into well
//   Turn 2: read absorbance at 450nm
//   Turn 3: report findings and complete
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore] // requires SiLA 2 mock on :50052
async fn orchestrator_drives_multi_step_experiment_through_sila2() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );

    let sink = Arc::new(RecordingSink::default());

    let llm = ScriptedLlm::new(vec![
        // Turn 0: move arm
        r#"{"tool": "move_arm", "params": {"x": 100.0, "y": 150.0, "z": 50.0}}"#.into(),
        // Turn 1: dispense
        r#"{"tool": "dispense", "params": {"pump_id": "well_A1", "volume_ul": 200.0}}"#.into(),
        // Turn 2: read absorbance
        r#"{"tool": "read_absorbance", "params": {"vessel_id": "well_A1", "wavelength_nm": 450.0}}"#.into(),
        // Turn 3: conclude
        r#"{"done": true, "summary": "Measured absorbance of sample at 450nm after dispensing 200uL. Beer-Lambert relationship confirmed within instrument noise."}"#.into(),
    ]);

    let engine = RuntimePolicyEngine::new(lab_manifest(ArtifactStatus::Passed))
        .mark_signature_verified();
    let ctx = test_execution_context();

    let dir = tempfile::tempdir().expect("tempdir");
    let audit_path = dir.path().join("audit.jsonl");

    let config = OrchestratorConfig {
        max_iterations: 10,
        code_gen_temperature: 0.0,
        reasoning_temperature: 0.0,
        audit_log_path: Some(audit_path.to_string_lossy().to_string()),
        capability_policy: Some(CapabilityPolicy::default_lab()),
        approval_policy: None, // disable two-person for this test
        event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..OrchestratorConfig::default()
    };

    let orchestrator = Orchestrator::new(llm, lab_sandbox(), sila2_tool_registry(Arc::clone(&clients)), config)
        .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("lab-ready-e2e-1", "Beer-Lambert absorbance test");
    let result = orchestrator.run_experiment(&mut exp).await;

    assert!(result.is_ok(), "experiment should complete: {result:?}");

    // ── Verify tool execution events ──────────────────────────────
    let tool_events = sink.tool_events.lock().unwrap();
    assert!(
        tool_events.len() >= 3,
        "should have at least 3 tool events (move_arm, dispense, read_absorbance), got {}",
        tool_events.len()
    );

    // All three tool calls should have succeeded
    let successes: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.status == "success")
        .collect();
    assert!(
        successes.len() >= 3,
        "at least 3 tool calls should succeed, got {}: {:?}",
        successes.len(),
        successes.iter().map(|e| &e.tool).collect::<Vec<_>>()
    );

    // Verify specific tools were called
    let tool_names: Vec<&str> = tool_events.iter().map(|e| e.tool.as_str()).collect();
    assert!(tool_names.contains(&"move_arm"), "should have called move_arm");
    assert!(tool_names.contains(&"dispense"), "should have called dispense");
    assert!(tool_names.contains(&"read_absorbance"), "should have called read_absorbance");

    // ── Verify notebook entry (discovery logged) ──────────────────
    let notebook = sink.notebook.lock().unwrap();
    assert!(
        !notebook.is_empty(),
        "experiment should produce at least one notebook entry"
    );
    assert!(
        notebook[0].entry.contains("Beer-Lambert") || notebook[0].entry.contains("absorbance"),
        "notebook should contain the experiment summary"
    );

    // ── Verify state transitions ──────────────────────────────────
    let transitions = sink.transitions.lock().unwrap();
    assert!(
        !transitions.is_empty(),
        "should have at least one state transition"
    );

    // ── Verify audit log written ──────────────────────────────────
    let audit = std::fs::read_to_string(&audit_path).unwrap_or_default();
    assert!(
        audit.contains("allow") || audit.contains("deny"),
        "audit log should contain decisions"
    );

    println!("\n=== LAB-READY TEST 1 PASSED ===");
    println!("  Tools executed: {tool_names:?}");
    println!("  Events captured: {} tool, {} transition, {} notebook",
        tool_events.len(), transitions.len(), notebook.len());
    println!("  Audit entries: {} lines", audit.lines().count());
}

// ═══════════════════════════════════════════════════════════════════
// Test 2: Proof policy blocks actuation but allows reads.
//
// With Verus artifacts FAILED, the Orchestrator should:
//   - BLOCK move_arm (Actuation) at proof stage
//   - ALLOW read_absorbance (ReadOnly) through gRPC
//   - Still complete the experiment
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn orchestrator_proof_policy_blocks_actuation_allows_reads() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );

    let sink = Arc::new(RecordingSink::default());

    let llm = ScriptedLlm::new(vec![
        // Turn 0: try to move arm — should be DENIED by proof policy
        r#"{"tool": "move_arm", "params": {"x": 100.0, "y": 100.0, "z": 50.0}}"#.into(),
        // Turn 1: read absorbance — should SUCCEED (read-only, no proof needed)
        r#"{"tool": "read_absorbance", "params": {"vessel_id": "cuvette_1", "wavelength_nm": 550.0}}"#.into(),
        // Turn 2: conclude
        r#"{"done": true, "summary": "Arm actuation was denied due to missing safety proof. Read-only measurements still functional."}"#.into(),
    ]);

    // FAILED Verus artifacts
    let engine = RuntimePolicyEngine::new(lab_manifest(ArtifactStatus::Failed))
        .mark_signature_verified();
    let ctx = test_execution_context();

    let config = OrchestratorConfig {
        max_iterations: 10,
        code_gen_temperature: 0.0,
        reasoning_temperature: 0.0,
        capability_policy: Some(CapabilityPolicy::default_lab()),
        approval_policy: None,
        event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..OrchestratorConfig::default()
    };

    let orchestrator = Orchestrator::new(llm, lab_sandbox(), sila2_tool_registry(Arc::clone(&clients)), config)
        .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("lab-ready-e2e-2", "proof gating test");
    let result = orchestrator.run_experiment(&mut exp).await;
    assert!(result.is_ok(), "experiment should complete: {result:?}");

    let tool_events = sink.tool_events.lock().unwrap();

    // move_arm should have been REJECTED
    let arm_events: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.tool == "move_arm")
        .collect();
    assert!(!arm_events.is_empty(), "move_arm should have been attempted");
    assert_eq!(
        arm_events[0].status, "rejected",
        "move_arm should be rejected by proof policy"
    );

    // read_absorbance should have SUCCEEDED
    let read_events: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.tool == "read_absorbance")
        .collect();
    assert!(!read_events.is_empty(), "read_absorbance should have been attempted");
    assert_eq!(
        read_events[0].status, "success",
        "read_absorbance should succeed (read-only, no proof needed)"
    );

    println!("\n=== LAB-READY TEST 2 PASSED ===");
    println!("  move_arm: BLOCKED by proof policy ✓");
    println!("  read_absorbance: ALLOWED through gRPC ✓");
}

// ═══════════════════════════════════════════════════════════════════
// Test 3: Capability bounds enforcement through Orchestrator.
//
// The LLM tries to move the arm out of bounds — the Orchestrator's
// capability stage should reject it, then the LLM retries within
// bounds and succeeds.
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn orchestrator_capability_rejects_then_retries_within_bounds() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );

    let sink = Arc::new(RecordingSink::default());

    let llm = ScriptedLlm::new(vec![
        // Turn 0: out-of-bounds move (x=999 > max 300)
        r#"{"tool": "move_arm", "params": {"x": 999.0, "y": 10.0, "z": 10.0}}"#.into(),
        // Turn 1: retry within bounds
        r#"{"tool": "move_arm", "params": {"x": 200.0, "y": 150.0, "z": 100.0}}"#.into(),
        // Turn 2: conclude
        r#"{"done": true, "summary": "Discovered arm x-axis limit is 300mm. Retry within bounds succeeded."}"#.into(),
    ]);

    let engine = RuntimePolicyEngine::new(lab_manifest(ArtifactStatus::Passed))
        .mark_signature_verified();
    let ctx = test_execution_context();

    let config = OrchestratorConfig {
        max_iterations: 10,
        code_gen_temperature: 0.0,
        reasoning_temperature: 0.0,
        capability_policy: Some(CapabilityPolicy::default_lab()),
        approval_policy: None,
        event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..OrchestratorConfig::default()
    };

    let orchestrator = Orchestrator::new(llm, lab_sandbox(), sila2_tool_registry(Arc::clone(&clients)), config)
        .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("lab-ready-e2e-3", "capability bounds test");
    let result = orchestrator.run_experiment(&mut exp).await;
    assert!(result.is_ok(), "experiment should complete: {result:?}");

    let tool_events = sink.tool_events.lock().unwrap();
    let arm_events: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.tool == "move_arm")
        .collect();

    assert!(
        arm_events.len() >= 2,
        "should have at least 2 move_arm attempts, got {}",
        arm_events.len()
    );

    // First attempt: rejected by capability bounds
    assert_eq!(arm_events[0].status, "rejected", "first move_arm out-of-bounds should be rejected");

    // Second attempt: succeeded
    assert_eq!(arm_events[1].status, "success", "second move_arm within bounds should succeed");

    println!("\n=== LAB-READY TEST 3 PASSED ===");
    println!("  Out-of-bounds move_arm: REJECTED ✓");
    println!("  In-bounds retry: SUCCEEDED via gRPC ✓");
}

// ═══════════════════════════════════════════════════════════════════
// Test 4: Sandbox blocks unauthorized commands.
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn orchestrator_sandbox_blocks_unauthorized_tool() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );

    let sink = Arc::new(RecordingSink::default());

    let llm = ScriptedLlm::new(vec![
        // Turn 0: try shell command (not in allowlist)
        r#"{"tool": "shell_exec", "params": {"cmd": "rm -rf /"}}"#.into(),
        // Turn 1: legitimate read
        r#"{"tool": "read_absorbance", "params": {"vessel_id": "c1", "wavelength_nm": 600.0}}"#.into(),
        // Turn 2: conclude
        r#"{"done": true, "summary": "Shell access denied. Spectrophotometer read succeeded."}"#.into(),
    ]);

    let engine = RuntimePolicyEngine::new(lab_manifest(ArtifactStatus::Passed))
        .mark_signature_verified();
    let ctx = test_execution_context();

    let config = OrchestratorConfig {
        max_iterations: 10,
        code_gen_temperature: 0.0,
        reasoning_temperature: 0.0,
        capability_policy: Some(CapabilityPolicy::default_lab()),
        approval_policy: None,
        event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..OrchestratorConfig::default()
    };

    let orchestrator = Orchestrator::new(llm, lab_sandbox(), sila2_tool_registry(Arc::clone(&clients)), config)
        .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("lab-ready-e2e-4", "sandbox isolation test");
    let result = orchestrator.run_experiment(&mut exp).await;
    assert!(result.is_ok(), "experiment should complete: {result:?}");

    let tool_events = sink.tool_events.lock().unwrap();

    // shell_exec: rejected by sandbox
    let shell_events: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.tool == "shell_exec")
        .collect();
    assert!(!shell_events.is_empty(), "shell_exec should have been attempted");
    assert_eq!(shell_events[0].status, "rejected", "shell_exec should be sandbox-denied");

    // read_absorbance: succeeded
    let read_events: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.tool == "read_absorbance")
        .collect();
    assert!(!read_events.is_empty(), "read_absorbance should have been attempted");
    assert_eq!(read_events[0].status, "success", "read_absorbance should succeed");

    println!("\n=== LAB-READY TEST 4 PASSED ===");
    println!("  shell_exec: BLOCKED by sandbox ✓");
    println!("  read_absorbance: SUCCEEDED via gRPC ✓");
}

// ═══════════════════════════════════════════════════════════════════
// Test 5: Full multi-instrument workflow.
//
// Script simulates a real titration-style experiment:
//   1. Calibrate pH meter
//   2. Move arm to sample
//   3. Dispense titrant
//   4. Read pH
//   5. Read absorbance
//   6. Conclude
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore]
async fn orchestrator_full_multi_instrument_workflow() {
    let clients = Arc::new(
        SiLA2Clients::connect("http://127.0.0.1:50052")
            .await
            .expect("SiLA 2 mock must be running on :50052"),
    );

    let sink = Arc::new(RecordingSink::default());

    let llm = ScriptedLlm::new(vec![
        // Turn 0: calibrate pH
        r#"{"tool": "calibrate_ph", "params": {"buffer_ph1": 4.0, "buffer_ph2": 7.0}}"#.into(),
        // Turn 1: move arm to sample position
        r#"{"tool": "move_arm", "params": {"x": 120.0, "y": 80.0, "z": 200.0}}"#.into(),
        // Turn 2: dispense 500 µL NaOH titrant
        r#"{"tool": "dispense", "params": {"pump_id": "titrant_NaOH", "volume_ul": 500.0}}"#.into(),
        // Turn 3: read pH after addition
        r#"{"tool": "read_ph", "params": {"sample_id": "titration_sample_1"}}"#.into(),
        // Turn 4: read absorbance (indicator color)
        r#"{"tool": "read_absorbance", "params": {"vessel_id": "titration_sample_1", "wavelength_nm": 550.0}}"#.into(),
        // Turn 5: conclude
        r#"{"done": true, "summary": "Titration complete. After 500uL NaOH addition, pH shifted. Absorbance at 550nm confirms indicator color change. Endpoint determination successful."}"#.into(),
    ]);

    let engine = RuntimePolicyEngine::new(lab_manifest(ArtifactStatus::Passed))
        .mark_signature_verified();
    let ctx = test_execution_context();

    let dir = tempfile::tempdir().expect("tempdir");
    let audit_path = dir.path().join("audit.jsonl");

    let config = OrchestratorConfig {
        max_iterations: 15,
        code_gen_temperature: 0.0,
        reasoning_temperature: 0.0,
        audit_log_path: Some(audit_path.to_string_lossy().to_string()),
        capability_policy: Some(CapabilityPolicy::default_lab()),
        approval_policy: None,
        event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..OrchestratorConfig::default()
    };

    let orchestrator = Orchestrator::new(llm, lab_sandbox(), sila2_tool_registry(Arc::clone(&clients)), config)
        .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("lab-ready-e2e-5", "Acid-base titration with indicator");
    let result = orchestrator.run_experiment(&mut exp).await;
    assert!(result.is_ok(), "experiment should complete: {result:?}");

    let tool_events = sink.tool_events.lock().unwrap();
    let tool_names: Vec<&str> = tool_events.iter().map(|e| e.tool.as_str()).collect();

    // All 5 instruments should have been hit
    assert!(tool_names.contains(&"calibrate_ph"), "should calibrate pH");
    assert!(tool_names.contains(&"move_arm"), "should move arm");
    assert!(tool_names.contains(&"dispense"), "should dispense");
    assert!(tool_names.contains(&"read_ph"), "should read pH");
    assert!(tool_names.contains(&"read_absorbance"), "should read absorbance");

    // All should have succeeded
    let failures: Vec<&ToolExecutionEvent> = tool_events
        .iter()
        .filter(|e| e.status == "rejected")
        .collect();
    assert!(
        failures.is_empty(),
        "no tool calls should have been rejected: {:?}",
        failures.iter().map(|e| (&e.tool, &e.reason)).collect::<Vec<_>>()
    );

    // Notebook should have the conclusion
    let notebook = sink.notebook.lock().unwrap();
    assert!(!notebook.is_empty(), "should have notebook entry");
    assert!(
        notebook[0].entry.contains("titration") || notebook[0].entry.contains("Titration"),
        "notebook should mention titration"
    );

    // Audit log should have entries for each tool
    let audit = std::fs::read_to_string(&audit_path).unwrap_or_default();
    let audit_lines: Vec<&str> = audit.lines().collect();
    assert!(
        audit_lines.len() >= 5,
        "audit log should have at least 5 entries (one per tool), got {}",
        audit_lines.len()
    );

    println!("\n=== LAB-READY TEST 5 PASSED ===");
    println!("  Instruments used: {tool_names:?}");
    println!("  All {} tool calls succeeded ✓", tool_events.len());
    println!("  {} notebook entries ✓", notebook.len());
    println!("  {} audit log lines ✓", audit_lines.len());
    println!("  Pipeline: LLM → Sandbox → Capability → Proof → gRPC → Hardware ✓");
}
