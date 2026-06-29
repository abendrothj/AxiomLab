//! SQLite-backed directive queue with leases and fail-closed restart recovery.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Pending,
    Running,
    RecoveryRequired,
    Completed,
    Failed,
    Cancelled,
}

impl QueueStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::RecoveryRequired => "recovery_required",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "recovery_required" => Self::RecoveryRequired,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: String,
    pub directive: String,
    pub status: QueueStatus,
    pub summary: Option<String>,
    pub created_secs: u64,
    pub submitted_by: String,
    pub lease_expires_secs: Option<u64>,
    pub version: u64,
}

pub struct ProtocolQueue {
    connection: Mutex<Connection>,
    notify: Notify,
}

impl ProtocolQueue {
    pub fn new() -> Self {
        Self::open_in_memory().expect("open in-memory queue")
    }

    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::from_connection(Connection::open(path)?)
    }

    fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> rusqlite::Result<Self> {
        connection.execute_batch(include_str!("../migrations/0001_operational_state.sql"))?;
        connection.execute(
            "UPDATE directives SET status='recovery_required', summary='Server stopped while execution outcome was uncertain', lease_owner=NULL, lease_expires_secs=NULL, updated_secs=?1, version=version+1 WHERE status='running'",
            [now_secs()],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            notify: Notify::new(),
        })
    }

    pub fn push(&self, directive: impl Into<String>) -> String {
        self.push_for(directive, "development")
    }

    pub fn push_for(&self, directive: impl Into<String>, submitted_by: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_secs();
        self.connection.lock().unwrap().execute(
            "INSERT INTO directives(id,directive,status,created_secs,updated_secs,submitted_by) VALUES(?1,?2,'pending',?3,?3,?4)",
            params![id, directive.into(), now, submitted_by],
        ).expect("insert directive");
        self.notify.notify_one();
        id
    }

    pub fn list(&self) -> Vec<QueueItem> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection.prepare("SELECT id,directive,status,summary,created_secs,submitted_by,lease_expires_secs,version FROM directives ORDER BY created_secs,id").unwrap();
        statement
            .query_map([], |row| {
                Ok(QueueItem {
                    id: row.get(0)?,
                    directive: row.get(1)?,
                    status: QueueStatus::parse(&row.get::<_, String>(2)?),
                    summary: row.get(3)?,
                    created_secs: row.get(4)?,
                    submitted_by: row.get(5)?,
                    lease_expires_secs: row.get(6)?,
                    version: row.get(7)?,
                })
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn submitted_by(&self, id: &str) -> Option<String> {
        self.connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT submitted_by FROM directives WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.transition(id, QueueStatus::Pending, QueueStatus::Cancelled, None)
    }

    pub fn claim_next(&self) -> Option<(String, String)> {
        let mut connection = self.connection.lock().unwrap();
        let transaction = connection.transaction().ok()?;
        let next: Option<(String,String)> = transaction.query_row("SELECT id,directive FROM directives WHERE status='pending' ORDER BY created_secs,id LIMIT 1", [], |row| Ok((row.get(0)?,row.get(1)?))).optional().ok()?;
        let (id, directive) = next?;
        let expiry = now_secs() + 30;
        if transaction.execute("UPDATE directives SET status='running',lease_owner='worker-1',lease_expires_secs=?2,updated_secs=?3,version=version+1 WHERE id=?1 AND status='pending'", params![id,expiry,now_secs()]).ok()? != 1 { return None; }
        transaction.commit().ok()?;
        Some((id, directive))
    }

    pub fn finish(&self, id: &str, status: QueueStatus, summary: Option<String>) {
        let _ = self.connection.lock().unwrap().execute("UPDATE directives SET status=?2,summary=?3,lease_owner=NULL,lease_expires_secs=NULL,updated_secs=?4,version=version+1 WHERE id=?1", params![id,status.as_str(),summary,now_secs()]);
    }

    pub fn reconcile(&self, id: &str, retry: bool, notes: &str) -> bool {
        let changed = self.transition(
            id,
            QueueStatus::RecoveryRequired,
            if retry {
                QueueStatus::Pending
            } else {
                QueueStatus::Failed
            },
            Some(notes),
        );
        if changed && retry {
            self.notify.notify_one();
        }
        changed
    }

    fn transition(
        &self,
        id: &str,
        from: QueueStatus,
        to: QueueStatus,
        summary: Option<&str>,
    ) -> bool {
        self.connection.lock().unwrap().execute("UPDATE directives SET status=?3,summary=COALESCE(?4,summary),updated_secs=?5,version=version+1 WHERE id=?1 AND status=?2", params![id,from.as_str(),to.as_str(),summary,now_secs()]).unwrap_or(0)==1
    }
    pub async fn wait(&self) {
        self.notify.notified().await;
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
        let id = q.push("x");
        assert_eq!(q.claim_next().unwrap().0, id);
        q.finish(&id, QueueStatus::Completed, Some("done".into()));
        assert_eq!(q.list()[0].status, QueueStatus::Completed);
    }
    #[test]
    fn restart_requires_reconciliation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let q = ProtocolQueue::open(&path).unwrap();
        let id = q.push("x");
        q.claim_next().unwrap();
        drop(q);
        let q = ProtocolQueue::open(&path).unwrap();
        assert_eq!(q.list()[0].status, QueueStatus::RecoveryRequired);
        assert!(q.reconcile(&id, true, "physical state verified"));
        assert_eq!(q.list()[0].status, QueueStatus::Pending);
    }

    #[test]
    fn restart_preserves_terminal_and_pending_states() {
        for (status, expected) in [
            (QueueStatus::Pending, QueueStatus::Pending),
            (QueueStatus::Completed, QueueStatus::Completed),
            (QueueStatus::Failed, QueueStatus::Failed),
            (QueueStatus::Cancelled, QueueStatus::Cancelled),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.db");
            let queue = ProtocolQueue::open(&path).unwrap();
            let id = queue.push("checkpoint");
            match status {
                QueueStatus::Pending => {}
                QueueStatus::Cancelled => {
                    assert!(queue.cancel(&id));
                }
                other => queue.finish(&id, other, Some("terminal".into())),
            }
            drop(queue);
            let recovered = ProtocolQueue::open(&path).unwrap();
            assert_eq!(recovered.list()[0].status, expected);
        }
    }

    #[test]
    fn two_workers_cannot_claim_same_directive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let first = ProtocolQueue::open(&path).unwrap();
        let second = ProtocolQueue::open(&path).unwrap();
        let id = first.push("once");
        assert_eq!(first.claim_next().unwrap().0, id);
        assert!(second.claim_next().is_none());
    }
}
