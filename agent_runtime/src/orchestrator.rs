//! Top-level orchestrator that drives the agent loop.
//!
//! Each iteration:
//! 1. Build a prompt from the current experiment state + tool specs.
//! 2. Call the LLM.
//! 3. Sanitize and schema-validate the response.
//! 4. Validate actions against sandbox → approval → capability → proof policy.
//! 5. Execute tool calls and advance the experiment lifecycle.

use crate::audit::{AuditEvent, AuditSigner, emit_jsonl, emit_remote_with_retry, trace_id};
use crate::approvals::{ApprovalPolicy, risk_class_for_action};
use crate::capabilities::CapabilityPolicy;
use crate::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
use crate::experiment::{Experiment, Stage};
use crate::llm::{ChatMessage, LlmBackend};
use crate::revocation::RevocationList;
use crate::sandbox::Sandbox;
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use proof_artifacts::manifest::RiskClass;
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use std::collections::HashMap;
use std::sync::Arc;
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

/// Maximum byte length of any LLM response accepted for tool-call parsing.
///
/// Responses beyond this length are truncated before JSON parsing to prevent
/// O(n) allocation amplification from pathological LLM output.
const MAX_RESPONSE_BYTES: usize = 64 * 1024; // 64 KiB

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
    /// Optional per-tool capability bounds (workspace geometry, max dispense volume, etc.).
    pub capability_policy: Option<CapabilityPolicy>,
    /// Optional two-person approval policy for high-risk actions.
    pub approval_policy: Option<ApprovalPolicy>,
    /// Session nonce for replay prevention.
    ///
    /// When `Some`, every high-risk approval bundle must carry this nonce.
    /// Generate with `uuid::Uuid::new_v4().to_string()` at session start.
    pub session_nonce: Option<String>,
    /// Optional Ed25519 signer for per-event audit signatures.
    pub audit_signer: Option<AuditSigner>,
    /// Revocation list for keys and approval IDs.
    pub revocation_list: RevocationList,
    /// Optional event sink for live visualizer integration.
    ///
    /// When set, the orchestrator emits [`StateTransitionEvent`],
    /// [`ToolExecutionEvent`], [`LlmTokenEvent`], and [`NotebookEntryEvent`]
    /// after each significant action. All methods are synchronous and must not
    /// block the orchestrator loop.
    pub event_sink: Option<Arc<dyn EventSink>>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
            audit_log_path: std::env::var("AXIOMLAB_AUDIT_LOG").ok(),
            capability_policy: Some(CapabilityPolicy::default_lab()),
            approval_policy: Some(ApprovalPolicy::default_high_risk()),
            session_nonce: Some(uuid::Uuid::new_v4().to_string()),
            audit_signer: AuditSigner::from_env(),
            revocation_list: RevocationList::from_env(),
            event_sink: None,
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
    action_risk_index: HashMap<String, RiskClass>,
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
            action_risk_index: HashMap::new(),
        }
    }

    /// Enable runtime proof-policy enforcement for tool calls.
    pub fn with_runtime_policy(
        mut self,
        engine: RuntimePolicyEngine,
        context: ExecutionContext,
    ) -> Self {
        self.action_risk_index = engine
            .manifest()
            .actions
            .iter()
            .map(|a| (a.action.clone(), a.risk_class.clone()))
            .collect();
        self.policy_engine = Some(engine);
        self.policy_context = Some(context);
        self
    }

    // ── Event helpers ─────────────────────────────────────────────

    fn emit_transition(&self, experiment: &Experiment, from: Stage) {
        if let Some(sink) = &self.config.event_sink {
            sink.on_state_transition(StateTransitionEvent {
                from: format!("{:?}", from),
                to: format!("{:?}", experiment.stage),
                experiment_id: experiment.id.clone(),
                timestamp_ms: unix_ms(),
            });
        }
    }

    fn emit_tool_event(
        &self,
        tool_name: &str,
        params: &serde_json::Value,
        status: &str,
        reason: &str,
    ) {
        if let Some(sink) = &self.config.event_sink {
            let target = extract_target(params);
            let max_safe_limit = self
                .config
                .capability_policy
                .as_ref()
                .and_then(|cp| primary_cap_limit(cp, tool_name))
                .unwrap_or(0.0);
            sink.on_tool_execution(ToolExecutionEvent {
                tool: tool_name.to_owned(),
                target,
                params: params.clone(),
                max_safe_limit,
                status: status.to_owned(),
                reason: reason.to_owned(),
            });
        }
    }

    async fn stream_tokens(&self, text: &str) {
        if let Some(sink) = &self.config.event_sink {
            for ch in text.chars() {
                sink.on_llm_token(LlmTokenEvent {
                    token: ch.to_string(),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
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

        // Emit the initial Proposed state.
        self.emit_transition(experiment, Stage::Proposed);

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

            let raw_response = self.llm.chat(&history, temperature).await?;
            info!(len = raw_response.len(), "LLM response received");

            // Stream tokens to the visualizer before further processing.
            self.stream_tokens(&raw_response).await;

            // Sanitize before any further processing.
            let response = sanitize_llm_response(&raw_response);

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
                    let prev = experiment.stage;
                    experiment.advance(Stage::CodeGenerated)?;
                    self.emit_transition(experiment, prev);
                }
            }

            // Check for completion signal.
            if response.contains("\"done\"") && response.contains("true") {
                let summary = extract_summary(&response);
                self.advance_to_completion(experiment)?;
                // Emit a Lab Notebook entry with the AI's documented finding.
                if let Some(sink) = &self.config.event_sink {
                    sink.on_notebook_entry(NotebookEntryEvent {
                        experiment_id: experiment.id.clone(),
                        entry: summary,
                        timestamp_ms: unix_ms(),
                        tool_that_triggered: "analysis".to_owned(),
                        outcome: "discovery".to_owned(),
                    });
                }
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
    /// against the sandbox, approval policy, capability bounds, and proof
    /// artifacts, then dispatch it.
    async fn try_tool_call(&self, response: &str) -> Option<ToolResult> {
        let parsed: serde_json::Value = serde_json::from_str(response).ok()?;

        // Schema validation: the JSON must have "tool" (string) and "params" (object).
        if let Err(schema_err) = validate_tool_call_schema(&parsed) {
            warn!(%schema_err, "LLM response failed tool-call schema validation");
            return None;
        }

        let tool_name = parsed.get("tool")?.as_str()?;
        let params = parsed.get("params")?.clone();

        // ── Stage 0: Sandbox allowlist ────────────────────────────────────
        if let Err(e) = self.sandbox.check_command(tool_name) {
            error!(%e, "sandbox rejected tool call");
            self.audit_decision(tool_name, "deny", &e.to_string(), false, None).await;
            self.emit_tool_event(tool_name, &params, "rejected", &e.to_string());
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(e.to_string()),
                success: false,
            });
        }

        let risk_class = risk_class_for_action(tool_name, &self.action_risk_index);

        // ── Stage 1: Two-person approval ──────────────────────────────────
        if let (Some(policy), Some(ctx)) = (&self.config.approval_policy, &self.policy_context) {
            match policy.validate_action(
                tool_name,
                risk_class.clone(),
                ctx,
                &params,
                self.config.session_nonce.as_deref(),
            ) {
                Ok(approval_ids) => {
                    // Check revocation on all approval IDs.
                    for aid in &approval_ids {
                        if self.config.revocation_list.is_approval_revoked(aid) {
                            let msg = format!("approval {aid} has been revoked");
                            self.audit_decision(tool_name, "deny", &msg, false, None).await;
                            return Some(ToolResult {
                                name: tool_name.to_owned(),
                                output: serde_json::Value::String(msg),
                                success: false,
                            });
                        }
                    }
                    if !approval_ids.is_empty() {
                        let reason = format!(
                            "two-person approval satisfied for high-risk action (approval_ids={})",
                            approval_ids.join(",")
                        );
                        self.audit_decision(
                            tool_name,
                            "allow",
                            &reason,
                            true,
                            Some(approval_ids),
                        )
                        .await;
                    }
                }
                Err(e) => {
                    self.audit_decision(tool_name, "deny", &e, false, None).await;
                    self.emit_tool_event(tool_name, &params, "rejected", &e);
                    return Some(ToolResult {
                        name: tool_name.to_owned(),
                        output: serde_json::Value::String(e),
                        success: false,
                    });
                }
            }
        }

        // ── Stage 2: Capability bounds ────────────────────────────────────
        if let Some(capability) = &self.config.capability_policy {
            if let Err(e) = capability.validate(tool_name, &params) {
                self.audit_decision(tool_name, "deny", &e, false, None).await;
                self.emit_tool_event(tool_name, &params, "rejected", &e);
                return Some(ToolResult {
                    name: tool_name.to_owned(),
                    output: serde_json::Value::String(e),
                    success: false,
                });
            }
        }

        // ── Stage 3: Fail-closed for high-risk without policy ─────────────
        let high_risk = matches!(risk_class, Some(RiskClass::Actuation | RiskClass::Destructive));
        if high_risk && (self.policy_engine.is_none() || self.policy_context.is_none()) {
            let msg = "high-risk action denied: runtime proof policy is not configured";
            self.audit_decision(tool_name, "deny", msg, false, None).await;
            self.emit_tool_event(tool_name, &params, "rejected", msg);
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(msg.into()),
                success: false,
            });
        }

        // ── Stage 4: Proof-artifact policy ───────────────────────────────
        if let (Some(engine), Some(ctx)) = (&self.policy_engine, &self.policy_context) {
            if let Err(e) = engine.authorize(tool_name, ctx) {
                let report = engine.explain(tool_name);
                self.audit_decision(tool_name, "deny", &report.reason, false, None).await;
                self.emit_tool_event(tool_name, &params, "rejected", &report.reason);
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

        // ── Stage 5: Audit allow + dispatch ──────────────────────────────
        self.audit_decision(
            tool_name,
            "allow",
            "policy and sandbox checks passed",
            true,
            None,
        )
        .await;

        self.emit_tool_event(tool_name, &params, "success", "policy and sandbox checks passed");

        let call = ToolCall {
            name: tool_name.to_owned(),
            params,
        };
        Some(self.tools.dispatch(&call).await)
    }

    async fn audit_decision(
        &self,
        action: &str,
        decision: &str,
        reason: &str,
        success: bool,
        approval_ids: Option<Vec<String>>,
    ) {
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
            approval_ids,
        };

        let mut payload_line = serde_json::to_string(&event).unwrap_or_default();

        if let Some(path) = &self.config.audit_log_path {
            match emit_jsonl(path, &event, self.config.audit_signer.as_ref()) {
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
                let prev = experiment.stage;
                experiment.advance(s)?;
                self.emit_transition(experiment, prev);
            }
        }
        Ok(())
    }
}

// ── Event utilities ───────────────────────────────────────────────────────────

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Extract a human-readable target identifier from tool params.
/// Checks common field names in priority order.
fn extract_target(params: &serde_json::Value) -> String {
    for key in &["pump_id", "sensor_id", "vessel_id", "target", "chamber_id"] {
        if let Some(v) = params.get(key).and_then(|v| v.as_str()) {
            return v.to_owned();
        }
    }
    // For move_arm, synthesize a position string.
    if let (Some(x), Some(y), Some(z)) = (
        params["x"].as_f64(),
        params["y"].as_f64(),
        params["z"].as_f64(),
    ) {
        return format!("({x:.0},{y:.0},{z:.0})mm");
    }
    "unknown".to_owned()
}

/// Return the primary upper-bound limit for the given tool from the capability policy.
fn primary_cap_limit(policy: &CapabilityPolicy, tool_name: &str) -> Option<f64> {
    match tool_name {
        "dispense" => policy.max_for("dispense", "volume_ul"),
        "move_arm" => policy.max_for("move_arm", "x"),
        _ => None,
    }
}

/// Extract the `summary` field from `{"done": true, "summary": "..."}` responses.
fn extract_summary(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(s) = v.get("summary").and_then(|s| s.as_str()) {
            return s.to_owned();
        }
    }
    // Fallback: use the raw response trimmed to 500 chars.
    text.chars().take(500).collect()
}

// ── LLM response sanitization ─────────────────────────────────────────────────

/// Sanitize a raw LLM response before any processing.
///
/// - Truncates to `MAX_RESPONSE_BYTES` to prevent allocation amplification.
/// - Strips null bytes (prevent JSON parser confusion).
/// - Does NOT strip JSON or code content — only structural anomalies.
fn sanitize_llm_response(raw: &str) -> String {
    // Truncate at a UTF-8 character boundary.
    let truncated = if raw.len() > MAX_RESPONSE_BYTES {
        warn!(
            original_bytes = raw.len(),
            limit = MAX_RESPONSE_BYTES,
            "LLM response truncated before processing"
        );
        // Walk back to the last valid UTF-8 char boundary.
        let mut end = MAX_RESPONSE_BYTES;
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        &raw[..end]
    } else {
        raw
    };

    // Strip null bytes that can confuse parsers.
    truncated.replace('\0', "")
}

/// Validate that a JSON value matches the expected tool-call schema:
/// `{ "tool": string, "params": object }`
///
/// Returns `Err` with a human-readable description of the first violation.
fn validate_tool_call_schema(value: &serde_json::Value) -> Result<(), String> {
    let obj = value.as_object().ok_or("tool call must be a JSON object")?;

    let tool = obj
        .get("tool")
        .ok_or("tool call missing required field 'tool'")?;
    if !tool.is_string() {
        return Err(format!("'tool' must be a string, got {}", tool));
    }

    let tool_name = tool.as_str().unwrap();
    // Tool names must be non-empty and alphanumeric + underscores only.
    if tool_name.is_empty() {
        return Err("'tool' must be a non-empty string".into());
    }
    if !tool_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
    {
        return Err(format!(
            "'tool' name '{tool_name}' contains invalid characters (allowed: [a-zA-Z0-9_])"
        ));
    }

    let params = obj
        .get("params")
        .ok_or("tool call missing required field 'params'")?;
    if !params.is_object() {
        return Err(format!("'params' must be a JSON object, got {}", params));
    }

    Ok(())
}

// ── Code extraction ───────────────────────────────────────────────────────────

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

    #[test]
    fn sanitize_truncates_oversized_response() {
        let big = "x".repeat(MAX_RESPONSE_BYTES + 1000);
        let result = sanitize_llm_response(&big);
        assert!(result.len() <= MAX_RESPONSE_BYTES);
    }

    #[test]
    fn sanitize_strips_null_bytes() {
        let with_nulls = "hello\0world\0";
        assert_eq!(sanitize_llm_response(with_nulls), "helloworld");
    }

    #[test]
    fn schema_valid_tool_call() {
        let v = serde_json::json!({"tool": "move_arm", "params": {"x": 10}});
        assert!(validate_tool_call_schema(&v).is_ok());
    }

    #[test]
    fn schema_rejects_missing_tool() {
        let v = serde_json::json!({"params": {}});
        assert!(validate_tool_call_schema(&v).is_err());
    }

    #[test]
    fn schema_rejects_non_string_tool() {
        let v = serde_json::json!({"tool": 42, "params": {}});
        assert!(validate_tool_call_schema(&v).is_err());
    }

    #[test]
    fn schema_rejects_invalid_tool_name_chars() {
        let v = serde_json::json!({"tool": "rm -rf /", "params": {}});
        assert!(validate_tool_call_schema(&v).is_err());
    }

    #[test]
    fn schema_rejects_non_object_params() {
        let v = serde_json::json!({"tool": "move_arm", "params": [1, 2, 3]});
        assert!(validate_tool_call_schema(&v).is_err());
    }
}
