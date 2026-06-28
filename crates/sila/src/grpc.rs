//! gRPC backend — thin adapters over the generated SiLA instrument clients.
//!
//! Each call connects to the configured endpoint and dispatches one RPC. No
//! business logic lives here; safety decisions were already made by the gate
//! pipeline before `execute` is reached.

use crate::SilaError;
use crate::pb;
use axiom_types::Action;
use serde_json::{Value, json};

/// gRPC endpoints for the instrument services.
///
/// In the common deployment all instruments share one endpoint (the SiLA mock
/// or a gateway); override per-service as needed.
#[derive(Debug, Clone)]
pub struct GrpcLab {
    pub liquid: String,
    pub spectro: String,
    pub thermal: String,
}

impl GrpcLab {
    /// All services on a single endpoint.
    pub fn single(endpoint: impl Into<String>) -> Self {
        let e = endpoint.into();
        Self { liquid: e.clone(), spectro: e.clone(), thermal: e }
    }

    pub async fn execute(&self, action: &Action) -> Result<Value, SilaError> {
        let p = &action.params;
        match action.tool.as_str() {
            "dispense" => {
                let mut c = pb::liquid_dispenser_client::LiquidDispenserClient::connect(self.liquid.clone())
                    .await
                    .map_err(grpc_conn)?;
                let req = pb::DispenseRequest {
                    target_container: str_of(p, "vessel_id").or_else(|_| str_of(p, "target_container"))?,
                    volume_ul: f64_of(p, "volume_ul")? as f32,
                    source_reagent: str_of(p, "reagent").unwrap_or_default(),
                };
                let resp = c.dispense_liquid(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({ "success": resp.success, "error_message": resp.error_message, "actual_volume_dispensed": resp.actual_volume_dispensed }))
            }
            "aspirate" => {
                let mut c = pb::liquid_dispenser_client::LiquidDispenserClient::connect(self.liquid.clone())
                    .await
                    .map_err(grpc_conn)?;
                let req = pb::AspirateRequest {
                    source_container: str_of(p, "vessel_id").or_else(|_| str_of(p, "source_container"))?,
                    volume_ul: f64_of(p, "volume_ul")? as f32,
                };
                let resp = c.aspirate_fluid(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({ "success": resp.success, "error_message": resp.error_message, "actual_volume_aspirated": resp.actual_volume_aspirated }))
            }
            "read_absorbance" => {
                let mut c = pb::spectrophotometer_client::SpectrophotometerClient::connect(self.spectro.clone())
                    .await
                    .map_err(grpc_conn)?;
                let req = pb::SpectroRequest {
                    target_container: str_of(p, "vessel_id").or_else(|_| str_of(p, "target_container"))?,
                    wavelength_nm: f64_of(p, "wavelength_nm").unwrap_or(500.0) as f32,
                };
                let resp = c.read_absorbance(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({ "success": resp.success, "error_message": resp.error_message, "absorbance_value": resp.absorbance_value, "actual_wavelength_nm": resp.actual_wavelength_nm }))
            }
            "set_temperature" => {
                let mut c = pb::thermal_controller_client::ThermalControllerClient::connect(self.thermal.clone())
                    .await
                    .map_err(grpc_conn)?;
                let req = pb::TemperatureSetRequest {
                    target_plate: str_of(p, "device_id").or_else(|_| str_of(p, "target_plate"))?,
                    target_temp_c: f64_of(p, "target_temp_c")? as f32,
                    ramp_rate_c_per_min: f64_of(p, "ramp_rate_c_per_min").unwrap_or(0.0) as f32,
                };
                let resp = c.set_temperature(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({ "success": resp.success, "error_message": resp.error_message, "final_temp_c": resp.final_temp_c }))
            }
            "read_temperature" => {
                let mut c = pb::thermal_controller_client::ThermalControllerClient::connect(self.thermal.clone())
                    .await
                    .map_err(grpc_conn)?;
                let req = pb::TemperatureReadRequest {
                    target_plate: str_of(p, "device_id").or_else(|_| str_of(p, "target_plate"))?,
                };
                let resp = c.read_temperature(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({ "success": resp.success, "error_message": resp.error_message, "current_temp_c": resp.current_temp_c }))
            }
            other => Err(SilaError::UnknownTool(format!(
                "{other} has no gRPC mapping (only liquid/spectro/thermal services are wired)"
            ))),
        }
    }
}

fn grpc_conn(e: tonic::transport::Error) -> SilaError {
    SilaError::Transport(e.to_string())
}
fn grpc_status(s: tonic::Status) -> SilaError {
    SilaError::Rpc(s.to_string())
}

fn str_of(p: &Value, key: &str) -> Result<String, SilaError> {
    p.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| SilaError::MissingParam(key.to_string()))
}
fn f64_of(p: &Value, key: &str) -> Result<f64, SilaError> {
    p.get(key).and_then(|v| v.as_f64()).ok_or_else(|| SilaError::MissingParam(key.to_string()))
}
