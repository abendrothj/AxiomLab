//! Operator approval queue for high-risk actions.
//!
//! The `ApprovalGate` registers a pending request scoped to a hash of the exact
//! tool + params, then awaits the operator's decision (with a timeout that
//! auto-denies). Once a scope is approved it is remembered for the session, so an
//! identical action does not re-prompt — but *different* params produce a
//! different scope hash and require a fresh approval.

use axiom_types::RiskClass;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// An operator's decision on a pending approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub approved: bool,
    pub notes: String,
    /// Identifier of the approving operator's key (checked against revocations).
    pub approver_id: String,
}

/// A request awaiting an operator decision (the serialisable view for the API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool: String,
    pub params: Value,
    pub scope_hash: String,
    pub created_secs: u64,
    pub expires_secs: u64,
    pub risk_class: Option<RiskClass>,
    pub gate: String,
    pub reason: String,
    #[serde(default)]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    TimedOut,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub request: ApprovalRequest,
    pub status: ApprovalStatus,
    pub decision: Option<Decision>,
    pub resolved_secs: Option<u64>,
}

struct Pending {
    request: ApprovalRequest,
    responder: oneshot::Sender<Decision>,
}

/// Live approval waiters plus a durable lifecycle journal shared by the gate
/// pipeline and server API. Restarted pending requests fail closed as interrupted.
pub struct ApprovalQueue {
    pending: Mutex<HashMap<String, Pending>>,
    granted_scopes: Mutex<HashSet<String>>,
    records: Mutex<Vec<ApprovalRecord>>,
    path: Option<PathBuf>,
    sqlite_path: Option<PathBuf>,
}

impl Default for ApprovalQueue {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            granted_scopes: Mutex::new(HashSet::new()),
            records: Mutex::new(Vec::new()),
            path: None,
            sqlite_path: None,
        }
    }
}

impl ApprovalQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut records: Vec<ApprovalRecord> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let recovered_at = now_secs();
        for record in &mut records {
            if record.status == ApprovalStatus::Pending {
                record.status = ApprovalStatus::Interrupted;
                record.resolved_secs = Some(recovered_at);
                record.decision = Some(Decision {
                    approved: false,
                    notes: "Server restarted while approval was pending".into(),
                    approver_id: "system".into(),
                });
            }
        }
        let queue = Self {
            pending: Mutex::new(HashMap::new()),
            granted_scopes: Mutex::new(HashSet::new()),
            records: Mutex::new(records),
            path: Some(path),
            sqlite_path: None,
        };
        queue.persist()?;
        Ok(queue)
    }

    pub fn open_sqlite(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let connection = rusqlite::Connection::open(&path).map_err(|e| e.to_string())?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS approval_records(id TEXT PRIMARY KEY,run_id TEXT,scope_hash TEXT NOT NULL,request_json TEXT NOT NULL,status TEXT NOT NULL,requested_by TEXT,decided_by TEXT,decision_json TEXT,created_secs INTEGER NOT NULL,resolved_secs INTEGER)").map_err(|e|e.to_string())?;
        let mut statement=connection.prepare("SELECT request_json,status,decision_json,resolved_secs FROM approval_records ORDER BY created_secs,id").map_err(|e|e.to_string())?;
        let mut records: Vec<ApprovalRecord> = statement
            .query_map([], |row| {
                let request: String = row.get(0)?;
                let status: String = row.get(1)?;
                let decision: Option<String> = row.get(2)?;
                Ok(ApprovalRecord {
                    request: serde_json::from_str(&request).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    status: ApprovalStatus::parse(&status),
                    decision: decision.and_then(|v| serde_json::from_str(&v).ok()),
                    resolved_secs: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        drop(statement);
        let recovered = now_secs();
        for record in &mut records {
            if record.status == ApprovalStatus::Pending {
                record.status = ApprovalStatus::Interrupted;
                record.resolved_secs = Some(recovered);
                record.decision = Some(Decision {
                    approved: false,
                    notes: "Server restarted while approval was pending".into(),
                    approver_id: "system".into(),
                });
            }
        }
        let queue = Self {
            pending: Mutex::new(HashMap::new()),
            granted_scopes: Mutex::new(HashSet::new()),
            records: Mutex::new(records),
            path: None,
            sqlite_path: Some(path),
        };
        queue.persist().map_err(|e| e.to_string())?;
        Ok(queue)
    }

    /// Scope hash for an action: `sha256(tool || params)`.
    pub fn scope_hash(tool: &str, params: &Value) -> String {
        let mut h = Sha256::new();
        h.update(tool.as_bytes());
        h.update(b"\0");
        h.update(params.to_string().as_bytes());
        format!("{:x}", h.finalize())
    }

    /// True if this exact scope was already approved this session.
    pub fn is_scope_granted(&self, scope_hash: &str) -> bool {
        self.granted_scopes.lock().unwrap().contains(scope_hash)
    }

    /// Register a pending request, returning a receiver for the operator's decision.
    pub fn request(&self, tool: &str, params: &Value) -> (String, oneshot::Receiver<Decision>) {
        self.request_with_metadata(
            tool,
            params,
            None,
            "ApprovalGate",
            "Operator approval required",
            Duration::from_secs(300),
        )
    }

    pub fn request_with_metadata(
        &self,
        tool: &str,
        params: &Value,
        risk_class: Option<RiskClass>,
        gate: impl Into<String>,
        reason: impl Into<String>,
        timeout: Duration,
    ) -> (String, oneshot::Receiver<Decision>) {
        self.request_with_metadata_for_run(tool, params, risk_class, gate, reason, timeout, None)
    }

    pub fn request_with_metadata_for_run(
        &self,
        tool: &str,
        params: &Value,
        risk_class: Option<RiskClass>,
        gate: impl Into<String>,
        reason: impl Into<String>,
        timeout: Duration,
        run_id: Option<String>,
    ) -> (String, oneshot::Receiver<Decision>) {
        let id = uuid::Uuid::new_v4().to_string();
        let scope_hash = Self::scope_hash(tool, params);
        let (tx, rx) = oneshot::channel();
        let created_secs = now_secs();
        let req = ApprovalRequest {
            id: id.clone(),
            tool: tool.to_string(),
            params: params.clone(),
            scope_hash,
            created_secs,
            expires_secs: created_secs.saturating_add(timeout.as_secs()),
            risk_class,
            gate: gate.into(),
            reason: reason.into(),
            run_id,
        };
        self.records.lock().unwrap().push(ApprovalRecord {
            request: req.clone(),
            status: ApprovalStatus::Pending,
            decision: None,
            resolved_secs: None,
        });
        self.pending.lock().unwrap().insert(
            id.clone(),
            Pending {
                request: req,
                responder: tx,
            },
        );
        self.persist_best_effort();
        (id, rx)
    }

    /// Resolve a pending request with the operator's decision. Remembers the
    /// scope when approved. Returns `Err` if the id is unknown.
    pub fn resolve(&self, id: &str, decision: Decision) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .unwrap()
            .remove(id)
            .ok_or_else(|| format!("no pending approval '{id}'"))?;
        let status = if decision.approved {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Denied
        };
        if decision.approved {
            self.granted_scopes
                .lock()
                .unwrap()
                .insert(pending.request.scope_hash.clone());
        }
        self.update_record(id, status, Some(decision.clone()));
        // Receiver may have dropped on timeout; ignore send error.
        let _ = pending.responder.send(decision);
        Ok(())
    }

    /// Drop a pending request without resolving (used on gate-side timeout).
    pub fn cancel(&self, id: &str) {
        if self.pending.lock().unwrap().remove(id).is_some() {
            self.update_record(
                id,
                ApprovalStatus::TimedOut,
                Some(Decision {
                    approved: false,
                    notes: "Approval timed out".into(),
                    approver_id: "system".into(),
                }),
            );
        }
    }

    /// Snapshot of all pending requests, for the `/api/approvals` route.
    pub fn list_pending(&self) -> Vec<ApprovalRequest> {
        self.pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.request.clone())
            .collect()
    }

    pub fn history(&self) -> Vec<ApprovalRecord> {
        self.records.lock().unwrap().clone()
    }

    fn update_record(&self, id: &str, status: ApprovalStatus, decision: Option<Decision>) {
        if let Some(record) = self
            .records
            .lock()
            .unwrap()
            .iter_mut()
            .find(|record| record.request.id == id)
        {
            record.status = status;
            record.decision = decision;
            record.resolved_secs = Some(now_secs());
        }
        self.persist_best_effort();
    }

    fn persist_best_effort(&self) {
        if let Err(error) = self.persist() {
            tracing::error!(%error, "failed to persist approval journal");
        }
    }

    fn persist(&self) -> std::io::Result<()> {
        if let Some(path) = &self.sqlite_path {
            let mut connection = rusqlite::Connection::open(path).map_err(std::io::Error::other)?;
            let transaction = connection.transaction().map_err(std::io::Error::other)?;
            transaction
                .execute("DELETE FROM approval_records", [])
                .map_err(std::io::Error::other)?;
            for record in self.records.lock().unwrap().iter() {
                transaction.execute("INSERT INTO approval_records(id,run_id,scope_hash,request_json,status,decided_by,decision_json,created_secs,resolved_secs) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",rusqlite::params![record.request.id,record.request.run_id,record.request.scope_hash,serde_json::to_string(&record.request).map_err(std::io::Error::other)?,record.status.as_str(),record.decision.as_ref().map(|d|d.approver_id.clone()),record.decision.as_ref().map(serde_json::to_string).transpose().map_err(std::io::Error::other)?,record.request.created_secs,record.resolved_secs]).map_err(std::io::Error::other)?;
            }
            transaction.commit().map_err(std::io::Error::other)?;
            return Ok(());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&*self.records.lock().unwrap())
                .map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(temporary, path)
    }
}

impl ApprovalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
        }
    }
    fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "denied" => Self::Denied,
            "timed_out" => Self::TimedOut,
            "interrupted" => Self::Interrupted,
            _ => Self::Interrupted,
        }
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
    use serde_json::json;

    #[test]
    fn scope_hash_is_param_sensitive() {
        let a = ApprovalQueue::scope_hash("move_arm", &json!({"x": 1.0}));
        let b = ApprovalQueue::scope_hash("move_arm", &json!({"x": 2.0}));
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn request_resolve_roundtrip() {
        let q = ApprovalQueue::new();
        let (id, rx) = q.request("move_arm", &json!({"x": 1.0}));
        assert_eq!(q.list_pending().len(), 1);
        q.resolve(
            &id,
            Decision {
                approved: true,
                notes: "ok".into(),
                approver_id: "op1".into(),
            },
        )
        .unwrap();
        let d = rx.await.unwrap();
        assert!(d.approved);
        assert!(q.is_scope_granted(&ApprovalQueue::scope_hash("move_arm", &json!({"x": 1.0}))));
        assert_eq!(q.list_pending().len(), 0);
    }

    #[test]
    fn resolve_unknown_errors() {
        let q = ApprovalQueue::new();
        assert!(
            q.resolve(
                "nope",
                Decision {
                    approved: true,
                    notes: String::new(),
                    approver_id: "x".into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn request_metadata_is_exposed_to_operators() {
        let q = ApprovalQueue::new();
        let (_id, _rx) = q.request_with_metadata(
            "move_arm",
            &json!({"x_mm": 10.0}),
            Some(RiskClass::Actuation),
            "ApprovalGate",
            "Physical movement requires review",
            Duration::from_secs(60),
        );
        let pending = q.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].risk_class, Some(RiskClass::Actuation));
        assert_eq!(pending[0].gate, "ApprovalGate");
        assert_eq!(pending[0].reason, "Physical movement requires review");
        assert_eq!(pending[0].expires_secs, pending[0].created_secs + 60);
    }

    #[test]
    fn restart_marks_pending_request_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        let queue = ApprovalQueue::open(&path).unwrap();
        queue.request("move_arm", &json!({"x": 1.0}));
        drop(queue);
        let recovered = ApprovalQueue::open(&path).unwrap();
        assert!(recovered.list_pending().is_empty());
        assert_eq!(recovered.history()[0].status, ApprovalStatus::Interrupted);
        assert_eq!(
            recovered.history()[0]
                .decision
                .as_ref()
                .unwrap()
                .approver_id,
            "system"
        );
    }

    #[test]
    fn sqlite_journal_marks_pending_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let queue = ApprovalQueue::open_sqlite(&path).unwrap();
        queue.request_with_metadata_for_run(
            "move_arm",
            &json!({"x":1}),
            Some(RiskClass::Actuation),
            "ApprovalGate",
            "review",
            Duration::from_secs(60),
            Some("run-1".into()),
        );
        drop(queue);
        let recovered = ApprovalQueue::open_sqlite(&path).unwrap();
        assert_eq!(recovered.history()[0].status, ApprovalStatus::Interrupted);
        assert_eq!(
            recovered.history()[0].request.run_id.as_deref(),
            Some("run-1")
        );
    }
}
