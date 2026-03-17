use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
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

/// Broadcasts all orchestrator events to every connected WebSocket client.
pub struct WebSocketSink {
    pub tx:  broadcast::Sender<String>,
    pub log: Arc<Mutex<ExplorationLog>>,
    /// Running copy of all notebook entries — sent to new viewers on connect.
    pub notebook: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl WebSocketSink {
    fn broadcast(&self, event: &str, payload: impl serde::Serialize) {
        let msg = serde_json::json!({ "event": event, "payload": payload });
        self.tx.send(msg.to_string()).ok();
    }
}

impl EventSink for WebSocketSink {
    fn on_state_transition(&self, event: StateTransitionEvent) {
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
        self.broadcast("tool_execution", &event);
    }

    fn on_llm_token(&self, event: LlmTokenEvent) {
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
        self.broadcast("notebook_entry", &event);
    }
}
