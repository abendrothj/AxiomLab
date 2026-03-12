//! Top-level orchestrator that drives the agent loop.
//!
//! Each iteration:
//! 1. Build a prompt from the current experiment state + tool specs.
//! 2. Call the LLM.
//! 3. Parse the response for tool calls or code generation.
//! 4. Validate actions against the sandbox.
//! 5. Execute tool calls and advance the experiment lifecycle.

use crate::audit::{AuditEvent, emit_jsonl, emit_remote_with_retry, trace_id};
use crate::experiment::{Experiment, Stage};
use crate::llm::{ChatMessage, LlmBackend};
use crate::sandbox::Sandbox;
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use thiserror::Error;
use tracing::{error, info, warn};

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("LLM error: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("sandbox violation: {0}")]
    Sandbox(#[from] crate::sandbox::SandboxError),
    #[error("experiment error: {0}")]
    Experiment(#[from] crate::experiment::ExperimentError),
    #[error("orchestrator halted: {0}")]
    Halted(String),
}

/// Configuration for the orchestrator.
pub struct OrchestratorConfig {
    /// Maximum iterations per experiment before aborting.
    pub max_iterations: u32,
    /// LLM temperature for code generation.
    pub code_gen_temperature: f64,
    /// LLM temperature for planning / reasoning.
    pub reasoning_temperature: f64,
    /// Optional JSONL audit log path for action allow/deny events.
    pub audit_log_path: Option<String>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
            audit_log_path: std::env::var("AXIOMLAB_AUDIT_LOG").ok(),
        }
    }
}

/// The main agent orchestrator.
pub struct Orchestrator<L: LlmBackend> {
    llm: L,
    sandbox: Sandbox,
    tools: ToolRegistry,
    config: OrchestratorConfig,
    policy_engine: Option<RuntimePolicyEngine>,
    policy_context: Option<ExecutionContext>,
}

impl<L: LlmBackend> Orchestrator<L> {
    pub fn new(
        llm: L,
        sandbox: Sandbox,
        tools: ToolRegistry,
        config: OrchestratorConfig,
    ) -> Self {
        Self {
            llm,
            sandbox,
            tools,
            config,
            policy_engine: None,
            policy_context: None,
        }
    }

    /// Enable runtime proof-policy enforcement for tool calls.
    pub fn with_runtime_policy(
        mut self,
        engine: RuntimePolicyEngine,
        context: ExecutionContext,
    ) -> Self {
        self.policy_engine = Some(engine);
        self.policy_context = Some(context);
        self
    }

    /// Build the system prompt from tool specs.
    fn system_prompt(&self) -> String {
        let tool_descriptions: Vec<String> = self
            .tools
            .specs()
            .iter()
            .map(|t| {
                format!(
                    "- **{}**: {}\n  params: {}",
                    t.name, t.description, t.parameters_schema
                )
            })
            .collect();

        format!(
            "You are an autonomous lab scientist agent in AxiomLab.\n\
             You control physical lab hardware through these tools:\n\
             {}\n\n\
             Respond with JSON when you want to call a tool:\n\
             {{\"tool\": \"<name>\", \"params\": {{...}}}}\n\n\
             When generating experiment code, wrap it in ```rust ... ```.\n\
             When you have a conclusion, respond with: {{\"done\": true, \"summary\": \"...\"}}",
            tool_descriptions.join("\n")
        )
    }

    /// Run a single experiment through the full lifecycle.
    pub async fn run_experiment(
        &self,
        experiment: &mut Experiment,
    ) -> Result<(), OrchestratorError> {
        info!(id = %experiment.id, hypothesis = %experiment.hypothesis, "starting experiment");

        let mut history = vec![
            ChatMessage {
                role: "system".into(),
                content: self.system_prompt(),
            },
            ChatMessage {
                role: "user".into(),
                content: format!(
                    "Design and execute an experiment to test this hypothesis: {}",
                    experiment.hypothesis
                ),
            },
        ];

        for iteration in 0..self.config.max_iterations {
            info!(iteration, stage = ?experiment.stage, "orchestrator step");

            let temperature = match experiment.stage {
                Stage::Proposed => self.config.reasoning_temperature,
                _ => self.config.code_gen_temperature,
            };

            let response = self.llm.chat(&history, temperature).await?;
            info!(len = response.len(), "LLM response received");

            history.push(ChatMessage {
                role: "assistant".into(),
                content: response.clone(),
            });

            // Try to parse as a tool call.
            if let Some(tool_result) = self.try_tool_call(&response).await {
                let result_json = serde_json::to_string(&tool_result).unwrap_or_default();
                history.push(ChatMessage {
                    role: "user".into(),
                    content: format!("Tool result: {result_json}"),
                });
                continue;
            }

            // Try to extract generated code.
            if let Some(code) = extract_rust_code(&response) {
                info!(len = code.len(), "extracted generated Rust code");
                experiment.source_code = Some(code);
                if experiment.stage == Stage::Proposed {
                    experiment.advance(Stage::CodeGenerated)?;
                }
            }

            // Check for completion signal.
            if response.contains("\"done\"") && response.contains("true") {
                self.advance_to_completion(experiment)?;
                info!(id = %experiment.id, "experiment completed");
                return Ok(());
            }
        }

        warn!(id = %experiment.id, "max iterations reached");
        experiment.fail("max orchestrator iterations reached");
        Err(OrchestratorError::Halted(
            "max iterations reached".to_owned(),
        ))
    }

    /// Attempt to parse a tool call from the LLM response, validate it
    /// against the sandbox, and dispatch it.
    async fn try_tool_call(&self, response: &str) -> Option<ToolResult> {
        let parsed: serde_json::Value = serde_json::from_str(response).ok()?;
        let tool_name = parsed.get("tool")?.as_str()?;
        let params = parsed.get("params")?.clone();

        // Sandbox check — the tool name must be on the allowlist.
        if let Err(e) = self.sandbox.check_command(tool_name) {
            error!(%e, "sandbox rejected tool call");
            self.audit_decision(tool_name, "deny", &e.to_string(), false).await;
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(e.to_string()),
                success: false,
            });
        }

        // Fail-closed mode for high-risk actions when policy is missing.
        let high_risk = matches!(tool_name, "move_arm" | "dispense");
        if high_risk && (self.policy_engine.is_none() || self.policy_context.is_none()) {
            let msg = "high-risk action denied: runtime proof policy is not configured";
            self.audit_decision(tool_name, "deny", msg, false).await;
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(msg.into()),
                success: false,
            });
        }

        // Proof-policy check — action is allowed only when required proof
        // artifacts are present, passed, and tied to this exact binary/commit.
        if let (Some(engine), Some(ctx)) = (&self.policy_engine, &self.policy_context) {
            if let Err(e) = engine.authorize(tool_name, ctx) {
                let report = engine.explain(tool_name);
                self.audit_decision(tool_name, "deny", &report.reason, false).await;
                return Some(ToolResult {
                    name: tool_name.to_owned(),
                    output: serde_json::json!({
                        "error": e.to_string(),
                        "decision": format!("{:?}", report.decision),
                        "reason": report.reason,
                        "policy": report.matched_policy,
                        "artifacts_checked": report.artifacts_checked
                    }),
                    success: false,
                });
            }
        }

        self.audit_decision(tool_name, "allow", "policy and sandbox checks passed", true).await;

        let call = ToolCall {
            name: tool_name.to_owned(),
            params,
        };
        Some(self.tools.dispatch(&call).await)
    }

    async fn audit_decision(&self, action: &str, decision: &str, reason: &str, success: bool) {
        let event = AuditEvent {
            unix_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            trace_id: trace_id(action),
            action: action.to_owned(),
            decision: decision.to_owned(),
            reason: reason.to_owned(),
            success,
        };

        let mut payload_line = serde_json::to_string(&event).unwrap_or_default();

        if let Some(path) = &self.config.audit_log_path {
            match emit_jsonl(path, &event) {
                Ok(line) => payload_line = line,
                Err(e) => warn!(error = %e, "failed to write local audit event"),
            }
        }

        if let Err(e) = emit_remote_with_retry(&payload_line).await {
            warn!(error = %e, "failed to mirror audit event to remote sink");
        }
    }

    fn advance_to_completion(
        &self,
        experiment: &mut Experiment,
    ) -> Result<(), OrchestratorError> {
        let stages = [
            Stage::CodeGenerated,
            Stage::Verified,
            Stage::Executing,
            Stage::Analysing,
            Stage::Completed,
        ];
        for &s in &stages {
            if experiment.stage < s {
                experiment.advance(s)?;
            }
        }
        Ok(())
    }
}

/// Extract the first ```rust ... ``` block from a string.
fn extract_rust_code(text: &str) -> Option<String> {
    let start = text.find("```rust")?;
    let code_start = start + 7;
    let end = text[code_start..].find("```")?;
    let code = text[code_start..code_start + end].trim();
    if code.is_empty() {
        None
    } else {
        Some(code.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_block() {
        let text = "Here is code:\n```rust\nfn main() {}\n```\nDone.";
        let code = extract_rust_code(text).unwrap();
        assert_eq!(code, "fn main() {}");
    }

    #[test]
    fn no_code_block() {
        assert!(extract_rust_code("no code here").is_none());
    }
}
