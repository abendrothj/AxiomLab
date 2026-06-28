//! Shared in-process queue for interactive human approval of high-risk actions.
//!
//! When the orchestrator reaches a high-risk tool call with no pre-signed bundle,
//! it calls [`PendingApprovalQueue::enqueue`] and awaits the returned oneshot
//! receiver.  An operator inspects the queue via `GET /api/approvals/pending`,
//! signs a bundle with `approvalctl`, and POSTs it to `POST /api/approvals/submit`.
//! The server calls [`PendingApprovalQueue::submit`], which wakes the waiting
//! orchestrator task.

use crate::approvals::SignedApproval;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use uuid::Uuid;

// ── Sidecar persistence ───────────────────────────────────────────────────────

fn approvals_dir() -> PathBuf {
    crate::audit::data_dir().join("approvals")
}

fn write_sidecar(info: &PendingApprovalInfo) {
    let dir = approvals_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.json", info.pending_id));
    if let Ok(json) = serde_json::to_string_pretty(info) {
        std::fs::write(path, json).ok();
    }
}

fn remove_sidecar(pending_id: &str) {
    let path = approvals_dir().join(format!("{pending_id}.json"));
    std::fs::remove_file(path).ok(); // silent: may not exist
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── System-sourced approval context ──────────────────────────────────────────

/// Where in a protocol the approval was triggered — set by `run_protocol`, not the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolStepInfo {
    pub protocol_name: String,
    pub step_index: usize,
    pub step_count: usize,
    /// Human-readable description from the protocol plan (e.g. "increment fill fraction by 100µL").
    pub description: String,
}

/// Context the orchestrator assembles from its own state — not from anything
/// the agent produced — so the operator can trust it.
#[derive(Debug, Clone, Default)]
pub struct ApprovalContext {
    /// The operator-issued directive this run is fulfilling (set externally,
    /// not from the agent's tool call output).
    pub directive: String,
    /// Stable ID of the current run.
    pub experiment_id: String,
    /// Which iteration of the execution loop this call was made in.
    pub iteration: u32,
    /// Risk class determined from the proof manifest (`ReadOnly`, `LiquidHandling`,
    /// `Actuation`, `Destructive`).  Set by `try_tool_call`, not the agent.
    pub risk_class: Option<String>,
    /// The last N tool calls successfully dispatched this run, in order.
    /// Each entry is `(tool_name, params)`.  Gives the operator a verifiable
    /// record of what the agent has actually done, not what it claims to have done.
    pub recent_actions: Vec<(String, serde_json::Value)>,
    /// If this approval was triggered from inside a structured protocol, the
    /// step position and description give the operator precise context.
    pub protocol_step: Option<ProtocolStepInfo>,
}

// ── Pending entry ─────────────────────────────────────────────────────────────

struct PendingEntry {
    pending_id:    String,
    tool_name:     String,
    params:        serde_json::Value,
    queued_at:     u64,
    session_nonce: Option<String>,
    context:       ApprovalContext,
    /// Taken exactly once by `submit()`.
    tx: Option<oneshot::Sender<Option<Vec<SignedApproval>>>>,
}

// ── Public info type (serialisable, sent to operator) ────────────────────────

/// Returned by `GET /api/approvals/pending`.
///
/// Every field is sourced from the orchestrator's own state, not from the
/// agent's output, so the operator can rely on it when deciding to approve or deny.
#[derive(Debug, Clone, Serialize)]
pub struct PendingApprovalInfo {
    pub pending_id:    String,
    pub tool_name:     String,
    /// Full params — what the agent is actually requesting.
    pub params:        serde_json::Value,
    pub queued_at:     u64,
    /// Included so the operator can pass the correct nonce to `approvalctl sign`.
    pub session_nonce: Option<String>,
    // ── System-verified context ───────────────────────────────────────────────
    /// Operator directive this run is fulfilling (set externally, not by the agent).
    pub directive:     String,
    pub experiment_id: String,
    pub iteration:     u32,
    /// Risk class from the proof manifest.
    pub risk_class:    Option<String>,
    /// Last ≤5 tool calls dispatched this run (verified from orchestrator
    /// state, not from the agent's narrative).
    pub recent_actions: Vec<serde_json::Value>,
    /// If this approval was triggered from a structured protocol, the step
    /// position and description give the operator precise context.
    pub protocol_step: Option<ProtocolStepInfo>,
}

// ── Submit errors ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SubmitError {
    /// No entry with this `pending_id` (timed out or never existed).
    NotFound,
    /// A bundle was already submitted for this `pending_id`.
    AlreadyConsumed,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound       => write!(f, "pending_id not found or already timed out"),
            Self::AlreadyConsumed => write!(f, "approval already submitted for this pending_id"),
        }
    }
}

// ── Queue ─────────────────────────────────────────────────────────────────────

fn entry_to_info(e: &PendingEntry) -> PendingApprovalInfo {
    let recent = e.context.recent_actions.iter()
        .map(|(tool, params)| serde_json::json!({"tool": tool, "params": params}))
        .collect();
    PendingApprovalInfo {
        pending_id:    e.pending_id.clone(),
        tool_name:     e.tool_name.clone(),
        params:        e.params.clone(),
        queued_at:     e.queued_at,
        session_nonce: e.session_nonce.clone(),
        directive:     e.context.directive.clone(),
        experiment_id: e.context.experiment_id.clone(),
        iteration:     e.context.iteration,
        risk_class:    e.context.risk_class.clone(),
        recent_actions: recent,
        protocol_step: e.context.protocol_step.clone(),
    }
}

/// Shared between orchestrator tasks and Axum HTTP handlers.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) because the critical
/// sections are tiny and never span an `.await` point.
pub struct PendingApprovalQueue {
    inner: Mutex<HashMap<String, PendingEntry>>,
}

impl PendingApprovalQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Place a high-risk action into the queue and return a receiver the
    /// orchestrator can `await` (with a timeout).
    ///
    /// The returned `pending_id` is a UUID that uniquely identifies this
    /// pending action and must be passed to [`submit`] by the operator.
    pub fn enqueue(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        session_nonce: Option<String>,
        context: ApprovalContext,
    ) -> (String, oneshot::Receiver<Option<Vec<SignedApproval>>>) {
        let (tx, rx) = oneshot::channel();
        let pending_id = Uuid::new_v4().to_string();
        let entry = PendingEntry {
            pending_id: pending_id.clone(),
            tool_name: tool_name.to_owned(),
            params,
            queued_at: unix_now_secs(),
            session_nonce,
            context,
            tx: Some(tx),
        };
        let info = entry_to_info(&entry);
        self.inner.lock().unwrap().insert(pending_id.clone(), entry);
        write_sidecar(&info);
        (pending_id, rx)
    }

    /// Remove a pending entry from the in-memory map and delete its sidecar.
    ///
    /// Use this for denied/timed-out/cancelled paths where no dispatch is
    /// happening and the sidecar is no longer needed.
    pub fn remove(&self, pending_id: &str) {
        self.inner.lock().unwrap().remove(pending_id);
        remove_sidecar(pending_id);
    }

    /// Remove a pending entry from the in-memory map but **keep the sidecar**.
    ///
    /// Call this when the operator approves an action and the orchestrator is
    /// about to proceed through the remaining validation stages toward dispatch.
    /// The sidecar must stay on disk as a WAL record so that if the process
    /// crashes before `purge_sidecar` is called, the stall detector can find
    /// and report the interrupted dispatch on the next startup.
    pub fn dequeue_approved(&self, pending_id: &str) {
        self.inner.lock().unwrap().remove(pending_id);
        // Sidecar is intentionally NOT removed here — it is removed after
        // emit_dispatch_complete via purge_sidecar().
    }

    /// Delete the on-disk sidecar for a completed dispatch.
    ///
    /// Call this after `emit_dispatch_complete` succeeds to signal that the
    /// dispatch is fully resolved and the sidecar is no longer a recovery marker.
    pub fn purge_sidecar(&self, pending_id: &str) {
        remove_sidecar(pending_id);
    }

    /// Return a snapshot of all currently pending approvals.
    pub fn list(&self) -> Vec<PendingApprovalInfo> {
        self.inner
            .lock()
            .unwrap()
            .values()
            .map(entry_to_info)
            .collect()
    }

    /// Wake the orchestrator waiting on `pending_id`.
    ///
    /// - `bundle = Some(approvals)` → operator approved; orchestrator will
    ///   validate the bundle against the full `ApprovalPolicy`.
    /// - `bundle = None` → operator explicitly denied.
    ///
    /// The entry is **not** removed here — the orchestrator calls `remove()`
    /// after it wakes, ensuring the entry stays visible in `list()` until the
    /// orchestrator has acknowledged the decision.
    pub fn submit(
        &self,
        pending_id: &str,
        bundle: Option<Vec<SignedApproval>>,
    ) -> Result<(), SubmitError> {
        let mut map = self.inner.lock().unwrap();
        let entry = map.get_mut(pending_id).ok_or(SubmitError::NotFound)?;
        let tx = entry.tx.take().ok_or(SubmitError::AlreadyConsumed)?;
        // Ignore SendError: the receiver may have timed out in the instant
        // between the lock being acquired here and the send completing.
        let _ = tx.send(bundle);
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ApprovalContext {
        ApprovalContext {
            directive:     "Calibrate spectrophotometer at 500 nm".into(),
            experiment_id: "exp-test-1".into(),
            iteration:     2,
            risk_class:    Some("LiquidHandling".into()),
            recent_actions: vec![],
            protocol_step: None,
        }
    }

    #[tokio::test]
    async fn submit_wakes_receiver_with_approval() {
        let q = PendingApprovalQueue::new();
        let (id, rx) = q.enqueue("dispense", serde_json::json!({"volume_ul": 100}), None, ctx());
        q.submit(&id, Some(vec![])).unwrap();
        let result = rx.await.unwrap();
        assert!(result.is_some());
        q.remove(&id);
        assert!(q.list().is_empty());
    }

    #[tokio::test]
    async fn submit_none_signals_denial() {
        let q = PendingApprovalQueue::new();
        let (id, rx) = q.enqueue("move_arm", serde_json::json!({}), None, ctx());
        q.submit(&id, None).unwrap();
        let result = rx.await.unwrap();
        assert!(result.is_none());
        q.remove(&id);
    }

    #[tokio::test]
    async fn double_submit_returns_already_consumed() {
        let q = PendingApprovalQueue::new();
        let (id, _rx) = q.enqueue("dispense", serde_json::json!({}), None, ctx());
        q.submit(&id, None).unwrap();
        assert!(matches!(q.submit(&id, None), Err(SubmitError::AlreadyConsumed)));
    }

    #[test]
    fn submit_unknown_id_returns_not_found() {
        let q = PendingApprovalQueue::new();
        assert!(matches!(
            q.submit("nonexistent-id", None),
            Err(SubmitError::NotFound)
        ));
    }

    #[test]
    fn list_exposes_system_context() {
        let q = PendingApprovalQueue::new();
        let (id1, _) = q.enqueue("dispense", serde_json::json!({}), None, ctx());
        let mut ctx2 = ctx();
        ctx2.recent_actions = vec![
            ("read_absorbance".into(), serde_json::json!({"vessel_id": "beaker_A", "wavelength_nm": 500})),
        ];
        let (id2, _) = q.enqueue("move_arm", serde_json::json!({}), Some("nonce-abc".into()), ctx2);
        let list = q.list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|e| e.pending_id == id1));
        let arm = list.iter().find(|e| e.pending_id == id2).unwrap();
        assert_eq!(arm.session_nonce.as_deref(), Some("nonce-abc"));
        assert_eq!(arm.directive, "Calibrate spectrophotometer at 500 nm");
        assert_eq!(arm.risk_class.as_deref(), Some("LiquidHandling"));
        assert_eq!(arm.recent_actions.len(), 1);
        assert_eq!(arm.recent_actions[0]["tool"], "read_absorbance");
    }

    #[tokio::test]
    async fn timeout_leaves_entry_until_remove() {
        let q = PendingApprovalQueue::new();
        let (id, rx) = q.enqueue("dispense", serde_json::json!({}), None, ctx());
        drop(rx);
        assert_eq!(q.list().len(), 1);
        q.remove(&id);
        assert!(q.list().is_empty());
    }
}
