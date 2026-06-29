//! [`GateContext`] — everything the gates need to make a decision.
//!
//! (The rewrite plan sketched this under `crates/types`, but it references
//! `Chain`/`Signer` from `crates/audit`, which depends on `types`. Placing it
//! here, where those are in scope, avoids a dependency cycle.)

use crate::approvals::ApprovalQueue;
use crate::capability::CapabilityPolicy;
use axiom_audit::{Chain, RevocationList, Signer};
use axiom_proofs::ProofChecker;
use axiom_sila::SilaClients;
use axiom_types::LabState;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Per-action scratch space shared between gates within a single pipeline run
/// (e.g. the `ExecuteGate` result that the `AuditGate` records).
#[derive(Default)]
pub struct Scratch {
    pub exec_result: Option<Value>,
    pub vessel_snapshot: Option<Value>,
}

/// The shared context threaded through every gate.
#[derive(Clone)]
pub struct GateContext {
    pub experiment_id: String,
    pub iteration: u32,
    pub lab_state: Arc<Mutex<LabState>>,
    pub audit_chain: Arc<Chain>,
    pub signer: Arc<dyn Signer>,
    pub clients: Arc<SilaClients>,
    pub proofs: Arc<ProofChecker>,
    pub capability: Arc<CapabilityPolicy>,
    pub approvals: Arc<ApprovalQueue>,
    pub revocations: Arc<RevocationList>,
    /// How long the `ApprovalGate` waits before auto-denying.
    pub approval_timeout: Duration,
    pub(crate) scratch: Arc<Mutex<Scratch>>,
}

impl GateContext {
    /// Assemble a context. `approval_timeout` defaults sensibly if `None`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        experiment_id: impl Into<String>,
        iteration: u32,
        lab_state: Arc<Mutex<LabState>>,
        audit_chain: Arc<Chain>,
        signer: Arc<dyn Signer>,
        clients: Arc<SilaClients>,
        proofs: Arc<ProofChecker>,
        capability: Arc<CapabilityPolicy>,
        approvals: Arc<ApprovalQueue>,
        revocations: Arc<RevocationList>,
        approval_timeout: Option<Duration>,
    ) -> Self {
        Self {
            experiment_id: experiment_id.into(),
            iteration,
            lab_state,
            audit_chain,
            signer,
            clients,
            proofs,
            capability,
            approvals,
            revocations,
            approval_timeout: approval_timeout.unwrap_or(Duration::from_secs(300)),
            scratch: Arc::new(Mutex::new(Scratch::default())),
        }
    }

    pub(crate) fn reset_scratch(&self) {
        *self.scratch.lock().unwrap() = Scratch::default();
    }
    pub(crate) fn set_exec_result(&self, result: Value, snapshot: Option<Value>) {
        let mut s = self.scratch.lock().unwrap();
        s.exec_result = Some(result);
        s.vessel_snapshot = snapshot;
    }
    pub(crate) fn take_exec(&self) -> (Option<Value>, Option<Value>) {
        let mut s = self.scratch.lock().unwrap();
        (s.exec_result.take(), s.vessel_snapshot.take())
    }
}
