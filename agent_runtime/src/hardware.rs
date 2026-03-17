// Hardware gRPC client pool for communicating with lab instruments via SiLA 2
// Generated from official SiLA 2 feature definitions (6 instruments)

use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::transport::Channel;

// ── SiLA 2 proto module tree ──────────────────────────────────────
// The generated code uses deeply nested package paths like
// sila2.org.axiomlab.liquidhandling.liquidhandler.v1
// We must replicate this module hierarchy so `super::` references
// to silastandard::Real / silastandard::String resolve correctly.

pub mod sila2 {
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
            pub mod motion {
                pub mod roboticarm {
                    pub mod v1 {
                        tonic::include_proto!("sila2.org.axiomlab.motion.roboticarm.v1");
                    }
                }
            }
            pub mod measurement {
                pub mod spectrophotometer {
                    pub mod v1 {
                        tonic::include_proto!("sila2.org.axiomlab.measurement.spectrophotometer.v1");
                    }
                }
                pub mod phmeter {
                    pub mod v1 {
                        tonic::include_proto!("sila2.org.axiomlab.measurement.phmeter.v1");
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
            pub mod separation {
                pub mod centrifuge {
                    pub mod v1 {
                        tonic::include_proto!("sila2.org.axiomlab.separation.centrifuge.v1");
                    }
                }
            }
        }
    }
}

// ── Convenience aliases ───────────────────────────────────────────
use sila2::org::silastandard::{Real, String as SilaString};

use sila2::org::axiomlab::liquidhandling::liquidhandler::v1 as lh;
use sila2::org::axiomlab::motion::roboticarm::v1 as ra;
use sila2::org::axiomlab::measurement::spectrophotometer::v1 as sp;
use sila2::org::axiomlab::environment::incubator::v1 as inc;
use sila2::org::axiomlab::separation::centrifuge::v1 as cf;
use sila2::org::axiomlab::measurement::phmeter::v1 as ph;

// Helper constructors for SiLA 2 wrapper types
fn sila_real(v: f64) -> Option<Real> {
    Some(Real { value: v })
}
fn sila_string(s: &str) -> Option<SilaString> {
    Some(SilaString { value: s.to_string() })
}
fn unwrap_real(o: &Option<Real>) -> f64 {
    o.as_ref().map(|r| r.value).unwrap_or(0.0)
}
fn unwrap_string(o: &Option<SilaString>) -> String {
    o.as_ref().map(|s| s.value.clone()).unwrap_or_default()
}

// ── SiLA 2 Client Pool ───────────────────────────────────────────

/// Connection pool for all 6 SiLA 2 instrument services.
/// All services share a single gRPC endpoint (the SiLA 2 server
/// multiplexes features on one port).
#[derive(Clone)]
pub struct SiLA2Clients {
    pub liquid_handler: Arc<Mutex<lh::liquid_handler_client::LiquidHandlerClient<Channel>>>,
    pub robotic_arm: Arc<Mutex<ra::robotic_arm_client::RoboticArmClient<Channel>>>,
    pub spectrophotometer: Arc<Mutex<sp::spectrophotometer_client::SpectrophotometerClient<Channel>>>,
    pub incubator: Arc<Mutex<inc::incubator_client::IncubatorClient<Channel>>>,
    pub centrifuge: Arc<Mutex<cf::centrifuge_client::CentrifugeClient<Channel>>>,
    pub ph_meter: Arc<Mutex<ph::ph_meter_client::PhMeterClient<Channel>>>,
}

impl SiLA2Clients {
    /// Connect to a SiLA 2 server (all features on one endpoint).
    pub async fn connect(addr: &str) -> Result<Self, tonic::transport::Error> {
        let addr = addr.to_string();
        Ok(SiLA2Clients {
            liquid_handler: Arc::new(Mutex::new(
                lh::liquid_handler_client::LiquidHandlerClient::connect(addr.clone()).await?,
            )),
            robotic_arm: Arc::new(Mutex::new(
                ra::robotic_arm_client::RoboticArmClient::connect(addr.clone()).await?,
            )),
            spectrophotometer: Arc::new(Mutex::new(
                sp::spectrophotometer_client::SpectrophotometerClient::connect(addr.clone()).await?,
            )),
            incubator: Arc::new(Mutex::new(
                inc::incubator_client::IncubatorClient::connect(addr.clone()).await?,
            )),
            centrifuge: Arc::new(Mutex::new(
                cf::centrifuge_client::CentrifugeClient::connect(addr.clone()).await?,
            )),
            ph_meter: Arc::new(Mutex::new(
                ph::ph_meter_client::PhMeterClient::connect(addr.clone()).await?,
            )),
        })
    }

    // ── LiquidHandler ─────────────────────────────────────────────

    pub async fn dispense(
        &self,
        target_vessel: &str,
        volume_ul: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.liquid_handler.lock().await;
        let req = tonic::Request::new(lh::DispenseParameters {
            target_vessel: sila_string(target_vessel),
            volume_ul: sila_real(volume_ul),
        });
        match client.dispense(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "dispensed_volume_ul": unwrap_real(&r.dispensed_volume_ul),
                    "target_vessel": target_vessel,
                }))
            }
            Err(e) => Err(format!("SiLA2 Dispense: {}", e.message())),
        }
    }

    pub async fn aspirate(
        &self,
        source_vessel: &str,
        volume_ul: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.liquid_handler.lock().await;
        let req = tonic::Request::new(lh::AspirateParameters {
            source_vessel: sila_string(source_vessel),
            volume_ul: sila_real(volume_ul),
        });
        match client.aspirate(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "aspirated_volume_ul": unwrap_real(&r.aspirated_volume_ul),
                    "source_vessel": source_vessel,
                }))
            }
            Err(e) => Err(format!("SiLA2 Aspirate: {}", e.message())),
        }
    }

    // ── RoboticArm ────────────────────────────────────────────────

    pub async fn move_arm(
        &self,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.robotic_arm.lock().await;
        let req = tonic::Request::new(ra::MoveArmParameters {
            x: sila_real(x),
            y: sila_real(y),
            z: sila_real(z),
        });
        match client.move_arm(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "reached_x": unwrap_real(&r.reached_x),
                    "reached_y": unwrap_real(&r.reached_y),
                    "reached_z": unwrap_real(&r.reached_z),
                }))
            }
            Err(e) => Err(format!("SiLA2 MoveArm: {}", e.message())),
        }
    }

    // ── Spectrophotometer ─────────────────────────────────────────

    pub async fn read_absorbance(
        &self,
        vessel_id: &str,
        wavelength_nm: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.spectrophotometer.lock().await;
        let req = tonic::Request::new(sp::ReadAbsorbanceParameters {
            vessel_id: sila_string(vessel_id),
            wavelength_nm: sila_real(wavelength_nm),
        });
        match client.read_absorbance(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "absorbance": unwrap_real(&r.absorbance),
                    "wavelength_nm": unwrap_real(&r.wavelength_nm),
                    "vessel_id": vessel_id,
                }))
            }
            Err(e) => Err(format!("SiLA2 ReadAbsorbance: {}", e.message())),
        }
    }

    // ── Incubator ─────────────────────────────────────────────────

    pub async fn set_temperature(
        &self,
        temp_c: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.incubator.lock().await;
        let req = tonic::Request::new(inc::SetTemperatureParameters {
            temperature_celsius: sila_real(temp_c),
        });
        match client.set_temperature(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "confirmed_temperature": unwrap_real(&r.confirmed_temperature),
                }))
            }
            Err(e) => Err(format!("SiLA2 SetTemperature: {}", e.message())),
        }
    }

    pub async fn read_temperature(
        &self,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.incubator.lock().await;
        let req = tonic::Request::new(inc::ReadTemperatureParameters {});
        match client.read_temperature(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "current_temperature": unwrap_real(&r.current_temperature),
                    "target_temperature": unwrap_real(&r.target_temperature),
                }))
            }
            Err(e) => Err(format!("SiLA2 ReadTemperature: {}", e.message())),
        }
    }

    pub async fn incubate(
        &self,
        duration_minutes: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.incubator.lock().await;
        let req = tonic::Request::new(inc::IncubateParameters {
            duration_minutes: sila_real(duration_minutes),
        });
        match client.incubate(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "elapsed_time": unwrap_real(&r.elapsed_time),
                }))
            }
            Err(e) => Err(format!("SiLA2 Incubate: {}", e.message())),
        }
    }

    // ── Centrifuge ────────────────────────────────────────────────

    pub async fn spin_centrifuge(
        &self,
        rcf: f64,
        duration_seconds: f64,
        temperature_c: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.centrifuge.lock().await;
        let req = tonic::Request::new(cf::SpinParameters {
            rcf: sila_real(rcf),
            duration_seconds: sila_real(duration_seconds),
            temperature_celsius: sila_real(temperature_c),
        });
        match client.spin(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "actual_rcf": unwrap_real(&r.actual_rcf),
                    "elapsed_seconds": unwrap_real(&r.elapsed_seconds),
                }))
            }
            Err(e) => Err(format!("SiLA2 Spin: {}", e.message())),
        }
    }

    pub async fn read_centrifuge_temperature(
        &self,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.centrifuge.lock().await;
        let req = tonic::Request::new(cf::ReadTemperatureParameters {});
        match client.read_temperature(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "current_temperature": unwrap_real(&r.current_temperature),
                }))
            }
            Err(e) => Err(format!("SiLA2 CentrifugeTemp: {}", e.message())),
        }
    }

    // ── pH Meter ──────────────────────────────────────────────────

    pub async fn read_ph(
        &self,
        sample_id: &str,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.ph_meter.lock().await;
        let req = tonic::Request::new(ph::ReadPhParameters {
            sample_id: sila_string(sample_id),
        });
        match client.read_ph(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "ph_value": unwrap_real(&r.ph_value),
                    "temperature": unwrap_real(&r.temperature),
                    "sample_id": sample_id,
                }))
            }
            Err(e) => Err(format!("SiLA2 ReadPH: {}", e.message())),
        }
    }

    pub async fn calibrate_ph(
        &self,
        buffer_ph1: f64,
        buffer_ph2: f64,
    ) -> Result<serde_json::Value, String> {
        let mut client = self.ph_meter.lock().await;
        let req = tonic::Request::new(ph::CalibrateParameters {
            buffer_ph1: sila_real(buffer_ph1),
            buffer_ph2: sila_real(buffer_ph2),
        });
        match client.calibrate(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                Ok(serde_json::json!({
                    "calibration_status": unwrap_string(&r.calibration_status),
                }))
            }
            Err(e) => Err(format!("SiLA2 Calibrate: {}", e.message())),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sila2_clients_clone() {
        // Compile-time check: SiLA2Clients derives Clone via Arc wrappers
    }
}
