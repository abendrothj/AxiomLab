//! Full SiLA 2 backend — adapters over the Python `sila_sim` wire protocol.
//!
//! This is separate from `grpc.rs`: `grpc.rs` speaks AxiomLab's simplified
//! `instruments.proto`, while this module speaks the generated SiLA 2 feature
//! packages with standard wrapper types (`sila2.org.silastandard.Real/String`).

use crate::SilaError;
use crate::sila2_pb::org::axiomlab::{
    environment::incubator::v1 as incubator, liquidhandling::liquidhandler::v1 as liquidhandler,
    measurement::spectrophotometer::v1 as spectrophotometer,
};
use crate::sila2_pb::org::silastandard as standard;
use axiom_types::Action;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct FullSilaLab {
    pub liquid: String,
    pub spectro: String,
    pub incubator: String,
}

impl FullSilaLab {
    pub fn single(endpoint: impl Into<String>) -> Self {
        let e = endpoint.into();
        Self {
            liquid: e.clone(),
            spectro: e.clone(),
            incubator: e,
        }
    }

    pub async fn execute(&self, action: &Action) -> Result<Value, SilaError> {
        let p = &action.params;
        match action.tool.as_str() {
            "dispense" => {
                let mut c = liquidhandler::liquid_handler_client::LiquidHandlerClient::connect(
                    self.liquid.clone(),
                )
                .await
                .map_err(grpc_conn)?;
                let req = liquidhandler::DispenseParameters {
                    target_vessel: Some(s(
                        str_of(p, "vessel_id").or_else(|_| str_of(p, "target_container"))?
                    )),
                    volume_ul: Some(r(f64_of(p, "volume_ul")?)),
                };
                let resp = c.dispense(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({
                    "success": true,
                    "error_message": "",
                    "actual_volume_dispensed": real(resp.dispensed_volume_ul, "DispensedVolumeUl")?,
                }))
            }
            "aspirate" => {
                let mut c = liquidhandler::liquid_handler_client::LiquidHandlerClient::connect(
                    self.liquid.clone(),
                )
                .await
                .map_err(grpc_conn)?;
                let req = liquidhandler::AspirateParameters {
                    source_vessel: Some(s(
                        str_of(p, "vessel_id").or_else(|_| str_of(p, "source_container"))?
                    )),
                    volume_ul: Some(r(f64_of(p, "volume_ul")?)),
                };
                let resp = c.aspirate(req).await.map_err(grpc_status)?.into_inner();
                Ok(json!({
                    "success": true,
                    "error_message": "",
                    "actual_volume_aspirated": real(resp.aspirated_volume_ul, "AspiratedVolumeUl")?,
                }))
            }
            "read_absorbance" => {
                let mut c =
                    spectrophotometer::spectrophotometer_client::SpectrophotometerClient::connect(
                        self.spectro.clone(),
                    )
                    .await
                    .map_err(grpc_conn)?;
                let req = spectrophotometer::ReadAbsorbanceParameters {
                    vessel_id: Some(s(
                        str_of(p, "vessel_id").or_else(|_| str_of(p, "target_container"))?
                    )),
                    wavelength_nm: Some(r(f64_of(p, "wavelength_nm").unwrap_or(500.0))),
                };
                let resp = c
                    .read_absorbance(req)
                    .await
                    .map_err(grpc_status)?
                    .into_inner();
                Ok(json!({
                    "success": true,
                    "error_message": "",
                    "absorbance_value": real(resp.absorbance, "Absorbance")?,
                    "actual_wavelength_nm": real(resp.wavelength_nm, "WavelengthNm")?,
                }))
            }
            "set_temperature" => {
                let mut c =
                    incubator::incubator_client::IncubatorClient::connect(self.incubator.clone())
                        .await
                        .map_err(grpc_conn)?;
                let req = incubator::SetTemperatureParameters {
                    temperature_celsius: Some(r(f64_of(p, "target_temp_c")?)),
                };
                let resp = c
                    .set_temperature(req)
                    .await
                    .map_err(grpc_status)?
                    .into_inner();
                let confirmed = real(resp.confirmed_temperature, "ConfirmedTemperature")?;
                Ok(json!({ "success": true, "error_message": "", "final_temp_c": confirmed }))
            }
            "read_temperature" => {
                let mut c =
                    incubator::incubator_client::IncubatorClient::connect(self.incubator.clone())
                        .await
                        .map_err(grpc_conn)?;
                let resp = c
                    .read_temperature(incubator::ReadTemperatureParameters {})
                    .await
                    .map_err(grpc_status)?
                    .into_inner();
                Ok(json!({
                    "success": true,
                    "error_message": "",
                    "current_temp_c": real(resp.current_temperature, "CurrentTemperature")?,
                    "target_temp_c": real(resp.target_temperature, "TargetTemperature")?,
                }))
            }
            other => Err(SilaError::UnknownTool(format!(
                "{other} has no full SiLA 2 mapping (liquid handler, spectrophotometer, incubator are wired)"
            ))),
        }
    }
}

fn s(value: String) -> standard::String {
    standard::String { value }
}

fn r(value: f64) -> standard::Real {
    standard::Real { value }
}

fn real(value: Option<standard::Real>, name: &str) -> Result<f64, SilaError> {
    value
        .map(|v| v.value)
        .ok_or_else(|| SilaError::Rpc(format!("missing SiLA response field: {name}")))
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
    p.get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| SilaError::MissingParam(key.to_string()))
}
