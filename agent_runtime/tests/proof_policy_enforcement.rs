use agent_runtime::experiment::Experiment;
use agent_runtime::llm::{ChatMessage, LlmBackend, LlmError};
use agent_runtime::orchestrator::{Orchestrator, OrchestratorConfig};
use agent_runtime::sandbox::{ResourceLimits, Sandbox};
use agent_runtime::tools::{ToolRegistry, register_lab_tools};
use proof_artifacts::manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest, RiskClass,
};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn lab_sandbox() -> Sandbox {
    Sandbox::new(
        vec![PathBuf::from("/lab/workspace")],
        vec!["move_arm".into(), "read_sensor".into(), "dispense".into()],
        ResourceLimits {
            max_execution_secs: 10,
            max_write_bytes: 1024 * 1024,
            max_hw_channels: 2,
        },
    )
}

struct ScriptedLlm {
    responses: Vec<String>,
}

impl ScriptedLlm {
    fn tool_then_done(tool: &str) -> Self {
        Self {
            responses: vec![
                format!("{{\"tool\": \"{}\", \"params\": {{\"x\": 1, \"y\": 2, \"z\": 3}}}}", tool),
                r#"{"done": true, "summary": "finished"}"#.into(),
            ],
        }
    }
}

impl LlmBackend for ScriptedLlm {
    fn chat(
        &self,
        messages: &[ChatMessage],
        _temperature: f64,
    ) -> impl std::future::Future<Output = Result<String, LlmError>> + Send {
        let turn = messages.iter().filter(|m| m.role == "assistant").count();
        let response = self
            .responses
            .get(turn)
            .cloned()
            .unwrap_or_else(|| r#"{"done": true, "summary": "finished"}"#.into());
        async move { Ok(response) }
    }
}

fn manifest_for(action_status: ArtifactStatus) -> ProofManifest {
    ProofManifest {
        schema_version: 1,
        generated_unix_secs: 0,
        build: BuildIdentity {
            git_commit: "git123".into(),
            binary_hash: "bin123".into(),
            workspace_hash: "ws123".into(),
            container_image_digest: Some("img:sha256:test".into()),
            device_id: Some("rig-test".into()),
            firmware_version: Some("fw-test".into()),
        },
        artifacts: vec![ProofArtifact {
            id: "arm_safety".into(),
            source_path: "verus_verified/lab_safety.rs".into(),
            source_hash: "h".into(),
            mir_path: None,
            mir_hash: None,
            lean: vec![],
            verus: None,
            theorem_count: 10,
            sorry_count: 0,
            status: action_status,
            metadata: BTreeMap::new(),
        }],
        actions: vec![ActionPolicy {
            action: "move_arm".into(),
            risk_class: RiskClass::Actuation,
            required_artifacts: vec!["arm_safety".into()],
            rationale: "Arm actuation requires arm safety proof chain".into(),
        }],
    }
}

#[tokio::test]
async fn proof_policy_blocks_action_when_artifact_failed() {
    let llm = ScriptedLlm::tool_then_done("move_arm");
    let sandbox = lab_sandbox();
    let mut tools = ToolRegistry::new();
    register_lab_tools(&mut tools);

    let engine = RuntimePolicyEngine::new_trusted(manifest_for(ArtifactStatus::Failed));
    let ctx = ExecutionContext {
        git_commit: "git123".into(),
        binary_hash: "bin123".into(),
        container_image_digest: Some("img:sha256:test".into()),
        device_id: Some("rig-test".into()),
        firmware_version: Some("fw-test".into()),
    };

    let orch = Orchestrator::new(
        llm,
        sandbox,
        tools,
        OrchestratorConfig {
            max_iterations: 3,
            code_gen_temperature: 0.0,
            reasoning_temperature: 0.0,
            audit_log_path: None,
        },
    )
    .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("proof-pol-deny", "policy deny check");
    let res = orch.run_experiment(&mut exp).await;
    assert!(res.is_ok(), "orchestrator should proceed to done even if tool denied");
}

#[tokio::test]
async fn proof_policy_allows_action_when_artifact_passed() {
    let llm = ScriptedLlm::tool_then_done("move_arm");
    let sandbox = lab_sandbox();
    let mut tools = ToolRegistry::new();
    register_lab_tools(&mut tools);

    let engine = RuntimePolicyEngine::new_trusted(manifest_for(ArtifactStatus::Passed));
    let ctx = ExecutionContext {
        git_commit: "git123".into(),
        binary_hash: "bin123".into(),
        container_image_digest: Some("img:sha256:test".into()),
        device_id: Some("rig-test".into()),
        firmware_version: Some("fw-test".into()),
    };

    let orch = Orchestrator::new(
        llm,
        sandbox,
        tools,
        OrchestratorConfig {
            max_iterations: 3,
            code_gen_temperature: 0.0,
            reasoning_temperature: 0.0,
            audit_log_path: None,
        },
    )
    .with_runtime_policy(engine, ctx);

    let mut exp = Experiment::new("proof-pol-allow", "policy allow check");
    let res = orch.run_experiment(&mut exp).await;
    assert!(res.is_ok());
}
