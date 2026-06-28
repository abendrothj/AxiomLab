//! SiLA 2 instrument access — one `execute` contract, two backends.
//!
//! [`SilaClients`] dispatches a proposed [`Action`] either to the offline
//! [`SimLab`](sim::SimLab) physics model or to real instruments over gRPC. The
//! gate pipeline calls `execute` only after every safety gate has passed, so
//! this crate contains transport + physics, never policy.

mod grpc;
mod sim;

/// Generated gRPC clients for the instrument services.
pub mod pb {
    tonic::include_proto!("axiomlab.hardware");
}

pub use grpc::GrpcLab;
pub use sim::SimLab;

use axiom_types::Action;
use serde_json::Value;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum SilaError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("missing parameter: {0}")]
    MissingParam(String),
    #[error("simulator physics error: {0}")]
    Physics(String),
    #[error("gRPC transport error: {0}")]
    Transport(String),
    #[error("gRPC call failed: {0}")]
    Rpc(String),
}

/// Unified instrument client. Construct with [`SilaClients::simulator`] for the
/// offline model or [`SilaClients::grpc`] for real hardware.
pub struct SilaClients {
    backend: Backend,
}

enum Backend {
    Simulator(Mutex<SimLab>),
    Grpc(GrpcLab),
}

impl SilaClients {
    /// Offline physics simulator backend (the default for development).
    pub fn simulator() -> Self {
        Self { backend: Backend::Simulator(Mutex::new(SimLab::new())) }
    }

    /// Real-hardware gRPC backend, all services on one endpoint.
    pub fn grpc(endpoint: impl Into<String>) -> Self {
        Self { backend: Backend::Grpc(GrpcLab::single(endpoint)) }
    }

    /// Select backend from the environment: `AXIOMLAB_SILA_ENDPOINT` enables the
    /// gRPC backend; otherwise the simulator is used.
    pub fn from_env() -> Self {
        match std::env::var("AXIOMLAB_SILA_ENDPOINT") {
            Ok(ep) if !ep.is_empty() => {
                tracing::info!(endpoint = %ep, "SiLA gRPC backend");
                Self::grpc(ep)
            }
            _ => {
                tracing::info!("SiLA simulator backend (no AXIOMLAB_SILA_ENDPOINT)");
                Self::simulator()
            }
        }
    }

    /// True if this is the offline simulator backend.
    pub fn is_simulator(&self) -> bool {
        matches!(self.backend, Backend::Simulator(_))
    }

    /// Execute a proposed action. Called by `ExecuteGate` after all safety gates pass.
    pub async fn execute(&self, action: &Action) -> Result<Value, SilaError> {
        match &self.backend {
            Backend::Simulator(lab) => lab.lock().await.execute(action),
            Backend::Grpc(lab) => lab.execute(action).await,
        }
    }

    /// Snapshot of vessel volumes (simulator only; `None` for gRPC).
    pub async fn vessel_snapshot(&self) -> Option<Value> {
        match &self.backend {
            Backend::Simulator(lab) => Some(lab.lock().await.vessel_snapshot()),
            Backend::Grpc(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_types::RiskClass;
    use serde_json::json;

    #[tokio::test]
    async fn simulator_execute_dispense() {
        let clients = SilaClients::simulator();
        assert!(clients.is_simulator());
        let action = Action::new("dispense", json!({"vessel_id": "tube_1", "volume_ul": 100.0}), RiskClass::LiquidHandling);
        let r = clients.execute(&action).await.unwrap();
        assert_eq!(r["success"], true);
        assert!(clients.vessel_snapshot().await.is_some());
    }

    #[tokio::test]
    async fn from_env_defaults_to_sim() {
        unsafe { std::env::remove_var("AXIOMLAB_SILA_ENDPOINT") };
        assert!(SilaClients::from_env().is_simulator());
    }
}
