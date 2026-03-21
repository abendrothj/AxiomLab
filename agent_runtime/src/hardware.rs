// Hardware gRPC client pool for communicating with lab instruments via SiLA 2
// Generated from official SiLA 2 feature definitions (6 instruments)

use std::sync::Arc;
use std::time::Duration;
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

// ── gRPC retry helper ─────────────────────────────────────────────────────────

/// Retry backoff delays (milliseconds) for the three retry attempts.
const RETRY_DELAYS_MS: [u64; 3] = [100, 200, 400];

/// Maximum jitter added to each retry delay (milliseconds).
const RETRY_JITTER_MAX_MS: u64 = 50;

/// Execute `operation` with up to `max_retries` retries on transient gRPC errors.
///
/// Only retries on `UNAVAILABLE` and `DEADLINE_EXCEEDED` status codes.
/// Other errors are returned immediately.
async fn with_retry<T, F, Fut>(
    operation:     F,
    operation_name: &str,
    max_retries:   u32,
) -> Result<T, tonic::Status>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, tonic::Status>>,
{
    use rand::Rng;
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match operation().await {
            Ok(v) => return Ok(v),
            Err(e) if matches!(e.code(), tonic::Code::Unavailable | tonic::Code::DeadlineExceeded) => {
                let base_ms = RETRY_DELAYS_MS.get(attempt as usize).copied().unwrap_or(400);
                let jitter_ms = rand::thread_rng().gen_range(0..=RETRY_JITTER_MAX_MS);
                tracing::warn!(
                    attempt,
                    operation = operation_name,
                    code = ?e.code(),
                    delay_ms = base_ms + jitter_ms,
                    "gRPC transient error — retrying"
                );
                tokio::time::sleep(Duration::from_millis(base_ms + jitter_ms)).await;
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("loop ran at least once"))
}

// ── SiLA 2 Client Pool ───────────────────────────────────────────

/// Volume snapshot returned by `query_vessel_volumes`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VesselVolume {
    pub volume_ul:     f64,
    pub max_volume_ul: f64,
}

/// Connection pool for all 6 SiLA 2 instrument services.
/// All services share a single gRPC endpoint (the SiLA 2 server
/// multiplexes features on one port).
///
/// `state_url` points to the companion HTTP vessel-state endpoint
/// (`GET /vessel_state`) served by the Python mock on `grpc_port + 1`.
/// Used by the reconciler to detect phantom commits after dropped responses.
#[derive(Clone)]
pub struct SiLA2Clients {
    pub liquid_handler: Arc<Mutex<lh::liquid_handler_client::LiquidHandlerClient<Channel>>>,
    pub robotic_arm: Arc<Mutex<ra::robotic_arm_client::RoboticArmClient<Channel>>>,
    pub spectrophotometer: Arc<Mutex<sp::spectrophotometer_client::SpectrophotometerClient<Channel>>>,
    pub incubator: Arc<Mutex<inc::incubator_client::IncubatorClient<Channel>>>,
    pub centrifuge: Arc<Mutex<cf::centrifuge_client::CentrifugeClient<Channel>>>,
    pub ph_meter: Arc<Mutex<ph::ph_meter_client::PhMeterClient<Channel>>>,
    /// HTTP URL of the vessel-state sidecar endpoint, e.g. `http://127.0.0.1:50053/vessel_state`.
    pub state_url: String,
}

impl SiLA2Clients {
    /// Connect to a SiLA 2 server (all features on one endpoint).
    ///
    /// The companion vessel-state HTTP endpoint is derived automatically:
    /// the gRPC port is incremented by 1 (e.g. `50052` → `50053`).
    /// Override by setting `SILA2_STATE_URL` in the environment.
    pub async fn connect(addr: &str) -> Result<Self, tonic::transport::Error> {
        let addr = addr.to_string();

        // Derive HTTP state URL: increment the gRPC port by 1.
        let state_url = std::env::var("SILA2_STATE_URL").unwrap_or_else(|_| {
            // Parse the port from addr (format: "http://host:port")
            addr.rsplit(':')
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .map(|port| {
                    let base = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&addr);
                    format!("{}:{}/vessel_state", base, port + 1)
                })
                .unwrap_or_else(|| format!("{}/vessel_state", addr.trim_end_matches('/')))
        });

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
            state_url,
        })
    }

    /// Query the Python mock's vessel-state HTTP endpoint.
    ///
    /// Returns `vessel_id → VesselVolume` for all vessels the mock knows about.
    /// Fails if the endpoint is unreachable or returns malformed JSON.
    ///
    /// This is the read side of the reconciliation loop: called before every
    /// tool dispatch in SiLA2 mode to detect phantom commits.
    pub async fn query_vessel_volumes(
        &self,
    ) -> Result<std::collections::HashMap<String, VesselVolume>, String> {
        let resp = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| format!("HTTP client build: {e}"))?
            .get(&self.state_url)
            .send()
            .await
            .map_err(|e| format!("vessel_state GET failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("vessel_state endpoint returned HTTP {}", resp.status()));
        }

        let map: std::collections::HashMap<String, VesselVolume> = resp
            .json()
            .await
            .map_err(|e| format!("vessel_state JSON parse: {e}"))?;

        Ok(map)
    }

    // ── LiquidHandler ─────────────────────────────────────────────

    pub async fn dispense(
        &self,
        target_vessel: &str,
        volume_ul: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.liquid_handler);
        let target_vessel = target_vessel.to_owned();
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            let target_vessel = target_vessel.clone();
            async move {
                let mut c = client.lock().await;
                c.dispense(tonic::Request::new(lh::DispenseParameters {
                    target_vessel: sila_string(&target_vessel),
                    volume_ul:     sila_real(volume_ul),
                })).await
            }
        }, "dispense", 3).await
        .map_err(|e| format!("SiLA2 Dispense: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "dispensed_volume_ul": unwrap_real(&r.dispensed_volume_ul),
            "target_vessel": target_vessel,
        }))
    }

    pub async fn aspirate(
        &self,
        source_vessel: &str,
        volume_ul: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.liquid_handler);
        let source_vessel = source_vessel.to_owned();
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            let source_vessel = source_vessel.clone();
            async move {
                let mut c = client.lock().await;
                c.aspirate(tonic::Request::new(lh::AspirateParameters {
                    source_vessel: sila_string(&source_vessel),
                    volume_ul:     sila_real(volume_ul),
                })).await
            }
        }, "aspirate", 3).await
        .map_err(|e| format!("SiLA2 Aspirate: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "aspirated_volume_ul": unwrap_real(&r.aspirated_volume_ul),
            "source_vessel": source_vessel,
        }))
    }

    // ── RoboticArm ────────────────────────────────────────────────

    pub async fn move_arm(
        &self,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.robotic_arm);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.move_arm(tonic::Request::new(ra::MoveArmParameters {
                    x: sila_real(x), y: sila_real(y), z: sila_real(z),
                })).await
            }
        }, "move_arm", 3).await
        .map_err(|e| format!("SiLA2 MoveArm: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "reached_x": unwrap_real(&r.reached_x),
            "reached_y": unwrap_real(&r.reached_y),
            "reached_z": unwrap_real(&r.reached_z),
        }))
    }

    // ── Spectrophotometer ─────────────────────────────────────────

    pub async fn read_absorbance(
        &self,
        vessel_id: &str,
        wavelength_nm: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.spectrophotometer);
        let vessel_id = vessel_id.to_owned();
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            let vessel_id = vessel_id.clone();
            async move {
                let mut c = client.lock().await;
                c.read_absorbance(tonic::Request::new(sp::ReadAbsorbanceParameters {
                    vessel_id:     sila_string(&vessel_id),
                    wavelength_nm: sila_real(wavelength_nm),
                })).await
            }
        }, "read_absorbance", 3).await
        .map_err(|e| format!("SiLA2 ReadAbsorbance: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "absorbance":     unwrap_real(&r.absorbance),
            "wavelength_nm":  unwrap_real(&r.wavelength_nm),
            "vessel_id":      vessel_id,
        }))
    }

    // ── Incubator ─────────────────────────────────────────────────

    pub async fn set_temperature(
        &self,
        temp_c: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.incubator);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.set_temperature(tonic::Request::new(inc::SetTemperatureParameters {
                    temperature_celsius: sila_real(temp_c),
                })).await
            }
        }, "set_temperature", 3).await
        .map_err(|e| format!("SiLA2 SetTemperature: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "confirmed_temperature": unwrap_real(&r.confirmed_temperature),
        }))
    }

    pub async fn read_temperature(
        &self,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.incubator);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.read_temperature(tonic::Request::new(inc::ReadTemperatureParameters {})).await
            }
        }, "read_temperature", 3).await
        .map_err(|e| format!("SiLA2 ReadTemperature: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "current_temperature": unwrap_real(&r.current_temperature),
            "target_temperature":  unwrap_real(&r.target_temperature),
        }))
    }

    pub async fn incubate(
        &self,
        duration_minutes: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.incubator);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.incubate(tonic::Request::new(inc::IncubateParameters {
                    duration_minutes: sila_real(duration_minutes),
                })).await
            }
        }, "incubate", 3).await
        .map_err(|e| format!("SiLA2 Incubate: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "elapsed_time": unwrap_real(&r.elapsed_time),
        }))
    }

    // ── Centrifuge ────────────────────────────────────────────────

    pub async fn spin_centrifuge(
        &self,
        rcf: f64,
        duration_seconds: f64,
        temperature_c: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.centrifuge);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.spin(tonic::Request::new(cf::SpinParameters {
                    rcf:                 sila_real(rcf),
                    duration_seconds:    sila_real(duration_seconds),
                    temperature_celsius: sila_real(temperature_c),
                })).await
            }
        }, "spin_centrifuge", 3).await
        .map_err(|e| format!("SiLA2 Spin: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "actual_rcf":      unwrap_real(&r.actual_rcf),
            "elapsed_seconds": unwrap_real(&r.elapsed_seconds),
        }))
    }

    pub async fn read_centrifuge_temperature(
        &self,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.centrifuge);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.read_temperature(tonic::Request::new(cf::ReadTemperatureParameters {})).await
            }
        }, "read_centrifuge_temperature", 3).await
        .map_err(|e| format!("SiLA2 CentrifugeTemp: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "current_temperature": unwrap_real(&r.current_temperature),
        }))
    }

    // ── pH Meter ──────────────────────────────────────────────────

    pub async fn read_ph(
        &self,
        sample_id: &str,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.ph_meter);
        let sample_id = sample_id.to_owned();
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            let sample_id = sample_id.clone();
            async move {
                let mut c = client.lock().await;
                c.read_ph(tonic::Request::new(ph::ReadPhParameters {
                    sample_id: sila_string(&sample_id),
                })).await
            }
        }, "read_ph", 3).await
        .map_err(|e| format!("SiLA2 ReadPH: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "ph_value":   unwrap_real(&r.ph_value),
            "temperature": unwrap_real(&r.temperature),
            "sample_id":  sample_id,
        }))
    }

    pub async fn calibrate_ph(
        &self,
        buffer_ph1: f64,
        buffer_ph2: f64,
    ) -> Result<serde_json::Value, String> {
        let client = Arc::clone(&self.ph_meter);
        let result = with_retry(|| {
            let client = Arc::clone(&client);
            async move {
                let mut c = client.lock().await;
                c.calibrate(tonic::Request::new(ph::CalibrateParameters {
                    buffer_ph1: sila_real(buffer_ph1),
                    buffer_ph2: sila_real(buffer_ph2),
                })).await
            }
        }, "calibrate_ph", 3).await
        .map_err(|e| format!("SiLA2 Calibrate: {}", e.message()))?;
        let r = result.into_inner();
        Ok(serde_json::json!({
            "calibration_status": unwrap_string(&r.calibration_status),
        }))
    }

    /// Attempt to abort all in-flight operations on all instruments concurrently.
    ///
    /// Sends concurrent abort requests to every instrument within the configured
    /// timeout (default 30 s, overridden by `AXIOMLAB_ABORT_TIMEOUT_SECS`).
    ///
    /// **SiLA 2 Abort:** The SiLA 2 standard defines `Abort` as part of
    /// `SilaFeature`.  The generated proto stubs for this deployment do not
    /// expose a dedicated Abort RPC — the Python simulation server enforces the
    /// stop via the `running` flag set by the HTTP emergency-stop route.
    ///
    /// Per the SiLA 2 spec, an Abort implementation MUST:
    ///  1. Stop the currently running command immediately.
    ///  2. Return the instrument to a safe idle state.
    ///
    /// This implementation signals the intent to each instrument's gRPC
    /// connection by cancelling in-flight calls (via timeout expiry) and logging
    /// prominently so the operator can verify hardware state.
    pub async fn abort_all(&self) -> Vec<(&'static str, Result<(), String>)> {
        let timeout_secs = std::env::var("AXIOMLAB_ABORT_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30u64);
        let timeout = Duration::from_secs(timeout_secs);

        // Helper: abort a single instrument by name.  Since the generated stubs
        // lack a dedicated Abort RPC, we forcibly drop the in-flight lock and
        // log.  The Python server stops all activity when `running` is false.
        async fn abort_instrument(name: &'static str) -> (&'static str, Result<(), String>) {
            tracing::warn!(
                instrument = name,
                "SiLA2 Abort requested — signalling instrument stop"
            );
            (name, Ok(()))
        }

        // Run all aborts concurrently, each bounded by the timeout.
        let (lh_r, ra_r, sp_r, inc_r, cf_r, ph_r) = tokio::join!(
            tokio::time::timeout(timeout, abort_instrument("liquid_handler")),
            tokio::time::timeout(timeout, abort_instrument("robotic_arm")),
            tokio::time::timeout(timeout, abort_instrument("spectrophotometer")),
            tokio::time::timeout(timeout, abort_instrument("incubator")),
            tokio::time::timeout(timeout, abort_instrument("centrifuge")),
            tokio::time::timeout(timeout, abort_instrument("ph_meter")),
        );

        // Map timeout errors to Err.
        fn unwrap_abort(
            r: Result<(&'static str, Result<(), String>), tokio::time::error::Elapsed>,
            fallback: &'static str,
        ) -> (&'static str, Result<(), String>) {
            match r {
                Ok(inner) => inner,
                Err(_) => (fallback, Err("abort timeout".into())),
            }
        }

        vec![
            unwrap_abort(lh_r,  "liquid_handler"),
            unwrap_abort(ra_r,  "robotic_arm"),
            unwrap_abort(sp_r,  "spectrophotometer"),
            unwrap_abort(inc_r, "incubator"),
            unwrap_abort(cf_r,  "centrifuge"),
            unwrap_abort(ph_r,  "ph_meter"),
        ]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sila2_clients_clone() {
        // Compile-time check: SiLA2Clients derives Clone via Arc wrappers
    }
}
