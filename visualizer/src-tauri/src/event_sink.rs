use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

// ── Shared exploration log ────────────────────────────────────────────────────

/// Accumulated knowledge from all experiments in this session.
/// Written by the event sink; read by the loop to build future mandates.
#[derive(Default)]
pub struct ExplorationLog {
    /// Notebook entry summaries the AI wrote after each experiment.
    pub findings: Vec<String>,
    /// Tool calls that were rejected: (tool_name, reason).
    pub rejections: Vec<(String, String)>,
    /// Tool calls that succeeded: tool names observed to work.
    pub successes: Vec<String>,
}

// ── Event sink ────────────────────────────────────────────────────────────────

pub struct TauriEventSink {
    pub app: AppHandle,
    pub log: Arc<Mutex<ExplorationLog>>,
}

impl EventSink for TauriEventSink {
    fn on_state_transition(&self, event: StateTransitionEvent) {
        self.app.emit("state_transition", event).ok();
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
        self.app.emit("tool_execution", event).ok();
    }

    fn on_llm_token(&self, event: LlmTokenEvent) {
        self.app.emit("llm_token", event).ok();
    }

    fn on_notebook_entry(&self, event: NotebookEntryEvent) {
        {
            let mut log = self.log.lock().unwrap();
            log.findings.push(event.entry.clone());
        }
        self.app.emit("notebook_entry", event).ok();
    }
}
