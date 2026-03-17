use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

// ── Exploration log ───────────────────────────────────────────────────────────

#[derive(Default)]
pub struct ExplorationLog {
    pub findings:   Vec<String>,
    pub rejections: Vec<(String, String)>, // (tool, reason)
    pub successes:  Vec<String>,
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

/// Broadcasts all orchestrator events to every connected WebSocket client
/// and buffers them in memory for the /api/history endpoint.
pub struct WebSocketSink {
    pub tx:      broadcast::Sender<String>,
    pub log:     Arc<Mutex<ExplorationLog>>,
    pub notebook: Arc<Mutex<Vec<serde_json::Value>>>,
    pub events:  EventBuffer,
}

impl WebSocketSink {
    fn broadcast(&self, event: &str, payload: impl serde::Serialize) {
        let msg = serde_json::json!({ "event": event, "payload": payload });
        self.tx.send(msg.to_string()).ok();
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
}
