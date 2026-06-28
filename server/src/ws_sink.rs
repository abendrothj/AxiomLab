use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, ProtocolConclusionEvent, ProtocolStepEvent,
    StateTransitionEvent, ToolExecutionEvent,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use crate::db::Db;
use crate::discovery::{journal_path, DiscoveryJournal};

// ── Loop status (pacing transparency) ─────────────────────────────────────────

/// Live, viewer-facing description of what the execution loop is doing between
/// experiments. Broadcast over WS as `loop_status` and seeded into the snapshot
/// so a freshly loaded page immediately shows any active wait.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LoopStatus {
    /// "running" | "paused" | "idle" | "backoff" | "converged"
    pub phase:        String,
    /// Human-readable explanation of the current phase.
    pub reason:       String,
    /// Total length of the current wait, in seconds (0 when running).
    pub wait_secs:    u64,
    /// Unix epoch (ms) when the next experiment is expected to start (0 when running).
    pub resume_at_ms: i64,
}

impl Default for LoopStatus {
    fn default() -> Self {
        Self {
            phase:        "running".into(),
            reason:       "Starting up".into(),
            wait_secs:    0,
            resume_at_ms: 0,
        }
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Exploration log ───────────────────────────────────────────────────────────

#[derive(Default)]
pub struct ExplorationLog {
    pub findings:   Vec<String>,
    pub rejections: Vec<(String, String)>, // (tool, reason)
    pub successes:  Vec<String>,
}

impl ExplorationLog {
    /// Seed findings from a persisted `DiscoveryJournal` so the mandate
    /// correctly shows "already discovered" after a server restart.
    pub fn from_journal(journal: &DiscoveryJournal) -> Self {
        Self {
            findings: journal.findings.iter().map(|f| f.statement.clone()).collect(),
            ..Default::default()
        }
    }
}

// ── In-memory event buffer (replaces SQLite) ──────────────────────────────────

const MAX_EVENTS: usize = 2000;

#[derive(Clone, Default)]
pub struct EventBuffer {
    inner: Arc<Mutex<BufferInner>>,
}

#[derive(Default)]
struct BufferInner {
    notebook:    VecDeque<serde_json::Value>,
    transitions: VecDeque<serde_json::Value>,
    tools:       VecDeque<serde_json::Value>,
}

impl EventBuffer {
    fn push(queue: &mut VecDeque<serde_json::Value>, v: serde_json::Value) {
        if queue.len() >= MAX_EVENTS {
            queue.pop_front();
        }
        queue.push_back(v);
    }

    pub fn push_notebook(&self, v: serde_json::Value) {
        if let Ok(mut g) = self.inner.lock() { Self::push(&mut g.notebook, v); }
    }
    pub fn push_transition(&self, v: serde_json::Value) {
        if let Ok(mut g) = self.inner.lock() { Self::push(&mut g.transitions, v); }
    }
    pub fn push_tool(&self, v: serde_json::Value) {
        if let Ok(mut g) = self.inner.lock() { Self::push(&mut g.tools, v); }
    }

    /// Returns (notebook, transitions, tools) as owned Vecs (oldest-first).
    pub fn snapshot(&self) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, Vec<serde_json::Value>) {
        match self.inner.lock() {
            Ok(g) => (
                g.notebook.iter().cloned().collect(),
                g.transitions.iter().cloned().collect(),
                g.tools.iter().cloned().collect(),
            ),
            Err(_) => (vec![], vec![], vec![]),
        }
    }
}

// ── Server-side event sink ────────────────────────────────────────────────────

/// Broadcasts all orchestrator events to every connected WebSocket client,
/// buffers them in memory for /api/history, and persists protocol conclusions
/// to the operation log (JSON + SQLite).
pub struct WebSocketSink {
    pub tx:       broadcast::Sender<String>,
    pub log:      Arc<Mutex<ExplorationLog>>,
    pub notebook: Arc<Mutex<Vec<serde_json::Value>>>,
    pub events:   EventBuffer,
    pub journal:  Arc<Mutex<DiscoveryJournal>>,
    /// SQLite dual-write target.
    pub db:       Arc<Db>,
    /// Shared loop-pacing status (also read by /api/status + the WS snapshot).
    pub loop_status: Arc<Mutex<LoopStatus>>,
}

impl WebSocketSink {
    fn broadcast(&self, event: &str, payload: impl serde::Serialize) {
        let msg = serde_json::json!({ "event": event, "payload": payload });
        self.tx.send(msg.to_string()).ok();
    }

    /// Record + broadcast that the loop is pausing for `wait_secs` before the
    /// next experiment. `wait_secs == 0` signals the loop is actively running.
    pub fn set_loop_status(&self, phase: &str, reason: impl Into<String>, wait_secs: u64) {
        let status = LoopStatus {
            phase:        phase.to_owned(),
            reason:       reason.into(),
            wait_secs,
            resume_at_ms: if wait_secs == 0 { 0 } else { now_ms() + (wait_secs as i64) * 1000 },
        };
        if let Ok(mut g) = self.loop_status.lock() {
            *g = status.clone();
        }
        self.broadcast("loop_status", &status);
    }
}

impl EventSink for WebSocketSink {
    fn on_state_transition(&self, event: StateTransitionEvent) {
        self.events.push_transition(serde_json::to_value(&event).unwrap_or_default());
        self.broadcast("state_transition", &event);
    }

    fn on_tool_execution(&self, event: ToolExecutionEvent) {
        {
            let mut log = self.log.lock().unwrap();
            if event.status == "rejected" {
                log.rejections.push((event.tool.clone(), event.reason.clone()));
            } else {
                log.successes.push(event.tool.clone());
            }
        }
        self.events.push_tool(serde_json::to_value(&event).unwrap_or_default());
        self.broadcast("tool_execution", &event);
    }

    fn on_llm_token(&self, event: LlmTokenEvent) {
        // LLM tokens are not buffered — too high volume, no analytical value.
        self.broadcast("llm_token", &event);
    }

    fn on_notebook_entry(&self, event: NotebookEntryEvent) {
        {
            let mut log = self.log.lock().unwrap();
            log.findings.push(event.entry.clone());
        }
        {
            let mut nb = self.notebook.lock().unwrap();
            nb.push(serde_json::to_value(&event).unwrap_or_default());
        }
        self.events.push_notebook(serde_json::to_value(&event).unwrap_or_default());
        self.broadcast("notebook_entry", &event);
    }

    fn on_protocol_step(&self, event: ProtocolStepEvent) {
        self.broadcast("protocol_step", &event);
    }

    fn on_protocol_conclusion(&self, event: ProtocolConclusionEvent) {
        // Persist to operation log (JSON + SQLite).
        {
            let mut journal = self.journal.lock().unwrap();
            journal.record_run(
                &event.run_id,
                &event.protocol_name,
                // The conclusion event doesn't carry the hypothesis; we use the protocol name
                // as a placeholder — the LLM can add a proper hypothesis via update_journal.
                &event.protocol_name,
                &event.conclusion,
                event.steps_succeeded,
                event.steps_total,
            );
            // Dual-write: SQLite row for the run just recorded.
            if let Some(run) = journal.runs.last() {
                self.db.insert_run(run);
            }
            let path = journal_path();
            if let Err(e) = journal.save(&path) {
                tracing::warn!("Failed to save operation log: {e}");
            }
        }

        // Also push to in-memory notebook buffer so it appears in /api/history.
        let entry = serde_json::json!({
            "type": "protocol_conclusion",
            "protocol_name": event.protocol_name,
            "conclusion": event.conclusion,
            "steps_succeeded": event.steps_succeeded,
            "steps_total": event.steps_total,
            "timestamp_ms": event.timestamp_ms,
        });
        self.events.push_notebook(entry);

        self.broadcast("protocol_conclusion", &event);
    }
}
