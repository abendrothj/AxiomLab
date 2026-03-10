//! Top-level orchestrator that drives the agent loop.
//!
//! Each iteration:
//! 1. Build a prompt from the current experiment state + tool specs.
//! 2. Call the LLM.
//! 3. Parse the response for tool calls or code generation.
//! 4. Validate actions against the sandbox.
//! 5. Execute tool calls and advance the experiment lifecycle.

use crate::experiment::{Experiment, Stage};
use crate::llm::{ChatMessage, LlmBackend};
use crate::sandbox::Sandbox;
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
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
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
        }
    }
}

/// The main agent orchestrator.
pub struct Orchestrator<L: LlmBackend> {
    llm: L,
    sandbox: Sandbox,
    tools: ToolRegistry,
    config: OrchestratorConfig,
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
        }
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
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(e.to_string()),
                success: false,
            });
        }

        let call = ToolCall {
            name: tool_name.to_owned(),
            params,
        };
        Some(self.tools.dispatch(&call).await)
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
