//! SiLA 2 instrument access — one `execute` contract, two backends.
//!
//! [`SilaClients`] dispatches a proposed [`Action`] either to the offline
//! [`SimLab`](sim::SimLab) physics model or to real instruments over gRPC. The
//! gate pipeline calls `execute` only after every safety gate has passed, so
//! this crate contains transport + physics, never policy.

mod full_sila;
mod grpc;
mod mock;
mod sim;

/// Generated gRPC clients (and mock-server stubs) for the instrument services.
pub mod pb {
    tonic::include_proto!("axiomlab.hardware");
}

/// Generated clients for the full SiLA 2 protocol used by `sila_sim`.
pub mod sila2_pb {
    pub mod org {
        pub mod silastandard {
            tonic::include_proto!("sila2.org.silastandard");
        }
        pub mod axiomlab {
            pub mod liquidhandling {
                pub mod liquidhandler {
                    pub mod v1 {
                        tonic::include_proto!("sila2.org.axiomlab.liquidhandling.liquidhandler.v1");
                    }
                }
            }
            pub mod measurement {
                pub mod spectrophotometer {
                    pub mod v1 {
                        tonic::include_proto!(
                            "sila2.org.axiomlab.measurement.spectrophotometer.v1"
                        );
                    }
                }
            }
            pub mod environment {
                pub mod incubator {
                    pub mod v1 {
                        tonic::include_proto!("sila2.org.axiomlab.environment.incubator.v1");
                    }
                }
            }
        }
    }
}

pub use full_sila::FullSilaLab;
pub use grpc::GrpcLab;
pub use mock::{serve as serve_mock, spawn_mock_server};
pub use sim::{FaultProfile, SimLab};

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
    #[error("injected simulator fault: {0}")]
    InjectedFault(String),
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
    FullSila(FullSilaLab),
}

impl SilaClients {
    /// Offline physics simulator backend (the default for development).
    pub fn simulator() -> Self {
        Self::simulator_with_faults(FaultProfile::default())
    }

    /// Offline simulator with a deterministic fault profile.
    pub fn simulator_with_faults(faults: FaultProfile) -> Self {
        Self {
            backend: Backend::Simulator(Mutex::new(SimLab::with_faults(faults))),
        }
    }

    /// Real-hardware gRPC backend, all services on one endpoint.
    pub fn grpc(endpoint: impl Into<String>) -> Self {
        Self {
            backend: Backend::Grpc(GrpcLab::single(endpoint)),
        }
    }

    /// Full SiLA 2 backend, compatible with the Python `sila_sim` server.
    pub fn full_sila(endpoint: impl Into<String>) -> Self {
        Self {
            backend: Backend::FullSila(FullSilaLab::single(endpoint)),
        }
    }

    /// Select backend from the environment: `AXIOMLAB_SILA_ENDPOINT` enables the
    /// gRPC backend; otherwise the simulator is used. Set
    /// `AXIOMLAB_SILA_PROTOCOL=sila2` to use the full SiLA 2 wire protocol
    /// expected by the Python `sila_sim` server.
    pub fn from_env() -> Self {
        match std::env::var("AXIOMLAB_SILA_ENDPOINT") {
            Ok(ep) if !ep.is_empty() => {
                if std::env::var("AXIOMLAB_SILA_PROTOCOL").as_deref() == Ok("sila2") {
                    tracing::info!(endpoint = %ep, "full SiLA 2 backend");
                    Self::full_sila(ep)
                } else {
                    tracing::info!(endpoint = %ep, "SiLA gRPC backend");
                    Self::grpc(ep)
                }
            }
            _ => {
                tracing::info!("SiLA simulator backend (no AXIOMLAB_SILA_ENDPOINT)");
                let faults = FaultProfile::from_env().unwrap_or_else(|error| {
                    tracing::warn!(%error, "invalid simulator fault profile; faults disabled");
                    FaultProfile::default()
                });
                Self::simulator_with_faults(faults)
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
            Backend::FullSila(lab) => lab.execute(action).await,
        }
    }

    /// Snapshot of vessel volumes (simulator only; `None` for gRPC).
    pub async fn vessel_snapshot(&self) -> Option<Value> {
        match &self.backend {
            Backend::Simulator(lab) => Some(lab.lock().await.vessel_snapshot()),
            Backend::Grpc(_) => None,
            Backend::FullSila(_) => None,
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
        let action = Action::new(
            "dispense",
            json!({"vessel_id": "tube_1", "volume_ul": 100.0}),
            RiskClass::LiquidHandling,
        );
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
