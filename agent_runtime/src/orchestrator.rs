//! Top-level orchestrator that drives the agent loop.
//!
//! Each iteration:
//! 1. Build a prompt from the current experiment state + tool specs.
//! 2. Call the LLM.
//! 3. Sanitize and schema-validate the response.
//! 4. Validate actions against sandbox → approval → capability → proof policy.
//! 5. Execute tool calls and advance the experiment lifecycle.

use crate::approval_queue::{ApprovalContext, PendingApprovalQueue, ProtocolStepInfo};
use crate::audit::{
    AuditEvent, AuditSigner, audit_signer_from_env, emit_jsonl, emit_protocol_conclusion, emit_protocol_step,
    emit_remote_with_retry, trace_id,
};
use crate::rekor;
use crate::approvals::{ApprovalPolicy, risk_class_for_action};
use crate::capabilities::CapabilityPolicy;
use crate::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, ProtocolConclusionEvent, ProtocolStepEvent,
    StateTransitionEvent, ToolExecutionEvent,
};
use crate::experiment::{Experiment, Stage};
use crate::llm::{ChatMessage, LlmBackend};
use crate::protocol::{Protocol, ProtocolPlan, ProtocolRunResult, RekorStatus, StepOutcome, ZkProofStatus};
use crate::revocation::RevocationList;
use crate::sandbox::Sandbox;
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use proof_artifacts::manifest::RiskClass;
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use sha2::Digest;
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

/// How many consecutive unparseable LLM responses trigger an experiment failure.
///
/// Each failure injects a format-correction re-prompt into history. If the model
/// cannot produce valid JSON after this many attempts in a row the experiment is
/// aborted rather than silently burning all remaining iterations.
const MAX_CONSECUTIVE_PARSE_FAILURES: u32 = 3;

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
    pub audit_signer: Option<Box<dyn AuditSigner>>,
    /// Revocation list for keys and approval IDs.
    pub revocation_list: RevocationList,
    /// Optional event sink for live visualizer integration.
    ///
    /// When set, the orchestrator emits [`StateTransitionEvent`],
    /// [`ToolExecutionEvent`], [`LlmTokenEvent`], and [`NotebookEntryEvent`]
    /// after each significant action. All methods are synchronous and must not
    /// block the orchestrator loop.
    pub event_sink: Option<Arc<dyn EventSink>>,

    /// Shared interactive approval queue.
    ///
    /// When `Some`, high-risk actions with no pre-signed bundle are placed into
    /// this queue and the tool call blocks until an operator approves or denies
    /// via `POST /api/approvals/submit`, or until `approval_timeout_secs` elapses.
    ///
    /// When `None`, the original instant-deny behaviour applies.
    pub approval_queue: Option<Arc<PendingApprovalQueue>>,

    /// Seconds to wait for operator approval before auto-denying.
    /// Default: 300 (5 minutes). Only meaningful when `approval_queue` is `Some`.
    pub approval_timeout_secs: u64,

    /// Pre-formatted discovery journal summary injected into every approval
    /// context this experiment produces.  Set by the server layer from the
    /// persisted journal before each experiment; not touched by the LLM.
    pub journal_summary: String,

    /// Number of confirmed findings in the journal at the start of this
    /// experiment.  Lets the operator compare "before" vs "current" to judge
    /// whether the agent has made progress.
    pub findings_at_start: u32,

    /// Per-instrument calibration validity at the start of this experiment.
    /// `tool_name → (is_calibrated, is_valid_now)`.
    /// Computed by the server from `DiscoveryJournal.last_calibration_for()`.
    /// Empty map means no calibration checking (graceful degradation).
    pub calibration_status: std::collections::HashMap<String, (bool, bool)>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
            audit_log_path: Some(
                crate::audit::audit_log_path().to_string_lossy().into_owned()
            ),
            capability_policy: Some(CapabilityPolicy::default_lab()),
            approval_policy: Some(ApprovalPolicy::default_high_risk()),
            session_nonce: Some(uuid::Uuid::new_v4().to_string()),
            audit_signer: audit_signer_from_env(),
            revocation_list: RevocationList::from_env(),
            event_sink: None,
            approval_queue: None,
            approval_timeout_secs: 300,
            journal_summary: String::new(),
            findings_at_start: 0,
            calibration_status: std::collections::HashMap::new(),
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

    /// Inject a risk index without a full policy engine.
    ///
    /// Lets integration tests trigger Stage 3 (fail-closed) in isolation.
    /// **Never use in production** — always call `with_runtime_policy` instead.
    #[doc(hidden)]
    pub fn with_risk_index_only(mut self, index: HashMap<String, RiskClass>) -> Self {
        self.action_risk_index = index;
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
                let units_note = if t.parameter_units.is_empty() {
                    String::new()
                } else {
                    let pairs: Vec<String> = t.parameter_units
                        .iter()
                        .map(|(k, v)| format!("{k} [{v}]"))
                        .collect();
                    format!("\n  units: {}", pairs.join(", "))
                };
                format!(
                    "- **{}**: {}{}\n  params: {}",
                    t.name, t.description, units_note, t.parameters_schema
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

        let mut consecutive_parse_failures: u32 = 0;
        // Verified record of tool calls dispatched this experiment — used to
        // give scientists trustworthy context when approving high-risk actions.
        let mut recent_actions: Vec<(String, serde_json::Value)> = Vec::new();

        for iteration in 0..self.config.max_iterations {
            info!(iteration, stage = ?experiment.stage, "orchestrator step");

            // First call: reasoning temperature for planning.
            // Subsequent calls: lower temperature for precise tool-call generation.
            let temperature = if iteration == 0 {
                self.config.reasoning_temperature
            } else {
                self.config.code_gen_temperature
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

            // Check for propose_protocol — a structured multi-step protocol.
            if let Some(protocol_result) =
                self.try_propose_protocol(&response).await
            {
                consecutive_parse_failures = 0;
                let result_json =
                    serde_json::to_string(&protocol_result).unwrap_or_default();
                history.push(ChatMessage {
                    role: "user".into(),
                    content: format!("Protocol run result: {result_json}"),
                });
                continue;
            }

            // Try to parse as a tool call.
            let approval_ctx = ApprovalContext {
                hypothesis:                 experiment.hypothesis.clone(),
                experiment_id:              experiment.id.clone(),
                iteration,
                risk_class:                 None, // filled in by try_tool_call from the manifest
                recent_actions:             recent_actions.iter().rev().take(5).cloned().collect::<Vec<_>>()
                                                .into_iter().rev().collect(),
                journal_summary:            self.config.journal_summary.clone(),
                protocol_step:              None, // not in a structured protocol
                findings_before_experiment: self.config.findings_at_start,
            };
            let cot = extract_reasoning(&response);
            if let Some(tool_result) = self.try_tool_call(&response, Some(approval_ctx), cot).await {
                consecutive_parse_failures = 0;
                // Record this dispatch in recent_actions (capped at 20).
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) {
                    if let (Some(tool), Some(params)) = (
                        parsed.get("tool").and_then(|v| v.as_str()),
                        parsed.get("params"),
                    ) {
                        if recent_actions.len() >= 20 {
                            recent_actions.remove(0);
                        }
                        recent_actions.push((tool.to_owned(), params.clone()));
                    }
                }
                // Advance to Executing on the first real tool call.
                if experiment.stage == Stage::Proposed {
                    let prev = experiment.stage;
                    experiment.advance(Stage::Executing)?;
                    self.emit_transition(experiment, prev);
                }
                let result_json = serde_json::to_string(&tool_result).unwrap_or_default();
                history.push(ChatMessage {
                    role: "user".into(),
                    content: format!("Tool result: {result_json}"),
                });
                continue;
            }

            // Check for completion signal.
            if response.contains("\"done\"") && response.contains("true") {
                let summary = extract_summary(&response);
                // Advance through any remaining stages to Completed.
                if experiment.stage == Stage::Proposed {
                    let prev = experiment.stage;
                    experiment.advance(Stage::Executing)?;
                    self.emit_transition(experiment, prev);
                }
                {
                    let prev = experiment.stage;
                    experiment.advance(Stage::Completed)?;
                    self.emit_transition(experiment, prev);
                }
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

            // Nothing matched — re-prompt with a format correction.
            consecutive_parse_failures += 1;
            warn!(
                iteration,
                consecutive_parse_failures,
                snippet = %response.chars().take(200).collect::<String>(),
                "LLM response did not match any expected format — re-prompting"
            );
            if consecutive_parse_failures >= MAX_CONSECUTIVE_PARSE_FAILURES {
                experiment.fail(format!(
                    "{MAX_CONSECUTIVE_PARSE_FAILURES} consecutive unparseable responses"
                ));
                return Err(OrchestratorError::Halted(
                    "too many consecutive parse failures".into(),
                ));
            }
            history.push(ChatMessage {
                role: "user".into(),
                content: "Your last response was not valid JSON. Respond with exactly one of:\n\
                          1. Tool call:    {\"tool\": \"<name>\", \"params\": {...}}\n\
                          2. Protocol:     {\"tool\": \"propose_protocol\", \"params\": {...}}\n\
                          3. Completion:   {\"done\": true, \"summary\": \"...\"}\n\
                          Output only the JSON — no prose, no markdown fences.".into(),
            });
        }

        warn!(id = %experiment.id, "max iterations reached");
        experiment.fail("max orchestrator iterations reached");
        Err(OrchestratorError::Halted(
            "max iterations reached".to_owned(),
        ))
    }

    /// Check if the LLM response is a `propose_protocol` call.
    ///
    /// Returns `Some(ProtocolRunResult)` if the response was a valid protocol
    /// proposal that was executed.  Returns `None` if the response is not a
    /// `propose_protocol` call (so the caller can fall through to `try_tool_call`).
    async fn try_propose_protocol(&self, response: &str) -> Option<ProtocolRunResult> {
        let parsed: serde_json::Value = serde_json::from_str(response).ok()?;
        if parsed.get("tool")?.as_str()? != "propose_protocol" {
            return None;
        }
        let params = parsed.get("params")?;
        let plan: ProtocolPlan = serde_json::from_value(params.clone())
            .map_err(|e| warn!("propose_protocol: invalid plan JSON: {e}"))
            .ok()?;

        if let Err(e) = plan.validate() {
            warn!("propose_protocol: plan validation failed: {e}");
            return None;
        }

        let protocol = plan.into_protocol();
        Some(self.run_protocol(protocol).await)
    }

    /// Execute a tool call from pre-parsed structured data.
    ///
    /// Runs the full 5-stage validation pipeline (sandbox → approval → capability
    /// → proof policy → dispatch) without JSON parsing.  Used by [`run_protocol`]
    /// to execute individual protocol steps.
    ///
    /// `approval_ctx` is forwarded to `try_tool_call` so that interactive
    /// approval requests carry accurate protocol-step context.
    ///
    /// Returns a [`ToolResult`] regardless of whether the action was allowed —
    /// `ToolResult.success` indicates the outcome.
    pub async fn execute_tool_direct(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        approval_ctx: Option<ApprovalContext>,
    ) -> ToolResult {
        // Synthesise a JSON string and route through try_tool_call so all
        // validation logic stays in one place.
        let json = serde_json::json!({ "tool": tool_name, "params": params });
        match self.try_tool_call(&json.to_string(), approval_ctx, None).await {
            Some(result) => result,
            None => ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(
                    "tool call schema rejected (internal error)".into(),
                ),
                success: false,
            metadata: None,
            },
        }
    }

    /// Run a structured [`Protocol`], executing each step through the full
    /// 5-stage validation pipeline.
    ///
    /// After each step the LLM is shown the result so it can adapt its plan.
    /// After all steps it is asked for a scientific conclusion, which is written
    /// to the audit log with a per-conclusion Ed25519 signature.
    pub async fn run_protocol(&self, protocol: Protocol) -> ProtocolRunResult {
        let run_id = uuid::Uuid::new_v4();
        info!(
            protocol_id = %protocol.id,
            run_id = %run_id,
            name = %protocol.name,
            steps = protocol.steps.len(),
            "starting protocol run"
        );

        // Compute the manifest hash for audit records.
        let manifest_hash = self
            .policy_engine
            .as_ref()
            .map(|e| {
                let raw = serde_json::to_string(e.manifest()).unwrap_or_default();
                format!("{:x}", sha2::Sha256::digest(raw.as_bytes()))
            })
            .unwrap_or_else(|| "no-manifest".into());

        let total_replicates = protocol.replicate_count.max(1) as usize;
        let mut all_step_results: Vec<StepOutcome> = Vec::new();
        let mut replicate_succeeded_counts: Vec<usize> = Vec::new();

        // Build a conversation for the LLM to observe step results.
        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: format!(
                "You are observing the execution of protocol '{}'. \
                 Hypothesis: {}\n\
                 You will see each step result in turn. After all steps, \
                 provide a scientific conclusion as plain text.",
                protocol.name, protocol.hypothesis
            ),
        }];

        let step_count = protocol.steps.len();

        for rep in 0..total_replicates {
            if total_replicates > 1 {
                messages.push(ChatMessage {
                    role: "user".into(),
                    content: format!("--- Replicate {}/{} ---", rep + 1, total_replicates),
                });
            }

            let mut rep_succeeded = 0usize;

            for (i, step) in protocol.steps.iter().enumerate() {
                info!(step = i, replicate = rep, tool = %step.tool, "executing protocol step");

                let step_actx = ApprovalContext {
                    hypothesis:                 protocol.hypothesis.clone(),
                    experiment_id:              protocol.id.to_string(),
                    iteration:                  (rep * step_count + i) as u32,
                    risk_class:                 None,
                    recent_actions:             Vec::new(),
                    journal_summary:            self.config.journal_summary.clone(),
                    protocol_step:              Some(ProtocolStepInfo {
                        protocol_name: protocol.name.clone(),
                        step_index:    i,
                        step_count,
                        description:   step.description.clone(),
                    }),
                    findings_before_experiment: self.config.findings_at_start,
                };
                let result = self.execute_tool_direct(
                    &step.tool,
                    step.params.clone(),
                    Some(step_actx),
                ).await;
                let allowed = result.success;
                let rejection_reason = if !allowed {
                    result.output.as_str().map(|s| s.to_owned())
                } else {
                    None
                };

                // Extract vessel snapshot embedded by dispense/aspirate handlers.
                let vessel_snapshot = result.output.get("_vessel_snapshot").cloned();

                // Write to audit chain.
                if let Some(path) = &self.config.audit_log_path {
                    let _ = emit_protocol_step(
                        path,
                        protocol.id,
                        run_id,
                        i,
                        &step.tool,
                        &step.description,
                        allowed,
                        rejection_reason.as_deref(),
                        &manifest_hash,
                        vessel_snapshot.as_ref(),
                        self.config.audit_signer.as_deref(),
                    );
                }

                // Emit event to visualizer.
                if let Some(sink) = &self.config.event_sink {
                    sink.on_protocol_step(ProtocolStepEvent {
                        protocol_id: protocol.id.to_string(),
                        run_id: run_id.to_string(),
                        step_index: i,
                        tool: step.tool.clone(),
                        description: step.description.clone(),
                        allowed,
                        timestamp_ms: unix_ms(),
                    });
                }

                // Feed result back to LLM as an observation (strip internal snapshot key).
                let llm_output = if allowed {
                    let mut out = result.output.clone();
                    if let Some(obj) = out.as_object_mut() {
                        obj.remove("_vessel_snapshot");
                    }
                    out
                } else {
                    result.output.clone()
                };
                let obs = if allowed {
                    format!(
                        "Step {i} ({} — {}): SUCCESS. Result: {}",
                        step.tool,
                        step.description,
                        serde_json::to_string(&llm_output).unwrap_or_default()
                    )
                } else {
                    format!(
                        "Step {i} ({} — {}): REJECTED. Reason: {}",
                        step.tool,
                        step.description,
                        rejection_reason.as_deref().unwrap_or("unknown")
                    )
                };
                messages.push(ChatMessage { role: "user".into(), content: obs });

                if allowed {
                    rep_succeeded += 1;
                }

                all_step_results.push(StepOutcome {
                    step_index: i,
                    replicate_index: rep,
                    tool: step.tool.clone(),
                    description: step.description.clone(),
                    allowed,
                    result: if allowed { Some(llm_output) } else { None },
                    rejection_reason,
                });
            }

            replicate_succeeded_counts.push(rep_succeeded);
        }

        let steps_succeeded: usize = replicate_succeeded_counts.iter().sum();
        let step_results = all_step_results;

        // Compute replication aggregate (None for single-replicate runs).
        let aggregate = if total_replicates > 1 {
            Some(crate::protocol::ReplicateAggregate::from_counts(&replicate_succeeded_counts))
        } else {
            None
        };

        // Ask the LLM for its scientific conclusion.
        let conclusion_prompt = if let Some(ref agg) = aggregate {
            format!(
                "Protocol complete ({total_replicates} replicates). \
                 Steps succeeded: {steps_succeeded}/{} total. \
                 Mean steps per replicate: {:.2} ± {:.2} SD. \
                 Write your scientific conclusion based on these observations.",
                protocol.steps.len() * total_replicates,
                agg.mean_steps_succeeded,
                agg.sd_steps_succeeded,
            )
        } else {
            format!(
                "Protocol complete. {steps_succeeded}/{} steps succeeded. \
                 Write your scientific conclusion based on these observations.",
                protocol.steps.len()
            )
        };
        messages.push(ChatMessage { role: "user".into(), content: conclusion_prompt });

        let conclusion = match self.llm.chat(&messages, self.config.reasoning_temperature).await {
            Ok(text) => sanitize_llm_response(&text),
            Err(e) => {
                warn!(error = %e, "LLM failed to generate protocol conclusion");
                format!(
                    "Protocol '{name}' completed: {steps_succeeded}/{total} steps succeeded. \
                     LLM conclusion unavailable: {e}",
                    name = protocol.name,
                    total = protocol.steps.len()
                )
            }
        };

        info!(
            protocol_id = %protocol.id,
            run_id = %run_id,
            steps_succeeded,
            "protocol run concluded"
        );

        // Write signed conclusion to audit chain, then anchor externally to Rekor.
        let mut rekor_status = RekorStatus::Skipped;
        if let Some(path) = &self.config.audit_log_path {
            let conclusion_line = emit_protocol_conclusion(
                path,
                protocol.id,
                run_id,
                &protocol.name,
                &conclusion,
                protocol.steps.len(),
                steps_succeeded,
                protocol.template_id.as_deref(),
                self.config.audit_signer.as_deref(),
            );

            if let (Ok(line), Some(signer)) =
                (conclusion_line, self.config.audit_signer.as_deref())
            {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let (Some(hash), Some(sig)) = (
                        entry["entry_hash"].as_str(),
                        entry["entry_sig_b64"].as_str(),
                    ) {
                        let pubkey_pem = rekor::ed25519_pubkey_pem(&signer.verifying_key_bytes());
                        match rekor::submit_with_retry(hash, sig, &pubkey_pem).await {
                            Ok(uuid) => {
                                info!(
                                    rekor_uuid = %uuid,
                                    "protocol conclusion anchored to Rekor transparency log"
                                );
                                rekor_status = RekorStatus::Anchored { uuid };
                            }
                            Err(reason) => {
                                error!(
                                    error = %reason,
                                    "Rekor anchoring failed after retries — local audit chain intact"
                                );
                                rekor_status = RekorStatus::Failed { reason };
                            }
                        }
                    }
                }
            }
        }

        // Emit conclusion event.
        if let Some(sink) = &self.config.event_sink {
            sink.on_protocol_conclusion(ProtocolConclusionEvent {
                protocol_id: protocol.id.to_string(),
                run_id: run_id.to_string(),
                protocol_name: protocol.name.clone(),
                conclusion: conclusion.clone(),
                steps_succeeded,
                steps_total: protocol.steps.len(),
                timestamp_ms: unix_ms(),
            });

            // Also write to the Lab Notebook.
            sink.on_notebook_entry(crate::events::NotebookEntryEvent {
                experiment_id: protocol.id.to_string(),
                entry: conclusion.clone(),
                timestamp_ms: unix_ms(),
                tool_that_triggered: "propose_protocol".into(),
                outcome: if steps_succeeded == protocol.steps.len() {
                    "discovery".into()
                } else {
                    "inconclusive".into()
                },
            });
        }

        // Spawn background ZK proof task — does not block protocol completion.
        let zk_proof_status = self.spawn_zk_proof_if_configured(&self.config.audit_log_path);

        let uncertainty_budgets = self.build_uncertainty_budgets(&step_results);
        let doe_anova = protocol.doe_design_json.as_deref()
            .and_then(|json| self.run_doe_anova(json, &step_results));

        ProtocolRunResult {
            protocol_id: protocol.id,
            run_id,
            protocol_name: protocol.name,
            steps_total: protocol.steps.len(),
            steps_succeeded,
            conclusion,
            step_results,
            replicate_count: protocol.replicate_count,
            aggregate,
            rekor_status,
            zk_proof_status,
            uncertainty_budgets,
            doe_anova,
        }
    }

    /// Resume a partially-completed protocol from a crash-recovery state.
    ///
    /// Seeds the LLM context with prior step results (as "observed" messages),
    /// then continues from `last_completed_step + 1` through the remaining steps.
    /// The conclusion, signing, and Rekor anchoring proceed identically to a
    /// fresh `run_protocol` run.
    ///
    /// The caller must verify the audit chain is valid before calling this
    /// (see `scan_for_protocol_state` which returns `ChainInvalid` if not).
    pub async fn resume_protocol(
        &self,
        recovery: crate::protocol::ProtocolRecoveryState,
        protocol: &Protocol,
    ) -> ProtocolRunResult {
        let run_id = recovery.run_id;
        info!(
            protocol_id = %protocol.id,
            run_id = %run_id,
            resume_from_step = recovery.last_completed_step + 1,
            "resuming interrupted protocol run"
        );

        let manifest_hash = self
            .policy_engine
            .as_ref()
            .map(|e| {
                let raw = serde_json::to_string(e.manifest()).unwrap_or_default();
                format!("{:x}", sha2::Sha256::digest(raw.as_bytes()))
            })
            .unwrap_or_else(|| "no-manifest".into());

        // Seed LLM context with prior observations from the audit log.
        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: format!(
                "You are resuming protocol '{}' after an interruption. \
                 Hypothesis: {}\n\
                 The following steps were already completed before the interruption. \
                 Continue from step {}.",
                protocol.name,
                protocol.hypothesis,
                recovery.last_completed_step + 1,
            ),
        }];
        for (i, prior) in recovery.step_results.iter().enumerate() {
            messages.push(ChatMessage {
                role: "user".into(),
                content: format!(
                    "[Prior] Step {i}: {}",
                    serde_json::to_string(prior).unwrap_or_default()
                ),
            });
        }

        let resume_from = recovery.last_completed_step + 1;
        let step_count = protocol.steps.len();
        let total_replicates = protocol.replicate_count.max(1) as usize;
        let mut all_step_results: Vec<StepOutcome> = Vec::new();
        let mut rep_succeeded = 0usize;

        for i in resume_from..step_count {
            let step = &protocol.steps[i];
            info!(step = i, tool = %step.tool, "executing resumed protocol step");

            let step_actx = ApprovalContext {
                hypothesis:                 protocol.hypothesis.clone(),
                experiment_id:              protocol.id.to_string(),
                iteration:                  (recovery.replicate_index * step_count + i) as u32,
                risk_class:                 None,
                recent_actions:             Vec::new(),
                journal_summary:            self.config.journal_summary.clone(),
                protocol_step:              Some(ProtocolStepInfo {
                    protocol_name: protocol.name.clone(),
                    step_index:    i,
                    step_count,
                    description:   step.description.clone(),
                }),
                findings_before_experiment: self.config.findings_at_start,
            };

            let result = self.execute_tool_direct(&step.tool, step.params.clone(), Some(step_actx)).await;
            let allowed = result.success;
            let rejection_reason = if !allowed {
                result.output.as_str().map(|s| s.to_owned())
            } else {
                None
            };
            let vessel_snapshot = result.output.get("_vessel_snapshot").cloned();

            if let Some(path) = &self.config.audit_log_path {
                let _ = emit_protocol_step(
                    path, protocol.id, run_id, i, &step.tool, &step.description,
                    allowed, rejection_reason.as_deref(), &manifest_hash,
                    vessel_snapshot.as_ref(), self.config.audit_signer.as_deref(),
                );
            }

            let mut llm_output = result.output.clone();
            if allowed {
                if let Some(obj) = llm_output.as_object_mut() {
                    obj.remove("_vessel_snapshot");
                }
                rep_succeeded += 1;
            }

            messages.push(ChatMessage {
                role: "user".into(),
                content: if allowed {
                    format!(
                        "Step {i} ({} — {}): SUCCESS. Result: {}",
                        step.tool, step.description,
                        serde_json::to_string(&llm_output).unwrap_or_default()
                    )
                } else {
                    format!(
                        "Step {i} ({} — {}): REJECTED. Reason: {}",
                        step.tool, step.description,
                        rejection_reason.as_deref().unwrap_or("unknown")
                    )
                },
            });

            all_step_results.push(StepOutcome {
                step_index: i,
                replicate_index: recovery.replicate_index,
                tool: step.tool.clone(),
                description: step.description.clone(),
                allowed,
                result: if allowed { Some(llm_output) } else { None },
                rejection_reason,
            });
        }

        let steps_succeeded = rep_succeeded;
        messages.push(ChatMessage {
            role: "user".into(),
            content: format!(
                "Protocol resumed and completed. {steps_succeeded}/{} remaining steps succeeded. \
                 Write your scientific conclusion based on all observations.",
                step_count - resume_from
            ),
        });

        let conclusion = match self.llm.chat(&messages, self.config.reasoning_temperature).await {
            Ok(text) => sanitize_llm_response(&text),
            Err(e) => {
                warn!(error = %e, "LLM failed to generate conclusion for resumed protocol");
                format!(
                    "Protocol '{}' resumed: {steps_succeeded}/{} steps succeeded. \
                     LLM conclusion unavailable: {e}",
                    protocol.name, step_count - resume_from
                )
            }
        };

        let mut rekor_status = RekorStatus::Skipped;
        if let Some(path) = &self.config.audit_log_path {
            let conclusion_line = emit_protocol_conclusion(
                path, protocol.id, run_id, &protocol.name, &conclusion,
                step_count, steps_succeeded,
                protocol.template_id.as_deref(), self.config.audit_signer.as_deref(),
            );
            if let (Ok(line), Some(signer)) = (conclusion_line, self.config.audit_signer.as_deref()) {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let (Some(hash), Some(sig)) = (
                        entry["entry_hash"].as_str(), entry["entry_sig_b64"].as_str(),
                    ) {
                        let pubkey_pem = rekor::ed25519_pubkey_pem(&signer.verifying_key_bytes());
                        match rekor::submit_with_retry(hash, sig, &pubkey_pem).await {
                            Ok(uuid) => {
                                info!(rekor_uuid = %uuid, "resumed protocol anchored to Rekor");
                                rekor_status = RekorStatus::Anchored { uuid };
                            }
                            Err(reason) => {
                                error!(error = %reason, "Rekor anchoring failed for resumed protocol");
                                rekor_status = RekorStatus::Failed { reason };
                            }
                        }
                    }
                }
            }
        }

        // Spawn background ZK proof task.
        let zk_proof_status = self.spawn_zk_proof_if_configured(&self.config.audit_log_path);

        let uncertainty_budgets = self.build_uncertainty_budgets(&all_step_results);
        let doe_anova = protocol.doe_design_json.as_deref()
            .and_then(|json| self.run_doe_anova(json, &all_step_results));

        ProtocolRunResult {
            protocol_id: protocol.id,
            run_id,
            protocol_name: protocol.name.clone(),
            steps_total: step_count,
            steps_succeeded,
            conclusion,
            step_results: all_step_results,
            replicate_count: total_replicates as u32,
            aggregate: None,
            rekor_status,
            zk_proof_status,
            uncertainty_budgets,
            doe_anova,
        }
    }

    /// Run one-way ANOVA on protocol step responses grouped by the first DoE factor level.
    ///
    /// Parses `doe_design_json` as a `DoeDesign`, pairs each run row with the
    /// corresponding allowed step result (in order), groups the numeric response
    /// values by the first factor's level bracket (low ≤ mid vs high > mid),
    /// then calls `anova_one_way`.  Returns `None` if the design cannot be parsed,
    /// there are insufficient data points, or ANOVA fails for any reason.
    fn run_doe_anova(
        &self,
        doe_design_json: &str,
        step_results: &[crate::protocol::StepOutcome],
    ) -> Option<crate::protocol::DoeAnovaResult> {
        use scientific_compute::doe::DoeDesign;
        use scientific_compute::stats::anova_one_way;

        let design: DoeDesign = serde_json::from_str(doe_design_json).ok()?;
        if design.factors.is_empty() || design.runs.is_empty() {
            return None;
        }

        // Collect numeric responses from allowed steps, in order.
        let responses: Vec<f64> = step_results.iter()
            .filter(|s| s.allowed)
            .filter_map(|s| {
                let result = s.result.as_ref()?;
                result.as_object()?.iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .find_map(|(_, v)| v.as_f64())
            })
            .collect();

        // Pair responses with DoE run rows (zip stops at the shorter).
        let first_factor = &design.factors[0];
        let mid = (first_factor.low + first_factor.high) / 2.0;

        // Group responses by first factor level (low ≤ mid → group 0, else group 1).
        let n_groups = 2usize;
        let mut groups: Vec<Vec<f64>> = vec![Vec::new(); n_groups];
        for (run, response) in design.runs.iter().zip(responses.iter()) {
            let level = run.get(&first_factor.name).copied().unwrap_or(first_factor.low);
            let group_idx = if level <= mid { 0 } else { 1 };
            groups[group_idx].push(*response);
        }

        // Need ≥2 observations per group.
        if groups.iter().any(|g| g.len() < 2) {
            return None;
        }

        let result = anova_one_way(&groups).ok()?;
        let n_total: usize = groups.iter().map(|g| g.len()).sum();
        Some(crate::protocol::DoeAnovaResult {
            f_statistic: result.f_statistic,
            p_value:     result.p_value,
            n_groups,
            n_total,
        })
    }

    /// Build GUM-compliant uncertainty budgets from step outcomes.
    ///
    /// Iterates over allowed steps that have a registered `InstrumentUncertainty`
    /// and extracts the primary numeric reading from the tool result JSON.
    /// Returns one budget per unique (tool, parameter) pair.
    fn build_uncertainty_budgets(&self, step_results: &[crate::protocol::StepOutcome]) -> Vec<crate::protocol::UncertaintyBudget> {
        use crate::protocol::UncertaintyBudget;

        let mut budgets: Vec<UncertaintyBudget> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for step in step_results {
            if !step.allowed { continue; }
            let Some(result) = &step.result else { continue; };
            let Some(spec) = self.tools.spec_for(&step.tool) else { continue; };
            let Some(unc) = &spec.instrument_uncertainty else { continue; };

            // Extract the first numeric value from the result object.
            // Prefer keys that don't start with `_` (which are metadata).
            let Some(reading) = result.as_object().and_then(|obj| {
                obj.iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .find_map(|(_, v)| v.as_f64())
            }) else { continue; };

            let key = format!("{}:{}", step.tool, unc.unit);
            if seen.contains(&key) { continue; }
            seen.insert(key);

            budgets.push(UncertaintyBudget::from_instrument_uncertainty(
                step.tool.replace('_', " "),
                unc.unit.clone(),
                reading,
                unc.u_type_a_fraction,
                unc.u_type_b_abs,
            ));
        }
        budgets
    }

    /// Attempt to parse a tool call from the LLM response, validate it
    /// against the sandbox, approval policy, capability bounds, and proof
    /// artifacts, then dispatch it.
    async fn try_tool_call(&self, response: &str, approval_ctx: Option<ApprovalContext>, reasoning_text: Option<String>) -> Option<ToolResult> {
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
            self.audit_decision(tool_name, "deny", &e.to_string(), false, None, reasoning_text.clone()).await;
            self.emit_tool_event(tool_name, &params, "rejected", &e.to_string());
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(e.to_string()),
                success: false,
            metadata: None,
            });
        }

        // ── Stage 0.1: Calibration check ─────────────────────────────────
        // Only active when calibration_status is populated by the server.
        // Quantitative sensor tools without any calibration record are denied.
        // Expired calibrations emit a warning but do not block execution.
        const QUANTITATIVE_TOOLS: &[&str] = &["read_ph", "read_absorbance"];
        const CALIBRATION_TOOLS:   &[&str] = &["read_ph", "read_absorbance", "read_temperature", "read_sensor"];

        if !self.config.calibration_status.is_empty()
            && CALIBRATION_TOOLS.contains(&tool_name)
        {
            match self.config.calibration_status.get(tool_name) {
                Some((true, false)) => {
                    // Calibrated but expired → warn, continue.
                    let msg = format!("calibration for '{tool_name}' has expired — readings may be inaccurate");
                    warn!(%msg);
                    self.audit_decision(tool_name, "allow", &msg, false, None, reasoning_text.clone()).await;
                }
                None | Some((false, _)) if QUANTITATIVE_TOOLS.contains(&tool_name) => {
                    // No calibration at all for quantitative tool → deny.
                    let msg = format!(
                        "calibration_required: '{tool_name}' has no calibration record. \
                         Run calibrate_ph / calibrate_spectrophotometer before taking readings."
                    );
                    warn!(%msg);
                    self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                    self.emit_tool_event(tool_name, &params, "rejected", &msg);
                    return Some(ToolResult {
                        name: tool_name.to_owned(),
                        output: serde_json::Value::String(msg),
                        success: false,
                        metadata: None,
                    });
                }
                _ => {} // Valid calibration or non-quantitative tool — proceed.
            }
        }

        // ── Stage 0.5: Tool parameter schema validation ───────────────────
        if let Some(schema) = self.tools.schema_for(tool_name) {
            if !schema.is_null() {
                match jsonschema::JSONSchema::compile(schema) {
                    Ok(compiled) => {
                        if let Err(errors) = compiled.validate(&params) {
                            let msg = errors
                                .map(|e: jsonschema::ValidationError<'_>| e.to_string())
                                .collect::<Vec<_>>()
                                .join("; ");
                            warn!(tool = tool_name, %msg, "params failed schema validation");
                            self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                            self.emit_tool_event(tool_name, &params, "rejected", &msg);
                            return Some(ToolResult {
                                name: tool_name.to_owned(),
                                output: serde_json::Value::String(format!(
                                    "parameter validation failed: {msg}"
                                )),
                                success: false,
                                metadata: None,
                            });
                        }
                    }
                    Err(e) => {
                        warn!(tool = tool_name, error = %e, "tool has malformed schema — skipping validation");
                    }
                }
            }
        }

        let risk_class = risk_class_for_action(tool_name, &self.action_risk_index);

        // ── Stage 0.25: Chemical compatibility ────────────────────────────
        // Only active for liquid-handling tools.  Checks the explicit reagent
        // parameter(s) in the call against the GHS/NFPA 704 incompatibility
        // table.  Vessel-contents cross-check is a no-op until Phase 2B
        // (LabState) populates it.
        if matches!(tool_name, "dispense" | "aspirate") {
            let reagent = params
                .get("reagent")
                .or_else(|| params.get("reagent_id"))
                .or_else(|| params.get("pump_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let target = params
                .get("vessel_id")
                .or_else(|| params.get("target_vessel"))
                .or_else(|| params.get("source_vessel"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Check the reagent name against itself and any explicit vessel contents
            // supplied in the params (Phase 2B will inject vessel_contents here).
            if let Some(existing_contents) = params
                .get("vessel_contents")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str())
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
            {
                let compat = crate::chemistry::global();
                if let Err(e) = compat.check_vessel_addition(&existing_contents, reagent) {
                    let msg = format!("chemical_compatibility_violation: {e}");
                    self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                    self.emit_tool_event(tool_name, &params, "rejected", &msg);
                    return Some(ToolResult {
                        name: tool_name.to_owned(),
                        output: serde_json::Value::String(msg),
                        success: false,
                        metadata: None,
                    });
                }
            } else if !reagent.is_empty() && !target.is_empty() {
                // No vessel contents provided — at minimum check the reagent
                // against the target identifier (catches obvious self-mixes).
                let compat = crate::chemistry::global();
                if let Err(e) = compat.check(reagent, target) {
                    let msg = format!("chemical_compatibility_violation: {e}");
                    self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                    self.emit_tool_event(tool_name, &params, "rejected", &msg);
                    return Some(ToolResult {
                        name: tool_name.to_owned(),
                        output: serde_json::Value::String(msg),
                        success: false,
                        metadata: None,
                    });
                }
            }
        }

        // Tracks the approval queue ID when an operator approves an interactive
        // action.  Set in the approval path below; consumed at stage 5 to emit
        // WAL entries and purge the sidecar after a confirmed dispatch.
        let mut dispatched_pending_id: Option<String> = None;

        // ── Stage 1: Two-person approval ──────────────────────────────────
        if let (Some(policy), Some(ctx)) = (&self.config.approval_policy, &self.policy_context) {
            let needs_approval = crate::approvals::requires_two_person_approval(risk_class.clone());
            let has_bundle = params
                .get("approval_bundle")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);

            if needs_approval && !has_bundle {
                if let Some(queue) = &self.config.approval_queue {
                    // ── Interactive approval path ──────────────────────────
                    let mut actx = approval_ctx.unwrap_or_default();
                    actx.risk_class = risk_class.as_ref().map(|r| format!("{r:?}"));
                    let (pending_id, rx) = queue.enqueue(
                        tool_name,
                        params.clone(),
                        self.config.session_nonce.clone(),
                        actx,
                    );
                    info!(
                        %pending_id,
                        tool = tool_name,
                        timeout_secs = self.config.approval_timeout_secs,
                        "high-risk action queued — waiting for operator approval"
                    );

                    let bundle_result = tokio::time::timeout(
                        tokio::time::Duration::from_secs(self.config.approval_timeout_secs),
                        rx,
                    )
                    .await;

                    // Remove from the in-memory map; keep the sidecar file as a
                    // WAL marker until dispatch_complete is confirmed at stage 5.
                    // On denied/timeout/error paths below, the sidecar is removed
                    // via queue.remove() which the error arms call explicitly.
                    queue.dequeue_approved(&pending_id);
                    dispatched_pending_id = Some(pending_id.clone());

                    // Determine the augmented params (with bundle injected) or deny.
                    let augmented = match bundle_result {
                        Err(_elapsed) => {
                            let msg = format!(
                                "approval timeout: '{tool_name}' waited {}s with no operator response",
                                self.config.approval_timeout_secs
                            );
                            warn!(%pending_id, %msg);
                            self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                            self.emit_tool_event(tool_name, &params, "rejected", &msg);
                            queue.purge_sidecar(&pending_id);
                            return Some(ToolResult {
                                name: tool_name.to_owned(),
                                output: serde_json::Value::String(msg),
                                success: false,
                            metadata: None,
            });
                        }
                        Ok(Err(_recv_err)) => {
                            let msg = "approval cancelled: server shutting down";
                            self.audit_decision(tool_name, "deny", msg, false, None, reasoning_text.clone()).await;
                            queue.purge_sidecar(&pending_id);
                            return Some(ToolResult {
                                name: tool_name.to_owned(),
                                output: serde_json::Value::String(msg.into()),
                                success: false,
                            metadata: None,
            });
                        }
                        Ok(Ok(None)) => {
                            let msg = format!("operator denied action '{tool_name}'");
                            warn!(%pending_id, %msg);
                            self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                            self.emit_tool_event(tool_name, &params, "rejected", &msg);
                            queue.purge_sidecar(&pending_id);
                            return Some(ToolResult {
                                name: tool_name.to_owned(),
                                output: serde_json::Value::String(msg),
                                success: false,
                            metadata: None,
            });
                        }
                        Ok(Ok(Some(bundle))) => {
                            let mut p = params.clone();
                            p["approval_bundle"] = serde_json::to_value(&bundle)
                                .unwrap_or(serde_json::Value::Array(vec![]));
                            p
                        }
                    };

                    // Validate the submitted bundle with the full policy.
                    match policy.validate_action(
                        tool_name,
                        risk_class.clone(),
                        ctx,
                        &augmented,
                        self.config.session_nonce.as_deref(),
                    ) {
                        Ok(approval_ids) => {
                            for aid in &approval_ids {
                                if self.config.revocation_list.is_approval_revoked(aid) {
                                    let msg = format!("approval {aid} has been revoked");
                                    self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                                    queue.purge_sidecar(&pending_id);
                                    return Some(ToolResult {
                                        name: tool_name.to_owned(),
                                        output: serde_json::Value::String(msg),
                                        success: false,
                                    metadata: None,
            });
                                }
                            }
                            let reason = format!(
                                "interactive approval satisfied (pending_id={pending_id}, \
                                 approval_ids={})",
                                approval_ids.join(",")
                            );
                            self.audit_decision(tool_name, "allow", &reason, true, Some(approval_ids), None).await;
                            // Fall through to Stage 2 with original params (no bundle injected).
                        }
                        Err(e) => {
                            self.audit_decision(tool_name, "deny", &e, false, None, reasoning_text.clone()).await;
                            self.emit_tool_event(tool_name, &params, "rejected", &e);
                            queue.purge_sidecar(&pending_id);
                            return Some(ToolResult {
                                name: tool_name.to_owned(),
                                output: serde_json::Value::String(e),
                                success: false,
                            metadata: None,
            });
                        }
                    }
                } else {
                    // ── No queue configured: instant deny ─────────────────
                    let e = format!(
                        "approval violation: '{tool_name}' requires signed approvals \
                         (no interactive approval queue configured)"
                    );
                    self.audit_decision(tool_name, "deny", &e, false, None, reasoning_text.clone()).await;
                    self.emit_tool_event(tool_name, &params, "rejected", &e);
                    return Some(ToolResult {
                        name: tool_name.to_owned(),
                        output: serde_json::Value::String(e),
                        success: false,
                    metadata: None,
            });
                }
            } else if has_bundle {
                // ── Pre-signed bundle path (unchanged) ────────────────────
                match policy.validate_action(
                    tool_name,
                    risk_class.clone(),
                    ctx,
                    &params,
                    self.config.session_nonce.as_deref(),
                ) {
                    Ok(approval_ids) => {
                        for aid in &approval_ids {
                            if self.config.revocation_list.is_approval_revoked(aid) {
                                let msg = format!("approval {aid} has been revoked");
                                self.audit_decision(tool_name, "deny", &msg, false, None, reasoning_text.clone()).await;
                                return Some(ToolResult {
                                    name: tool_name.to_owned(),
                                    output: serde_json::Value::String(msg),
                                    success: false,
                                metadata: None,
            });
                            }
                        }
                        if !approval_ids.is_empty() {
                            let reason = format!(
                                "two-person approval satisfied (approval_ids={})",
                                approval_ids.join(",")
                            );
                            self.audit_decision(tool_name, "allow", &reason, true, Some(approval_ids), None).await;
                        }
                    }
                    Err(e) => {
                        self.audit_decision(tool_name, "deny", &e, false, None, reasoning_text.clone()).await;
                        self.emit_tool_event(tool_name, &params, "rejected", &e);
                        return Some(ToolResult {
                            name: tool_name.to_owned(),
                            output: serde_json::Value::String(e),
                            success: false,
                        metadata: None,
            });
                    }
                }
            }
            // !needs_approval → fall through to Stage 2.
        }

        // ── Stage 2: Capability bounds ────────────────────────────────────
        if let Some(capability) = &self.config.capability_policy {
            let param_units = self.tools.spec_for(tool_name).map(|s| &s.parameter_units);
            if let Err(e) = capability.validate(tool_name, &params, param_units) {
                self.audit_decision(tool_name, "deny", &e, false, None, reasoning_text.clone()).await;
                self.emit_tool_event(tool_name, &params, "rejected", &e);
                return Some(ToolResult {
                    name: tool_name.to_owned(),
                    output: serde_json::Value::String(e),
                    success: false,
                metadata: None,
            });
            }
        }

        // ── Stage 3: Fail-closed for high-risk without policy ─────────────
        let high_risk = matches!(risk_class, Some(RiskClass::Actuation | RiskClass::Destructive));
        if high_risk && (self.policy_engine.is_none() || self.policy_context.is_none()) {
            let msg = "high-risk action denied: runtime proof policy is not configured";
            self.audit_decision(tool_name, "deny", msg, false, None, reasoning_text.clone()).await;
            self.emit_tool_event(tool_name, &params, "rejected", msg);
            return Some(ToolResult {
                name: tool_name.to_owned(),
                output: serde_json::Value::String(msg.into()),
                success: false,
            metadata: None,
            });
        }

        // ── Stage 4: Proof-artifact policy ───────────────────────────────
        if let (Some(engine), Some(ctx)) = (&self.policy_engine, &self.policy_context) {
            if let Err(e) = engine.authorize(tool_name, ctx) {
                let report = engine.explain(tool_name);
                self.audit_decision(tool_name, "deny", &report.reason, false, None, reasoning_text.clone()).await;
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
                    metadata: None,
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
            reasoning_text,
        )
        .await;

        self.emit_tool_event(tool_name, &params, "success", "policy and sandbox checks passed");

        // WAL: emit pending_dispatch before dispatching an operator-approved action.
        // If the process crashes after this point but before dispatch_complete is written,
        // the sidecar file on disk marks the action as stalled for recovery on restart.
        if let Some(ref aid) = dispatched_pending_id {
            if let Some(path) = &self.config.audit_log_path {
                crate::audit::emit_pending_dispatch(
                    path, aid, tool_name, &params, self.config.audit_signer.as_deref(),
                ).ok();
            }
        }

        let call = ToolCall {
            name: tool_name.to_owned(),
            params,
        };
        let result = self.tools.dispatch(&call).await;

        // WAL: emit dispatch_complete and purge sidecar after a successful dispatch.
        if let Some(ref aid) = dispatched_pending_id {
            if let Some(path) = &self.config.audit_log_path {
                crate::audit::emit_dispatch_complete(
                    path, aid, tool_name, self.config.audit_signer.as_deref(),
                ).ok();
            }
            if let Some(queue) = &self.config.approval_queue {
                queue.purge_sidecar(aid);
            }
        }

        Some(result)
    }

    /// Spawn a background task that generates a ZK audit proof and submits it
    /// to Base L2.  Returns the initial status (`Pending` or `Disabled`).
    ///
    /// The task logs its outcome at INFO/ERROR but does not update
    /// `ProtocolRunResult` in place — callers use the event bus for that.
    fn spawn_zk_proof_if_configured(
        &self,
        audit_log_path: &Option<String>,
    ) -> ZkProofStatus {
        // Require the audit log path and Base L2 config.
        let path = match audit_log_path.clone() {
            Some(p) => p,
            None => return ZkProofStatus::Disabled,
        };

        let cfg = match zk_audit::ZkConfig::from_env() {
            Some(c) => c,
            None => {
                info!(
                    "AXIOMLAB_BASE_RPC_URL not set — ZK audit proof disabled. \
                     Set BASE_RPC_URL, BASE_CONTRACT_ADDR, and BASE_WALLET_KEY to enable."
                );
                return ZkProofStatus::Disabled;
            }
        };

        tokio::spawn(async move {
            match zk_audit::prove_and_submit(&path, &cfg).await {
                Ok(tx) => {
                    info!(
                        tx_hash = %tx,
                        "ZK audit proof submitted to Base — https://basescan.org/tx/{tx}"
                    );
                }
                Err(e) => {
                    error!(error = %e, "ZK audit proof submission failed");
                }
            }
        });

        ZkProofStatus::Pending
    }

    async fn audit_decision(
        &self,
        action: &str,
        decision: &str,
        reason: &str,
        success: bool,
        approval_ids: Option<Vec<String>>,
        reasoning_text: Option<String>,
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
            reasoning_text,
        };

        let mut payload_line = serde_json::to_string(&event).unwrap_or_default();

        if let Some(path) = &self.config.audit_log_path {
            match emit_jsonl(path, &event, self.config.audit_signer.as_deref()) {
                Ok(line) => payload_line = line,
                Err(e) => warn!(error = %e, "failed to write local audit event"),
            }
        }

        if let Err(e) = emit_remote_with_retry(&payload_line).await {
            warn!(error = %e, "failed to mirror audit event to remote sink");
        }
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

/// Extract the LLM chain-of-thought that precedes the JSON tool call.
///
/// The LLM sometimes emits prose reasoning before the JSON object, e.g.:
/// ```text
/// I need to check the pH before dispensing. {"tool": "read_ph", ...}
/// ```
/// This function returns the trimmed text before the first `{`, or `None` if
/// the response starts directly with JSON or the prefix is whitespace-only.
///
/// The extracted text is stored in the audit log so every decision has a
/// cryptographically bound rationale — it cannot be swapped after the fact.
fn extract_reasoning(response: &str) -> Option<String> {
    let brace_pos = response.find('{')?;
    if brace_pos == 0 {
        return None;
    }
    let prefix = response[..brace_pos].trim();
    if prefix.is_empty() {
        None
    } else {
        // Truncate to 2 KiB to keep audit entries reasonable.
        let truncated: String = prefix.chars().take(2048).collect();
        Some(truncated)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmError;
    use crate::sandbox::{ResourceLimits, Sandbox};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A scripted mock LLM that pops responses from a queue in order.
    /// Panics if the queue is exhausted (test bug, not prod bug).
    struct ScriptedLlm {
        responses: Mutex<VecDeque<String>>,
    }

    impl ScriptedLlm {
        fn new(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(|s| s.into()).collect()),
            }
        }
    }

    impl crate::llm::LlmBackend for ScriptedLlm {
        async fn chat(
            &self,
            _messages: &[crate::llm::ChatMessage],
            _temperature: f64,
        ) -> Result<String, LlmError> {
            let mut q = self.responses.lock().unwrap();
            Ok(q.pop_front().expect("ScriptedLlm: response queue exhausted"))
        }
    }

    fn minimal_sandbox() -> Sandbox {
        Sandbox::new(
            vec![PathBuf::from("/lab/workspace")],
            vec!["move_arm".into(), "read_sensor".into()],
            ResourceLimits::default(),
        )
    }

    fn minimal_config() -> OrchestratorConfig {
        OrchestratorConfig {
            max_iterations: 20,
            audit_log_path: None,
            capability_policy: None,
            approval_policy: None,
            session_nonce: None,
            audit_signer: None,
            revocation_list: crate::revocation::RevocationList::default(),
            event_sink: None,
            ..OrchestratorConfig::default()
        }
    }

    /// LLM returns garbage twice (below threshold), then a valid done signal.
    /// Experiment must complete successfully.
    #[tokio::test]
    async fn recovers_after_parse_failures_below_threshold() {
        let llm = ScriptedLlm::new([
            "this is not JSON at all",
            "also not JSON",
            r#"{"done": true, "summary": "all good"}"#,
        ]);
        let orchestrator = Orchestrator::new(
            llm,
            minimal_sandbox(),
            ToolRegistry::new(),
            minimal_config(),
        );
        let mut exp = crate::experiment::Experiment::new("test-1", "recovery test");
        let result = orchestrator.run_experiment(&mut exp).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(exp.stage, crate::experiment::Stage::Completed);
    }

    /// LLM returns garbage >= MAX_CONSECUTIVE_PARSE_FAILURES times.
    /// Experiment must fail with Halted error.
    #[tokio::test]
    async fn fails_after_too_many_consecutive_parse_failures() {
        let garbage: Vec<String> = (0..MAX_CONSECUTIVE_PARSE_FAILURES as usize)
            .map(|i| format!("garbage response #{i}"))
            .collect();
        let llm = ScriptedLlm::new(garbage);
        let orchestrator = Orchestrator::new(
            llm,
            minimal_sandbox(),
            ToolRegistry::new(),
            minimal_config(),
        );
        let mut exp = crate::experiment::Experiment::new("test-2", "failure test");
        let result = orchestrator.run_experiment(&mut exp).await;
        assert!(matches!(result, Err(OrchestratorError::Halted(_))));
        assert_eq!(exp.stage, crate::experiment::Stage::Failed);
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
