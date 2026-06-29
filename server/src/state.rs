//! Minimal shared application state.
//!
//! No `HypothesisManager`, no `DiscoveryJournal`, no notebook. The audit chain is
//! the authoritative record; everything else is derived or transient.

use crate::queue::ProtocolQueue;
use crate::auth::AuthStore;
use axiom_audit::{Chain, RevocationList, Signer};
use axiom_gate::{ApprovalQueue, CapabilityPolicy};
use axiom_proofs::ProofChecker;
use axiom_sila::SilaClients;
use axiom_types::LabState;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub running: Arc<AtomicBool>,
    pub iteration: Arc<AtomicU32>,
    pub audit_chain: Arc<Chain>,
    pub lab_state: Arc<Mutex<LabState>>,
    pub approval_queue: Arc<ApprovalQueue>,
    pub protocol_queue: Arc<ProtocolQueue>,
    pub tx: broadcast::Sender<String>,

    // Dependencies needed to build a GateContext for each run.
    pub signer: Arc<dyn Signer>,
    pub clients: Arc<SilaClients>,
    pub proofs: Arc<ProofChecker>,
    pub capability: Arc<CapabilityPolicy>,
    pub revocations: Arc<RevocationList>,
    pub auth: Arc<AuthStore>,
}

impl AppState {
    /// Broadcast a JSON event line to all WebSocket subscribers (best-effort).
    pub fn broadcast(&self, event: serde_json::Value) {
        let _ = self.tx.send(event.to_string());
    }
}
