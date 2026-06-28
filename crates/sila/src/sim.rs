//! Offline physics simulator backend.
//!
//! Mirrors the Beer-Lambert vessel model of the Python SiLA 2 mock so agent
//! behaviour developed offline transfers faithfully to hardware runs. Same
//! `execute` interface as the gRPC backend — two backends, one contract.

use crate::SilaError;
use axiom_types::Action;
use rand::Rng;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone)]
struct VesselParams {
    volume_ul: f64,
    max_volume_ul: f64,
    /// ε in Beer-Lambert: absorbance per unit fill-fraction per cm path length.
    epsilon: f64,
    path_length_cm: f64,
}

impl VesselParams {
    fn unknown() -> Self {
        Self { volume_ul: 0.0, max_volume_ul: 50_000.0, epsilon: 1.0, path_length_cm: 1.0 }
    }
}

/// In-memory laboratory physics model for the simulator backend.
pub struct SimLab {
    vessels: HashMap<String, VesselParams>,
    /// Device temperatures in °C (thermal controllers / incubators).
    temps_c: HashMap<String, f64>,
}

impl Default for SimLab {
    fn default() -> Self {
        Self::new()
    }
}

impl SimLab {
    pub fn new() -> Self {
        let mut vessels = HashMap::new();
        // Parameters match axiomlab_sim/_vessel_state_python.py.
        for (id, max, eps, path, init) in [
            ("beaker_A", 50_000.0, 1.2, 1.0, 0.0),
            ("beaker_B", 50_000.0, 0.8, 1.0, 0.0),
            ("tube_1", 2_000.0, 1.5, 1.0, 0.0),
            ("tube_2", 2_000.0, 1.5, 1.0, 0.0),
            ("tube_3", 2_000.0, 1.5, 1.0, 0.0),
            ("plate_well_A1", 300.0, 2.0, 0.5, 0.0),
            ("plate_well_B1", 300.0, 2.0, 0.5, 0.0),
            ("reservoir", 200_000.0, 0.3, 1.0, 100_000.0),
        ] {
            vessels.insert(
                id.to_string(),
                VesselParams { volume_ul: init, max_volume_ul: max, epsilon: eps, path_length_cm: path },
            );
        }
        Self { vessels, temps_c: HashMap::new() }
    }

    /// Dispatch a proposed action against the physics model.
    pub fn execute(&mut self, action: &Action) -> Result<Value, SilaError> {
        let p = &action.params;
        match action.tool.as_str() {
            "dispense" => {
                let (vessel, volume) = (vessel_of(p)?, f64_of(p, "volume_ul")?);
                let new_vol = self.dispense(&vessel, volume)?;
                Ok(json!({ "success": true, "vessel_id": vessel, "actual_volume_dispensed": volume, "vessel_volume_ul": new_vol }))
            }
            "aspirate" => {
                let (vessel, volume) = (vessel_of(p)?, f64_of(p, "volume_ul")?);
                let new_vol = self.aspirate(&vessel, volume)?;
                Ok(json!({ "success": true, "vessel_id": vessel, "actual_volume_aspirated": volume, "vessel_volume_ul": new_vol }))
            }
            "read_absorbance" => {
                let vessel = vessel_of(p)?;
                let wl = f64_of(p, "wavelength_nm").unwrap_or(500.0);
                let a = self.read_absorbance(&vessel, wl);
                Ok(json!({ "success": true, "vessel_id": vessel, "absorbance_value": a, "actual_wavelength_nm": wl }))
            }
            "read_ph" => {
                let vessel = vessel_of(p)?;
                let ph = self.read_ph(&vessel);
                Ok(json!({ "success": true, "vessel_id": vessel, "ph_value": ph }))
            }
            "read_temperature" => {
                let device = device_of(p)?;
                let t = *self.temps_c.get(&device).unwrap_or(&22.0);
                Ok(json!({ "success": true, "device_id": device, "current_temp_c": t }))
            }
            "set_temperature" => {
                let (device, target) = (device_of(p)?, f64_of(p, "target_temp_c")?);
                self.temps_c.insert(device.clone(), target);
                Ok(json!({ "success": true, "device_id": device, "final_temp_c": target }))
            }
            "move_arm" => {
                let (x, y, z) = (f64_of(p, "x")?, f64_of(p, "y")?, f64_of(p, "z")?);
                Ok(json!({ "success": true, "position": { "x": x, "y": y, "z": z } }))
            }
            "incubate" => {
                let device = device_of(p).unwrap_or_else(|_| "incubator".into());
                let duration = f64_of(p, "duration_s").unwrap_or(0.0);
                if let Ok(t) = f64_of(p, "temp_c") {
                    self.temps_c.insert(device.clone(), t);
                }
                Ok(json!({ "success": true, "device_id": device, "duration_s": duration }))
            }
            "centrifuge" => {
                let rpm = f64_of(p, "rpm").unwrap_or(0.0);
                let duration = f64_of(p, "duration_s").unwrap_or(0.0);
                Ok(json!({ "success": true, "rpm": rpm, "duration_s": duration }))
            }
            other => Err(SilaError::UnknownTool(other.to_string())),
        }
    }

    /// Snapshot all vessel volumes for audit embedding.
    pub fn vessel_snapshot(&self) -> Value {
        self.vessels
            .iter()
            .map(|(id, p)| {
                (id.clone(), json!({ "volume_ul": p.volume_ul, "max_volume_ul": p.max_volume_ul }))
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    fn dispense(&mut self, vessel_id: &str, volume_ul: f64) -> Result<f64, SilaError> {
        let p = self.vessels.entry(vessel_id.to_owned()).or_insert_with(VesselParams::unknown);
        let new_vol = p.volume_ul + volume_ul;
        if new_vol > p.max_volume_ul {
            return Err(SilaError::Physics(format!(
                "overflow: {new_vol:.1} µL > {:.1} µL capacity",
                p.max_volume_ul
            )));
        }
        p.volume_ul = new_vol;
        Ok(new_vol)
    }

    fn aspirate(&mut self, vessel_id: &str, volume_ul: f64) -> Result<f64, SilaError> {
        let p = self.vessels.entry(vessel_id.to_owned()).or_insert_with(VesselParams::unknown);
        if volume_ul > p.volume_ul {
            return Err(SilaError::Physics(format!(
                "underflow: requested {volume_ul:.1} µL, only {:.1} µL available",
                p.volume_ul
            )));
        }
        p.volume_ul -= volume_ul;
        Ok(p.volume_ul)
    }

    fn read_absorbance(&self, vessel_id: &str, wavelength_nm: f64) -> f64 {
        let p = self.vessels.get(vessel_id).cloned().unwrap_or_else(VesselParams::unknown);
        let fill = if p.max_volume_ul > 0.0 { p.volume_ul / p.max_volume_ul } else { 0.0 };
        let a_base = p.epsilon * fill * p.path_length_cm;
        let wl_factor = (-0.5 * ((wavelength_nm - 500.0) / 150.0).powi(2)).exp();
        let a_det = a_base * wl_factor;
        let noise = rand::thread_rng().gen_range(0.98..=1.02_f64);
        f64::max(0.001, (a_det * noise * 10_000.0).round() / 10_000.0)
    }

    fn read_ph(&self, vessel_id: &str) -> f64 {
        // Neutral baseline with small deterministic-ish jitter; the real pH model
        // lives in the chemistry/lab-state layer. Empty vessels read ~7.0.
        let fill = self
            .vessels
            .get(vessel_id)
            .map(|p| if p.max_volume_ul > 0.0 { p.volume_ul / p.max_volume_ul } else { 0.0 })
            .unwrap_or(0.0);
        let ph = 7.0 - fill * 0.5;
        (ph * 100.0).round() / 100.0
    }
}

// ── Param extraction helpers ───────────────────────────────────────────────

fn vessel_of(p: &Value) -> Result<String, SilaError> {
    str_of(p, "vessel_id")
        .or_else(|_| str_of(p, "target_container"))
        .or_else(|_| str_of(p, "source_container"))
}

fn device_of(p: &Value) -> Result<String, SilaError> {
    str_of(p, "device_id")
        .or_else(|_| str_of(p, "target_plate"))
        .or_else(|_| str_of(p, "target_device"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_types::RiskClass;

    fn act(tool: &str, params: Value) -> Action {
        Action::new(tool, params, RiskClass::LiquidHandling)
    }

    #[test]
    fn dispense_then_aspirate() {
        let mut lab = SimLab::new();
        let r = lab.execute(&act("dispense", json!({"vessel_id": "tube_1", "volume_ul": 500.0}))).unwrap();
        assert_eq!(r["vessel_volume_ul"], 500.0);
        let r = lab.execute(&act("aspirate", json!({"vessel_id": "tube_1", "volume_ul": 200.0}))).unwrap();
        assert_eq!(r["vessel_volume_ul"], 300.0);
    }

    #[test]
    fn dispense_overflow_errors() {
        let mut lab = SimLab::new();
        let r = lab.execute(&act("dispense", json!({"vessel_id": "tube_1", "volume_ul": 9999.0})));
        assert!(matches!(r, Err(SilaError::Physics(_))));
    }

    #[test]
    fn absorbance_scales_with_fill() {
        let mut lab = SimLab::new();
        let empty = lab.read_absorbance("beaker_A", 500.0);
        lab.dispense("beaker_A", 25_000.0).unwrap();
        let half = lab.read_absorbance("beaker_A", 500.0);
        assert!(half > empty);
    }

    #[test]
    fn set_and_read_temperature() {
        let mut lab = SimLab::new();
        lab.execute(&act("set_temperature", json!({"device_id": "plate1", "target_temp_c": 37.0}))).unwrap();
        let r = lab.execute(&act("read_temperature", json!({"device_id": "plate1"}))).unwrap();
        assert_eq!(r["current_temp_c"], 37.0);
    }

    #[test]
    fn unknown_tool_errors() {
        let mut lab = SimLab::new();
        assert!(matches!(lab.execute(&act("frobnicate", json!({}))), Err(SilaError::UnknownTool(_))));
    }

    #[test]
    fn target_container_alias_works() {
        let mut lab = SimLab::new();
        let r = lab.execute(&act("read_absorbance", json!({"target_container": "tube_2", "wavelength_nm": 500.0}))).unwrap();
        assert_eq!(r["success"], true);
    }
}
