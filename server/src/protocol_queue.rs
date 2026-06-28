//! Operator-driven protocol queue.
//!
//! Operators (humans, higher-level systems, or the Telegram overseer) push
//! protocol directives here. The execution loop drains them in priority order
//! before falling back to the built-in commissioning agenda.
//!
//! This is the primary interface between external intent and agentic lab
//! execution: the queue is what turns AxiomLab from a self-contained loop
//! into an addressable execution engine.

use agent_runtime::audit::data_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ── Status ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    /// Waiting to be assigned to an experiment slot.
    Pending,
    /// Currently executing in an experiment slot.
    Running,
    /// Execution completed; result_summary contains the quantitative outcome.
    Completed,
    /// Execution failed; result_summary contains the error.
    Failed,
}

impl std::fmt::Display for QueueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending   => write!(f, "pending"),
            Self::Running   => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed    => write!(f, "failed"),
        }
    }
}

// ── Queued item ───────────────────────────────────────────────────────────────

/// A single operator-requested protocol directive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedProtocol {
    pub id: String,

    /// Natural-language statement of what the agent should execute.
    /// Becomes the top-level directive in the LLM execution mandate.
    /// Write it as a precise lab instruction: instrument, procedure, and
    /// the quantitative outcome expected (e.g. "slope ± stderr, R²").
    pub statement: String,

    /// 0 = normal, 255 = urgent.  Higher-priority items run before lower.
    /// Items at the same priority level run in FIFO order.
    pub priority: u8,

    pub added_at_secs: i64,
    pub status: QueueStatus,

    /// Experiment ID assigned when status transitions to Running.
    pub experiment_id: Option<String>,

    /// Human-readable outcome recorded on Completed or Failed.
    pub result_summary: Option<String>,
}

// ── Queue ─────────────────────────────────────────────────────────────────────

/// Persistent, priority-ordered queue of operator-requested protocols.
pub struct ProtocolQueue {
    pub items: Vec<QueuedProtocol>,
    path: PathBuf,
}

impl ProtocolQueue {
    /// Canonical queue file path.
    pub fn default_path() -> PathBuf {
        data_dir().join("protocol_queue.json")
    }

    /// Load from disk; returns an empty queue if the file does not exist.
    pub fn load(path: &Path) -> Self {
        let items: Vec<QueuedProtocol> = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { items, path: path.to_owned() }
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.items) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    // ── Mutations ─────────────────────────────────────────────────────────────

    /// Push a new directive onto the queue. Returns the new item's ID.
    pub fn enqueue(&mut self, statement: String, priority: u8) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.items.push(QueuedProtocol {
            id: id.clone(),
            statement,
            priority,
            added_at_secs: now_secs(),
            status: QueueStatus::Pending,
            experiment_id: None,
            result_summary: None,
        });
        self.sort();
        self.save();
        id
    }

    /// Return the next pending item without removing it.
    pub fn next_pending(&self) -> Option<&QueuedProtocol> {
        self.items.iter().find(|i| i.status == QueueStatus::Pending)
    }

    /// Transition a pending item to Running and record the experiment ID.
    pub fn mark_running(&mut self, id: &str, experiment_id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = QueueStatus::Running;
            item.experiment_id = Some(experiment_id.to_owned());
            self.save();
            true
        } else {
            false
        }
    }

    /// Transition to Completed and record the outcome summary.
    pub fn mark_completed(&mut self, id: &str, summary: String) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = QueueStatus::Completed;
            item.result_summary = Some(summary);
            self.save();
            self.trim_history();
            true
        } else {
            false
        }
    }

    /// Transition to Failed and record the error.
    pub fn mark_failed(&mut self, id: &str, error: String) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = QueueStatus::Failed;
            item.result_summary = Some(error);
            self.save();
            self.trim_history();
            true
        } else {
            false
        }
    }

    /// Remove an item from the queue regardless of status.
    /// Returns true when an item was found and removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        if self.items.len() != before {
            self.save();
            true
        } else {
            false
        }
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    #[allow(dead_code)]
    pub fn has_pending(&self) -> bool {
        self.items.iter().any(|i| i.status == QueueStatus::Pending)
    }

    pub fn items(&self) -> &[QueuedProtocol] {
        &self.items
    }

    #[allow(dead_code)]
    pub fn pending_items(&self) -> impl Iterator<Item = &QueuedProtocol> {
        self.items.iter().filter(|i| i.status == QueueStatus::Pending)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Sort: highest priority first; FIFO within same priority.
    fn sort(&mut self) {
        self.items.sort_by(|a, b| {
            b.priority.cmp(&a.priority)
                .then(a.added_at_secs.cmp(&b.added_at_secs))
        });
    }

    /// Cap completed/failed history at 50 items (oldest trimmed first).
    fn trim_history(&mut self) {
        let done_indices: Vec<usize> = self.items.iter().enumerate()
            .filter(|(_, i)| matches!(i.status, QueueStatus::Completed | QueueStatus::Failed))
            .map(|(idx, _)| idx)
            .collect();
        if done_indices.len() > 50 {
            let drop_count = done_indices.len() - 50;
            let drop_set: std::collections::HashSet<usize> =
                done_indices.into_iter().take(drop_count).collect();
            let mut idx = 0;
            self.items.retain(|_| {
                let keep = !drop_set.contains(&idx);
                idx += 1;
                keep
            });
        }
    }
}
