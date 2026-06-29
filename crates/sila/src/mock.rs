//! A gRPC instrument server implementing `instruments.proto`, backed by the
//! [`SimLab`](crate::SimLab) physics model.
//!
//! This is what `SilaClients::grpc` talks to when there is no real hardware: it
//! exercises the exact client + transport path end to end. Run it standalone via
//! the `mock-instrument-server` binary and point the system at it with
//! `AXIOMLAB_SILA_ENDPOINT`, or spawn it in-process with [`spawn_mock_server`].
//!
//! Note: this speaks our simplified `instruments.proto`. The Python `sila_sim`
//! server speaks full SiLA 2 (different packages, `SiLAFramework.proto`, wrapped
//! standard types); talking to *that* would need a separate SiLA 2 client layer.

use crate::pb;
use crate::sim::SimLab;
use crate::SilaError;
use axiom_types::{Action, RiskClass};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

#[derive(Clone)]
struct MockInstruments {
    lab: Arc<Mutex<SimLab>>,
}

impl MockInstruments {
    fn new() -> Self {
        Self { lab: Arc::new(Mutex::new(SimLab::new())) }
    }

    async fn run(&self, tool: &str, params: Value) -> Result<Value, SilaError> {
        self.lab.lock().await.execute(&Action::new(tool, params, RiskClass::ReadOnly))
    }
}

#[tonic::async_trait]
impl pb::liquid_dispenser_server::LiquidDispenser for MockInstruments {
    async fn dispense_liquid(
        &self,
        req: Request<pb::DispenseRequest>,
    ) -> Result<Response<pb::DispenseResponse>, Status> {
        let r = req.into_inner();
        let params = json!({
            "vessel_id": r.target_container,
            "volume_ul": r.volume_ul as f64,
            "reagent": r.source_reagent,
        });
        match self.run("dispense", params).await {
            Ok(v) => Ok(Response::new(pb::DispenseResponse {
                success: true,
                error_message: String::new(),
                actual_volume_dispensed: v["actual_volume_dispensed"].as_f64().unwrap_or(r.volume_ul as f64) as f32,
            })),
            // A physical limit (overflow) is reported in-band, like real hardware.
            Err(SilaError::Physics(m)) => Ok(Response::new(pb::DispenseResponse {
                success: false,
                error_message: m,
                actual_volume_dispensed: 0.0,
            })),
            Err(e) => Err(Status::invalid_argument(e.to_string())),
        }
    }

    async fn aspirate_fluid(
        &self,
        req: Request<pb::AspirateRequest>,
    ) -> Result<Response<pb::AspirateResponse>, Status> {
        let r = req.into_inner();
        let params = json!({ "vessel_id": r.source_container, "volume_ul": r.volume_ul as f64 });
        match self.run("aspirate", params).await {
            Ok(v) => Ok(Response::new(pb::AspirateResponse {
                success: true,
                error_message: String::new(),
                actual_volume_aspirated: v["actual_volume_aspirated"].as_f64().unwrap_or(r.volume_ul as f64) as f32,
            })),
            Err(SilaError::Physics(m)) => Ok(Response::new(pb::AspirateResponse {
                success: false,
                error_message: m,
                actual_volume_aspirated: 0.0,
            })),
            Err(e) => Err(Status::invalid_argument(e.to_string())),
        }
    }
}

#[tonic::async_trait]
impl pb::spectrophotometer_server::Spectrophotometer for MockInstruments {
    async fn read_absorbance(
        &self,
        req: Request<pb::SpectroRequest>,
    ) -> Result<Response<pb::SpectroResponse>, Status> {
        let r = req.into_inner();
        let params = json!({ "vessel_id": r.target_container, "wavelength_nm": r.wavelength_nm as f64 });
        match self.run("read_absorbance", params).await {
            Ok(v) => Ok(Response::new(pb::SpectroResponse {
                success: true,
                error_message: String::new(),
                absorbance_value: v["absorbance_value"].as_f64().unwrap_or(0.0) as f32,
                actual_wavelength_nm: r.wavelength_nm,
            })),
            Err(e) => Err(Status::invalid_argument(e.to_string())),
        }
    }
}

#[tonic::async_trait]
impl pb::thermal_controller_server::ThermalController for MockInstruments {
    async fn set_temperature(
        &self,
        req: Request<pb::TemperatureSetRequest>,
    ) -> Result<Response<pb::TemperatureResponse>, Status> {
        let r = req.into_inner();
        let params = json!({ "device_id": r.target_plate, "target_temp_c": r.target_temp_c as f64 });
        match self.run("set_temperature", params).await {
            Ok(v) => Ok(Response::new(pb::TemperatureResponse {
                success: true,
                error_message: String::new(),
                final_temp_c: v["final_temp_c"].as_f64().unwrap_or(r.target_temp_c as f64) as f32,
                current_temp_c: v["final_temp_c"].as_f64().unwrap_or(r.target_temp_c as f64) as f32,
            })),
            Err(e) => Err(Status::invalid_argument(e.to_string())),
        }
    }

    async fn read_temperature(
        &self,
        req: Request<pb::TemperatureReadRequest>,
    ) -> Result<Response<pb::TemperatureResponse>, Status> {
        let r = req.into_inner();
        let params = json!({ "device_id": r.target_plate });
        match self.run("read_temperature", params).await {
            Ok(v) => Ok(Response::new(pb::TemperatureResponse {
                success: true,
                error_message: String::new(),
                final_temp_c: 0.0,
                current_temp_c: v["current_temp_c"].as_f64().unwrap_or(22.0) as f32,
            })),
            Err(e) => Err(Status::invalid_argument(e.to_string())),
        }
    }
}

fn router() -> tonic::transport::server::Router {
    let mock = MockInstruments::new();
    tonic::transport::Server::builder()
        .add_service(pb::liquid_dispenser_server::LiquidDispenserServer::new(mock.clone()))
        .add_service(pb::spectrophotometer_server::SpectrophotometerServer::new(mock.clone()))
        .add_service(pb::thermal_controller_server::ThermalControllerServer::new(mock))
}

/// Serve the mock instruments on `addr` until the process exits.
pub async fn serve(addr: SocketAddr) -> Result<(), tonic::transport::Error> {
    tracing::info!(%addr, "mock instrument server (instruments.proto) listening");
    router().serve(addr).await
}

/// Spawn the mock server on an ephemeral port. Returns its endpoint URL and the
/// task handle (abort it to stop). For tests and local end-to-end runs.
pub async fn spawn_mock_server() -> std::io::Result<(String, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let endpoint = format!("http://{addr}");
    let stream = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        let _ = router().serve_with_incoming(stream).await;
    });
    Ok((endpoint, handle))
}
