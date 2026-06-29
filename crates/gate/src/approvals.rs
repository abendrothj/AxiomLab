//! Operator approval queue for high-risk actions.
//!
//! The `ApprovalGate` registers a pending request scoped to a hash of the exact
//! tool + params, then awaits the operator's decision (with a timeout that
//! auto-denies). Once a scope is approved it is remembered for the session, so an
//! identical action does not re-prompt — but *different* params produce a
//! different scope hash and require a fresh approval.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tokio::sync::oneshot;

/// An operator's decision on a pending approval request.
#[derive(Debug, Clone)]
pub struct Decision {
    pub approved: bool,
    pub notes: String,
    /// Identifier of the approving operator's key (checked against revocations).
    pub approver_id: String,
}

/// A request awaiting an operator decision (the serialisable view for the API).
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool: String,
    pub params: Value,
    pub scope_hash: String,
    pub created_secs: u64,
}

struct Pending {
    request: ApprovalRequest,
    responder: oneshot::Sender<Decision>,
}

/// In-memory approval queue shared between the gate pipeline and the server API.
#[derive(Default)]
pub struct ApprovalQueue {
    pending: Mutex<HashMap<String, Pending>>,
    granted_scopes: Mutex<HashSet<String>>,
}

impl ApprovalQueue {
    pub fn new() -> Self {
        Self::default()
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
        let id = uuid::Uuid::new_v4().to_string();
        let scope_hash = Self::scope_hash(tool, params);
        let (tx, rx) = oneshot::channel();
        let req = ApprovalRequest {
            id: id.clone(),
            tool: tool.to_string(),
            params: params.clone(),
            scope_hash,
            created_secs: now_secs(),
        };
        self.pending.lock().unwrap().insert(id.clone(), Pending { request: req, responder: tx });
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
        if decision.approved {
            self.granted_scopes.lock().unwrap().insert(pending.request.scope_hash.clone());
        }
        // Receiver may have dropped on timeout; ignore send error.
        let _ = pending.responder.send(decision);
        Ok(())
    }

    /// Drop a pending request without resolving (used on gate-side timeout).
    pub fn cancel(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
    }

    /// Snapshot of all pending requests, for the `/api/approvals` route.
    pub fn list_pending(&self) -> Vec<ApprovalRequest> {
        self.pending.lock().unwrap().values().map(|p| p.request.clone()).collect()
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
        q.resolve(&id, Decision { approved: true, notes: "ok".into(), approver_id: "op1".into() })
            .unwrap();
        let d = rx.await.unwrap();
        assert!(d.approved);
        assert!(q.is_scope_granted(&ApprovalQueue::scope_hash("move_arm", &json!({"x": 1.0}))));
        assert_eq!(q.list_pending().len(), 0);
    }

    #[test]
    fn resolve_unknown_errors() {
        let q = ApprovalQueue::new();
        assert!(q.resolve("nope", Decision { approved: true, notes: String::new(), approver_id: "x".into() }).is_err());
    }
}
