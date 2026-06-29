//! In-memory protocol (directive) queue.
//!
//! Operators push directives; a background worker claims the next pending one,
//! runs it through the orchestrator + gate pipeline, and records the outcome.

use serde::Serialize;
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    pub id: String,
    pub directive: String,
    pub status: QueueStatus,
    pub summary: Option<String>,
    pub created_secs: u64,
}

#[derive(Default)]
pub struct ProtocolQueue {
    items: Mutex<Vec<QueueItem>>,
    notify: Notify,
}

impl ProtocolQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, directive: impl Into<String>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.items.lock().unwrap().push(QueueItem {
            id: id.clone(),
            directive: directive.into(),
            status: QueueStatus::Pending,
            summary: None,
            created_secs: now_secs(),
        });
        self.notify.notify_one();
        id
    }

    pub fn list(&self) -> Vec<QueueItem> {
        self.items.lock().unwrap().clone()
    }

    /// Cancel a pending item. Returns `false` if it is missing or already running.
    pub fn cancel(&self, id: &str) -> bool {
        let mut items = self.items.lock().unwrap();
        if let Some(it) = items.iter_mut().find(|i| i.id == id) {
            if it.status == QueueStatus::Pending {
                it.status = QueueStatus::Cancelled;
                return true;
            }
        }
        false
    }

    /// Claim the next pending item, marking it Running. Returns `(id, directive)`.
    pub fn claim_next(&self) -> Option<(String, String)> {
        let mut items = self.items.lock().unwrap();
        let it = items.iter_mut().find(|i| i.status == QueueStatus::Pending)?;
        it.status = QueueStatus::Running;
        Some((it.id.clone(), it.directive.clone()))
    }

    pub fn finish(&self, id: &str, status: QueueStatus, summary: Option<String>) {
        let mut items = self.items.lock().unwrap();
        if let Some(it) = items.iter_mut().find(|i| i.id == id) {
            it.status = status;
            it.summary = summary;
        }
    }

    /// Wait until a new item may be available.
    pub async fn wait(&self) {
        self.notify.notified().await;
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_claim_finish() {
        let q = ProtocolQueue::new();
        let id = q.push("do thing");
        assert_eq!(q.list().len(), 1);
        let (cid, directive) = q.claim_next().unwrap();
        assert_eq!(cid, id);
        assert_eq!(directive, "do thing");
        assert!(q.claim_next().is_none());
        q.finish(&id, QueueStatus::Completed, Some("done".into()));
        assert_eq!(q.list()[0].status, QueueStatus::Completed);
    }

    #[test]
    fn cancel_only_pending() {
        let q = ProtocolQueue::new();
        let id = q.push("x");
        assert!(q.cancel(&id));
        assert!(!q.cancel(&id)); // already cancelled
        assert!(!q.cancel("missing"));
    }
}
