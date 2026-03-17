use agent_runtime::events::{
    EventSink, LlmTokenEvent, NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent,
};
use tauri::{AppHandle, Emitter};

/// Bridges the orchestrator's `EventSink` to Tauri's event bus.
///
/// Each method calls `AppHandle::emit()`, which is synchronous and
/// `Send + Sync` in Tauri v2. Errors are swallowed with `.ok()` so
/// that a closed window does not panic the orchestrator task.
pub struct TauriEventSink {
    pub app: AppHandle,
}

impl EventSink for TauriEventSink {
    fn on_state_transition(&self, event: StateTransitionEvent) {
        self.app.emit("state_transition", event).ok();
    }

    fn on_tool_execution(&self, event: ToolExecutionEvent) {
        self.app.emit("tool_execution", event).ok();
    }

    fn on_llm_token(&self, event: LlmTokenEvent) {
        self.app.emit("llm_token", event).ok();
    }

    fn on_notebook_entry(&self, event: NotebookEntryEvent) {
        self.app.emit("notebook_entry", event).ok();
    }
}
