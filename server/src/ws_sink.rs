use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
use crate::db::EventDb;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

// ── Exploration log (same as Tauri version) ───────────────────────────────────

#[derive(Default)]
pub struct ExplorationLog {
    pub findings:  Vec<String>,
    pub rejections: Vec<(String, String)>, // (tool, reason)
    pub successes:  Vec<String>,
}

// ── Server-side event sink ────────────────────────────────────────────────────

/// Broadcasts all orchestrator events to every connected WebSocket client
/// and records each event immutably in the SQLite event log.
pub struct WebSocketSink {
    pub tx:       broadcast::Sender<String>,
    pub log:      Arc<Mutex<ExplorationLog>>,
    pub notebook: Arc<Mutex<Vec<serde_json::Value>>>,
    pub db:       EventDb,
}

impl WebSocketSink {
    fn broadcast(&self, event: &str, payload: impl serde::Serialize) {
        let msg = serde_json::json!({ "event": event, "payload": payload });
        self.tx.send(msg.to_string()).ok();
    }
}

impl EventSink for WebSocketSink {
    fn on_state_transition(&self, event: StateTransitionEvent) {
        self.db.record("state_transition", &event);
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
        self.db.record("tool_execution", &event);
        self.broadcast("tool_execution", &event);
    }

    fn on_llm_token(&self, event: LlmTokenEvent) {
        // LLM tokens are not persisted — too high volume, no analytical value.
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
        self.db.record("notebook_entry", &event);
        self.broadcast("notebook_entry", &event);
    }
}
