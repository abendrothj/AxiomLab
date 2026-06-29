//! In-memory protocol (directive) queue.
//!
//! Operators push directives; a background worker claims the next pending one,
//! runs it through the orchestrator + gate pipeline, and records the outcome.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: String,
    pub directive: String,
    pub status: QueueStatus,
    pub summary: Option<String>,
    pub created_secs: u64,
}

pub struct ProtocolQueue {
    items: Mutex<Vec<QueueItem>>,
    notify: Notify,
    path: Option<PathBuf>,
}

impl Default for ProtocolQueue {
    fn default() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
            notify: Notify::new(),
            path: None,
        }
    }
}

impl ProtocolQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut items: Vec<QueueItem> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        for item in &mut items {
            if item.status == QueueStatus::Running {
                item.status = QueueStatus::Pending;
                item.summary = Some("Recovered after an interrupted server process".into());
            }
        }
        let queue = Self {
            items: Mutex::new(items),
            notify: Notify::new(),
            path: Some(path),
        };
        queue.persist()?;
        Ok(queue)
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
        self.persist_best_effort();
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
                drop(items);
                self.persist_best_effort();
                return true;
            }
        }
        false
    }

    /// Claim the next pending item, marking it Running. Returns `(id, directive)`.
    pub fn claim_next(&self) -> Option<(String, String)> {
        let mut items = self.items.lock().unwrap();
        let it = items
            .iter_mut()
            .find(|i| i.status == QueueStatus::Pending)?;
        it.status = QueueStatus::Running;
        let claimed = (it.id.clone(), it.directive.clone());
        drop(items);
        self.persist_best_effort();
        Some(claimed)
    }

    pub fn finish(&self, id: &str, status: QueueStatus, summary: Option<String>) {
        let mut items = self.items.lock().unwrap();
        if let Some(it) = items.iter_mut().find(|i| i.id == id) {
            it.status = status;
            it.summary = summary;
        }
        drop(items);
        self.persist_best_effort();
    }

    /// Wait until a new item may be available.
    pub async fn wait(&self) {
        self.notify.notified().await;
    }

    fn persist_best_effort(&self) {
        if let Err(error) = self.persist() {
            tracing::error!(%error, "failed to persist protocol queue");
        }
    }

    fn persist(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&*self.items.lock().unwrap())
                .map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(temporary, path)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

    #[test]
    fn interrupted_run_is_requeued_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let queue = ProtocolQueue::open(&path).unwrap();
        queue.push("recover me");
        queue.claim_next().unwrap();
        drop(queue);
        let recovered = ProtocolQueue::open(&path).unwrap();
        assert_eq!(recovered.list()[0].status, QueueStatus::Pending);
        assert!(
            recovered.list()[0]
                .summary
                .as_deref()
                .unwrap()
                .contains("Recovered")
        );
    }
}
