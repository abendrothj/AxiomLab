//! Event sink abstraction for the orchestrator.
//!
//! Consumers (e.g. the Tauri visualizer) implement [`EventSink`] and inject it
//! into [`OrchestratorConfig`] to receive live events from the agent loop.

use serde::Serialize;

// ── Event payloads ────────────────────────────────────────────────

/// Fired when the experiment advances to a new [`Stage`].
#[derive(Clone, Serialize)]
pub struct StateTransitionEvent {
    /// The stage name before the transition (empty string for the initial state).
    pub from: String,
    /// The stage name after the transition.
    pub to: String,
    /// The experiment being run.
    pub experiment_id: String,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// Fired after the orchestrator's validation pipeline decides on a tool call.
#[derive(Clone, Serialize)]
pub struct ToolExecutionEvent {
    /// Name of the tool (e.g. `"dispense"`, `"move_arm"`).
    pub tool: String,
    /// Primary target identifier extracted from params (pump_id, sensor_id, etc.).
    pub target: String,
    /// Full tool params as JSON.
    pub params: serde_json::Value,
    /// Capability-policy upper bound for the primary numeric parameter (0.0 if N/A).
    pub max_safe_limit: f64,
    /// `"success"` if all pipeline stages passed, `"rejected"` if any stage denied.
    pub status: String,
    /// Human-readable reason for the outcome.
    pub reason: String,
}

/// Fired for each character of the LLM response, after it is received.
///
/// Emitting character-by-character at ~5 ms intervals creates the appearance
/// of real-time streaming in the UI without requiring SSE from the LLM provider.
#[derive(Clone, Serialize)]
pub struct LlmTokenEvent {
    /// A single character (or a small chunk) of the LLM response.
    pub token: String,
}

/// Fired when the AI documents a finding in its Lab Notebook.
///
/// Emitted after the `Analysing → Completed` transition when a `"done": true`
/// summary is parsed from the LLM response.
#[derive(Clone, Serialize)]
pub struct NotebookEntryEvent {
    /// The experiment this finding belongs to.
    pub experiment_id: String,
    /// The AI's prose finding / conclusion.
    pub entry: String,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Name of the tool call that triggered this analysis.
    pub tool_that_triggered: String,
    /// Outcome classification.
    /// - `"discovery"` — a novel relationship or interaction was found
    /// - `"rejection"` — the proof engine rejected a hypothesis
    /// - `"inconclusive"` — ambiguous result; more data needed
    pub outcome: String,
}

// ── Trait ─────────────────────────────────────────────────────────

/// Synchronous event sink for the orchestrator.
///
/// Implementations must be `Send + Sync` because the orchestrator runs inside
/// a Tokio task. All methods are synchronous fire-and-forget; the sink should
/// not block — use internal queues or channels if back-pressure is needed.
pub trait EventSink: Send + Sync {
    fn on_state_transition(&self, event: StateTransitionEvent);
    fn on_tool_execution(&self, event: ToolExecutionEvent);
    fn on_llm_token(&self, event: LlmTokenEvent);
    fn on_notebook_entry(&self, event: NotebookEntryEvent);
}
