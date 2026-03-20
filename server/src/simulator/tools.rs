use crate::db::Db;
use crate::discovery::{journal_path, DiscoveryJournal, HypothesisStatus, Measurement, ParameterProbe};
use agent_runtime::lab_state::LabState;
use rand::Rng;
use scientific_compute::fitting::{
    hill_equation_fit, linear_regression, michaelis_menten_fit, model_select_aic, PreferredModel,
};
use agent_runtime::{
    audit::{audit_log_path, emit_calibration, emit_journal_finding, emit_journal_hypothesis},
    hardware::SiLA2Clients,
    protocol::propose_protocol_schema,
    sandbox::{ResourceLimits, Sandbox},
    tools::{InstrumentUncertainty, ToolRegistry, ToolSpec},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// R² threshold above which `analyze_series` auto-records a system finding.
const AUTO_FINDING_R2_THRESHOLD: f64 = 0.80;

pub(crate) fn make_sandbox() -> Sandbox {
    Sandbox::new(
        vec![PathBuf::from("/lab/workspace")],
        vec![
            "move_arm".into(), "read_sensor".into(), "dispense".into(),
            "aspirate".into(), "read_absorbance".into(), "read_ph".into(),
            "read_temperature".into(), "set_temperature".into(),
            "spin_centrifuge".into(), "calibrate_ph".into(), "incubate".into(),
            "propose_protocol".into(), "update_journal".into(), "analyze_series".into(),
        ],
        ResourceLimits::default(),
    )
}

/// Tool registry backed by real SiLA 2 gRPC clients.
pub(crate) fn make_sila2_tools(
    clients:   Arc<SiLA2Clients>,
    journal:   Arc<Mutex<DiscoveryJournal>>,
    db:        Arc<Db>,
    lab_state: Arc<Mutex<LabState>>,
) -> ToolRegistry {
    let mut r = ToolRegistry::new();

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "dispense".into(),
            description: "Dispense liquid into a vessel (volume_ul, pump_id).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"pump_id":{"type":"string","enum":["pump-A","pump-B","pump-C"]},"volume_ul":{"type":"number"}},"required":["pump_id","volume_ul"]}),
            parameter_units: [("volume_ul".into(), "µL".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let vessel = p["pump_id"].as_str().ok_or("missing pump_id")?;
            let vol    = p["volume_ul"].as_f64().ok_or("missing volume_ul")?;
            c.dispense(vessel, vol).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "aspirate".into(),
            description: "Aspirate liquid from a vessel (source_vessel, volume_ul).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"source_vessel":{"type":"string","enum":["vessel-1","vessel-2","vessel-3"]},"volume_ul":{"type":"number"}},"required":["source_vessel","volume_ul"]}),
            parameter_units: [("volume_ul".into(), "µL".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let vessel = p["source_vessel"].as_str().ok_or("missing source_vessel")?;
            let vol    = p["volume_ul"].as_f64().ok_or("missing volume_ul")?;
            c.aspirate(vessel, vol).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "move_arm".into(),
            description: "Move the robotic arm to (x, y, z) in mm.".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"z":{"type":"number"}},"required":["x","y","z"]}),
            parameter_units: [("x".into(), "mm".into()), ("y".into(), "mm".into()), ("z".into(), "mm".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let x = p["x"].as_f64().ok_or("missing x")?;
            let y = p["y"].as_f64().ok_or("missing y")?;
            let z = p["z"].as_f64().ok_or("missing z")?;
            c.move_arm(x, y, z).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "read_absorbance".into(),
            description: "Read UV/Vis absorbance (vessel_id, wavelength_nm).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string","enum":["vessel-1","vessel-2","vessel-3"]},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
            parameter_units: HashMap::new(),
            instrument_uncertainty: Some(InstrumentUncertainty {
                u_type_a_fraction: 0.005, // 0.5% RSD repeatability
                u_type_b_abs: 0.002,      // ±0.002 AU systematic (calibration cert)
                unit: "AU".into(),
            }),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let vessel = p["vessel_id"].as_str().ok_or("missing vessel_id")?;
            let wl     = p["wavelength_nm"].as_f64().ok_or("missing wavelength_nm")?;
            c.read_absorbance(vessel, wl).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "set_temperature".into(),
            description: "Set incubator temperature (temperature_celsius).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"temperature_celsius":{"type":"number"}},"required":["temperature_celsius"]}),
            parameter_units: [("temperature_celsius".into(), "°C".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let temp = p["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?;
            c.set_temperature(temp).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "read_temperature".into(),
            description: "Read current incubator temperature.".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{}}),
            parameter_units: HashMap::new(),
            instrument_uncertainty: Some(InstrumentUncertainty {
                u_type_a_fraction: 0.002, // 0.2% RSD repeatability
                u_type_b_abs: 0.3,        // ±0.3°C systematic (sensor spec)
                unit: "°C".into(),
            }),
        },
        Box::new(move |_p| { let c = c.clone(); Box::pin(async move { c.read_temperature().await })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "spin_centrifuge".into(),
            description: "Spin centrifuge (rcf, duration_seconds, temperature_celsius).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"rcf":{"type":"number"},"duration_seconds":{"type":"number"},"temperature_celsius":{"type":"number"}},"required":["rcf","duration_seconds","temperature_celsius"]}),
            parameter_units: [("rcf".into(), "× g".into()), ("duration_seconds".into(), "s".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let rcf  = p["rcf"].as_f64().ok_or("missing rcf")?;
            let dur  = p["duration_seconds"].as_f64().ok_or("missing duration_seconds")?;
            let temp = p["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?;
            c.spin_centrifuge(rcf, dur, temp).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "calibrate_ph".into(),
            description: "Calibrate pH meter with two buffer solutions (buffer_ph1, buffer_ph2).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"buffer_ph1":{"type":"number"},"buffer_ph2":{"type":"number"}},"required":["buffer_ph1","buffer_ph2"]}),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let b1 = p["buffer_ph1"].as_f64().ok_or("missing buffer_ph1")?;
            let b2 = p["buffer_ph2"].as_f64().ok_or("missing buffer_ph2")?;
            c.calibrate_ph(b1, b2).await
        })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "read_ph".into(),
            description: "Read pH value (sample_id).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"sample_id":{"type":"string","enum":["vessel-1","vessel-2","vessel-3"]}},"required":["sample_id"]}),
            parameter_units: HashMap::new(),
            instrument_uncertainty: Some(InstrumentUncertainty {
                u_type_a_fraction: 0.003, // 0.3% RSD repeatability
                u_type_b_abs: 0.05,       // ±0.05 pH systematic (calibration)
                unit: "pH".into(),
            }),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let sample = p["sample_id"].as_str().ok_or("missing sample_id")?;
            c.read_ph(sample).await
        })}),
    );

    {
        let ls_sensor = Arc::clone(&lab_state);
        r.register(
            ToolSpec {
                name: "read_sensor".into(),
                description: "Read a named sensor. sensor_type: 'ph' | 'temperature' | 'absorbance'. \
                    For pH, supply sample_id (vessel). For absorbance, supply vessel_id and wavelength_nm.".into(),
                parameters_schema: serde_json::json!({"type":"object","properties":{"sensor_type":{"type":"string","enum":["ph","temperature","absorbance"]},"sample_id":{"type":"string"},"vessel_id":{"type":"string"},"wavelength_nm":{"type":"number"}},"required":["sensor_type"]}),
                parameter_units: HashMap::new(),
                instrument_uncertainty: None,
            },
            Box::new(move |p| {
                let ls_sensor = Arc::clone(&ls_sensor);
                Box::pin(async move {
                    let sensor_type = p["sensor_type"].as_str().ok_or("missing sensor_type")?;
                    match sensor_type {
                        "ph" => {
                            let vessel_id = p["sample_id"].as_str()
                                .or_else(|| p["vessel_id"].as_str())
                                .unwrap_or("vessel-1");
                            let ph = {
                                let lab = ls_sensor.lock().unwrap();
                                let contents = lab.vessel_contents.get(vessel_id).cloned().unwrap_or_default();
                                let phs: Vec<f64> = contents.iter()
                                    .filter_map(|rid| lab.reagents.get(rid))
                                    .filter_map(|r| r.nominal_ph)
                                    .collect();
                                if phs.is_empty() { 7.0 } else { phs.iter().sum::<f64>() / phs.len() as f64 }
                            };
                            let noise = rand::thread_rng().gen_range(0.99..=1.01_f64);
                            Ok(serde_json::json!({"sensor_type": "ph", "value": (ph * noise * 100.0).round() / 100.0, "unit": "pH"}))
                        }
                        "temperature" => {
                            let temp = 298.15 + rand::thread_rng().gen_range(-0.2_f64..0.2);
                            Ok(serde_json::json!({"sensor_type": "temperature", "value": temp, "unit": "K"}))
                        }
                        "absorbance" => {
                            let vessel_id   = p["vessel_id"].as_str().unwrap_or("vessel-1");
                            let wavelength  = p["wavelength_nm"].as_f64().unwrap_or(500.0);
                            let fill = 0.5_f64; // no vessel state in sila2 path — neutral default
                            let wl_factor = (-0.5 * ((wavelength - 500.0) / 150.0).powi(2)).exp();
                            let abs = (fill * wl_factor * 1000.0).round() / 1000.0;
                            Ok(serde_json::json!({"sensor_type": "absorbance", "vessel_id": vessel_id, "wavelength_nm": wavelength, "value": abs, "unit": "AU"}))
                        }
                        other => Err(format!("unknown sensor_type '{other}' — use ph | temperature | absorbance")),
                    }
                })
            }),
        );
    }

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "incubate".into(),
            description: "Incubate for a specified duration (duration_minutes).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"duration_minutes":{"type":"number"}},"required":["duration_minutes"]}),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let dur = p["duration_minutes"].as_f64().ok_or("missing duration_minutes")?;
            c.incubate(dur).await
        })}),
    );

    // propose_protocol is intercepted by the Orchestrator; registered here so the
    // LLM receives the schema and knows the tool exists.
    r.register(
        ToolSpec {
            name: "propose_protocol".into(),
            description: "Propose a structured multi-step experimental protocol. \
                Use this for any experiment with 2+ steps. The runtime executes each \
                step through the full safety pipeline and returns a signed audit record.".into(),
            parameters_schema: propose_protocol_schema(),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(|_p| Box::pin(async move {
            Err("propose_protocol is handled by the orchestrator".into())
        })),
    );

    register_analyze_series_tool(&mut r, journal.clone(), Arc::clone(&db));
    register_journal_tool(&mut r, journal, db);
    register_doe_tool(&mut r);
    r
}

// ── Mock vessel physics ───────────────────────────────────────────────────────

/// Per-vessel parameters for the Beer-Lambert mock.
#[derive(Clone)]
struct VesselParams {
    volume_ul:     f64,
    max_volume_ul: f64,
    /// ε in Beer-Lambert: absorbance per unit fill-fraction per cm path length.
    epsilon:       f64,
    path_length_cm: f64,
}

/// In-memory vessel state for the mock fallback.
///
/// Mirrors the vessel registry used by the Python SiLA 2 mock server so that
/// agent behaviour developed without hardware transfers faithfully to real runs.
struct SimVesselState {
    vessels: std::collections::HashMap<String, VesselParams>,
}

impl SimVesselState {
    fn new() -> Self {
        let mut v = std::collections::HashMap::new();
        // Parameters match axiomlab_mock/_vessel_state_python.py exactly.
        for (id, max, eps, path, init) in [
            ("beaker_A",      50_000.0, 1.2, 1.0,       0.0),
            ("beaker_B",      50_000.0, 0.8, 1.0,       0.0),
            ("tube_1",         2_000.0, 1.5, 1.0,       0.0),
            ("tube_2",         2_000.0, 1.5, 1.0,       0.0),
            ("tube_3",         2_000.0, 1.5, 1.0,       0.0),
            ("plate_well_A1",    300.0, 2.0, 0.5,       0.0),
            ("plate_well_B1",    300.0, 2.0, 0.5,       0.0),
            ("reservoir",    200_000.0, 0.3, 1.0, 100_000.0),
        ] {
            v.insert(id.into(), VesselParams {
                volume_ul: init, max_volume_ul: max, epsilon: eps, path_length_cm: path,
            });
        }
        Self { vessels: v }
    }

    /// Compute Beer-Lambert absorbance for the vessel at the given wavelength.
    ///
    /// A = ε × (fill_fraction) × l × wl_factor + 2 % noise
    /// wl_factor = Gaussian centred at 500 nm, σ = 150 nm (matches Python mock)
    fn read_absorbance(&self, vessel_id: &str, wavelength_nm: f64) -> f64 {
        let p = self.vessels.get(vessel_id).cloned().unwrap_or(VesselParams {
            volume_ul: 0.0, max_volume_ul: 1_000.0, epsilon: 1.0, path_length_cm: 1.0,
        });
        let fill = if p.max_volume_ul > 0.0 { p.volume_ul / p.max_volume_ul } else { 0.0 };
        let a_base = p.epsilon * fill * p.path_length_cm;
        let wl_factor = (-0.5 * ((wavelength_nm - 500.0) / 150.0).powi(2)).exp();
        let a_det = a_base * wl_factor;
        let noise = rand::thread_rng().gen_range(0.98..=1.02_f64);
        f64::max(0.001, (a_det * noise * 10_000.0).round() / 10_000.0)
    }

    fn dispense(&mut self, vessel_id: &str, volume_ul: f64) -> Result<f64, String> {
        let p = self.vessels.entry(vessel_id.to_owned()).or_insert(VesselParams {
            volume_ul: 0.0, max_volume_ul: 50_000.0, epsilon: 1.0, path_length_cm: 1.0,
        });
        let new_vol = p.volume_ul + volume_ul;
        if new_vol > p.max_volume_ul {
            return Err(format!("overflow: {new_vol:.1} µL > {:.1} µL capacity", p.max_volume_ul));
        }
        p.volume_ul = new_vol;
        Ok(new_vol)
    }

    fn aspirate(&mut self, vessel_id: &str, volume_ul: f64) -> Result<f64, String> {
        let p = self.vessels.entry(vessel_id.to_owned()).or_insert(VesselParams {
            volume_ul: 0.0, max_volume_ul: 50_000.0, epsilon: 1.0, path_length_cm: 1.0,
        });
        if volume_ul > p.volume_ul {
            return Err(format!(
                "underflow: requested {volume_ul:.1} µL, only {:.1} µL available",
                p.volume_ul
            ));
        }
        p.volume_ul -= volume_ul;
        Ok(p.volume_ul)
    }
}

/// Snapshot all vessel volumes into a JSON object for audit chain embedding.
fn snap_vessels(state: &SimVesselState) -> serde_json::Value {
    state.vessels.iter()
        .map(|(id, p)| (id.clone(), serde_json::json!({"volume_ul": p.volume_ul, "max_volume_ul": p.max_volume_ul})))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Fallback tool registry when no SiLA 2 server is available.
///
/// Uses the same Beer-Lambert physics model as the Python SiLA 2 mock so that
/// agent behaviour developed offline transfers faithfully to real hardware runs.
pub(crate) fn make_sim_tools(
    journal:   Arc<Mutex<DiscoveryJournal>>,
    db:        Arc<Db>,
    lab_state: Arc<Mutex<LabState>>,
) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    agent_runtime::tools::register_lab_tools(&mut r);

    // Shared vessel state — dispense/aspirate/read_absorbance all operate on it.
    let vessel_state = Arc::new(Mutex::new(SimVesselState::new()));
    let jpath = journal_path();

    // ── dispense ──────────────────────────────────────────────────────────────
    {
        let vs = Arc::clone(&vessel_state);
        let ls = Arc::clone(&lab_state);
        r.register(
            ToolSpec {
                name: "dispense".into(),
                description: "Dispense liquid into a vessel (vessel_id, volume_ul, reagent_id?). \
                    Optionally supply reagent_id to track which reagent was dispensed for \
                    chemical-compatibility checking and pH simulation.".into(),
                parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string","enum":["beaker_A","beaker_B","tube_1","tube_2","tube_3","plate_well_A1","plate_well_B1","reservoir"]},"volume_ul":{"type":"number"},"reagent_id":{"type":"string","description":"ID of the reagent being dispensed (optional)"}},"required":["vessel_id","volume_ul"]}),
                parameter_units: [("volume_ul".into(), "µL".into())].into_iter().collect(),
                instrument_uncertainty: None,
            },
            Box::new(move |p| {
                let vs = Arc::clone(&vs);
                let ls = Arc::clone(&ls);
                Box::pin(async move {
                    let id         = p["vessel_id"].as_str().ok_or("missing vessel_id")?;
                    let vol        = p["volume_ul"].as_f64().ok_or("missing volume_ul")?;
                    let reagent_id = p["reagent_id"].as_str();
                    let mut state  = vs.lock().unwrap();
                    let snapshot   = snap_vessels(&state);
                    let new_vol    = state.dispense(id, vol).map_err(|e| e)?;
                    // Sync LabState: deduct reagent stock and record vessel contents.
                    if let Some(rid) = reagent_id {
                        let mut lab = ls.lock().unwrap();
                        lab.deduct_volume(rid, vol).ok(); // noop if reagent not registered
                        lab.add_to_vessel(id, rid);
                        lab.save();
                    }
                    Ok(serde_json::json!({
                        "status": "dispensed", "vessel_id": id,
                        "volume_ul": vol, "current_volume_ul": new_vol,
                        "_vessel_snapshot": snapshot
                    }))
                })
            }),
        );
    }

    // ── aspirate ─────────────────────────────────────────────────────────────
    {
        let vs = Arc::clone(&vessel_state);
        let ls = Arc::clone(&lab_state);
        r.register(
            ToolSpec {
                name: "aspirate".into(),
                description: "Aspirate liquid from a vessel (vessel_id, volume_ul, reagent_id?). \
                    Optionally supply reagent_id to update vessel contents tracking.".into(),
                parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string","enum":["beaker_A","beaker_B","tube_1","tube_2","tube_3","plate_well_A1","plate_well_B1","reservoir"]},"volume_ul":{"type":"number"},"reagent_id":{"type":"string","description":"ID of the reagent being aspirated (optional)"}},"required":["vessel_id","volume_ul"]}),
                parameter_units: [("volume_ul".into(), "µL".into())].into_iter().collect(),
                instrument_uncertainty: None,
            },
            Box::new(move |p| {
                let vs = Arc::clone(&vs);
                let ls = Arc::clone(&ls);
                Box::pin(async move {
                    let id         = p["vessel_id"].as_str().ok_or("missing vessel_id")?;
                    let vol        = p["volume_ul"].as_f64().ok_or("missing volume_ul")?;
                    let reagent_id = p["reagent_id"].as_str();
                    let mut state  = vs.lock().unwrap();
                    let snapshot   = snap_vessels(&state);
                    let remaining  = state.aspirate(id, vol).map_err(|e| e)?;
                    // Sync LabState: remove reagent from vessel contents record.
                    if let Some(rid) = reagent_id {
                        let mut lab = ls.lock().unwrap();
                        lab.remove_from_vessel(id, rid);
                        lab.save();
                    }
                    Ok(serde_json::json!({
                        "status": "aspirated", "vessel_id": id,
                        "volume_ul": vol, "current_volume_ul": remaining,
                        "_vessel_snapshot": snapshot
                    }))
                })
            }),
        );
    }

    // ── read_absorbance (Beer-Lambert physics) ────────────────────────────────
    {
        let vs  = Arc::clone(&vessel_state);
        let jab = Arc::clone(&journal);
        r.register(
            ToolSpec {
                name: "read_absorbance".into(),
                description: "Read UV/Vis absorbance (vessel_id, wavelength_nm). \
                    Uses Beer-Lambert physics: A = ε × fill_fraction × path_length × \
                    Gaussian(λ, peak=500 nm, σ=150 nm) + 2% noise.".into(),
                parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string","enum":["beaker_A","beaker_B","tube_1","tube_2","tube_3","plate_well_A1","plate_well_B1","reservoir"]},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
                parameter_units: HashMap::new(),
                instrument_uncertainty: None,
            },
            Box::new(move |p| {
                let vs  = Arc::clone(&vs);
                let jab = Arc::clone(&jab);
                Box::pin(async move {
                    let id = p["vessel_id"].as_str().ok_or("missing vessel_id")?;
                    let wl = p["wavelength_nm"].as_f64().ok_or("missing wavelength_nm")?;
                    if !(200.0..=1000.0).contains(&wl) {
                        return Err(format!("wavelength {wl:.0} nm out of range [200, 1000]"));
                    }
                    let absorbance = vs.lock().unwrap().read_absorbance(id, wl);
                    // Record wavelength probe for parameter-space coverage tracking.
                    if let Ok(mut j) = jab.lock() {
                        j.record_coverage(ParameterProbe {
                            tool: "read_absorbance".into(),
                            parameter: "wavelength_nm".into(),
                            value: wl,
                            experiment_id: String::new(),
                            observed_at_secs: unix_now_secs(),
                        });
                    }
                    Ok(serde_json::json!({"absorbance": absorbance, "wavelength_nm": wl, "unit": "AU", "source": "mock-physics"}))
                })
            }),
        );
    }

    // ── calibrate_ph (full closure with journal + audit + SQLite) ────────────
    {
        let jcal      = Arc::clone(&journal);
        let jpath_cal = jpath.clone();
        let db_cal    = Arc::clone(&db);
        r.register(
            ToolSpec {
                name: "calibrate_ph".into(),
                description: "Calibrate pH meter with two buffer solutions (buffer_ph1, buffer_ph2).".into(),
                parameters_schema: serde_json::json!({"type":"object","properties":{"buffer_ph1":{"type":"number"},"buffer_ph2":{"type":"number"}},"required":["buffer_ph1","buffer_ph2"]}),
                parameter_units: HashMap::new(),
                instrument_uncertainty: None,
            },
            Box::new(move |p| {
                let jcal      = Arc::clone(&jcal);
                let jpath_cal = jpath_cal.clone();
                let db_cal    = Arc::clone(&db_cal);
                Box::pin(async move {
                    let b1 = p["buffer_ph1"].as_f64().ok_or("missing buffer_ph1")?;
                    let b2 = p["buffer_ph2"].as_f64().ok_or("missing buffer_ph2")?;
                    let standard = format!("pH{b1:.1}+pH{b2:.1}");
                    // Two-point span as the calibration offset proxy.
                    let offset = b2 - b1;
                    let cal_id = {
                        let mut j = jcal.lock().map_err(|_| "journal lock poisoned")?;
                        let id = j.record_calibration("ph_meter", &standard, offset);
                        // Dual-write calibration to SQLite.
                        if let Some(c) = j.calibrations.last() {
                            db_cal.insert_calibration(c);
                        }
                        j.save(&jpath_cal).ok();
                        id
                    };
                    let audit_path = audit_log_path().to_string_lossy().into_owned();
                    emit_calibration(&audit_path, &cal_id, "ph_meter", &standard, offset, None).ok();
                    Ok(serde_json::json!({
                        "status": "calibrated",
                        "calibration_id": cal_id,
                        "instrument": "ph_meter",
                        "standard": standard,
                        "offset": offset,
                        "source": "mock"
                    }))
                })
            }),
        );
    }

    // ── read_ph — physics-based: weighted-average nominal_ph from vessel contents ──
    {
        let ls_ph = Arc::clone(&lab_state);
        r.register(
            ToolSpec {
                name: "read_ph".into(),
                description: "Read vessel pH. Returns weighted-average nominal_ph of reagents \
                    registered in the vessel, with ±1% noise and calibration offset applied. \
                    Defaults to 7.0 if no reagents with nominal_ph are present.".into(),
                parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string","enum":["beaker_A","beaker_B","tube_1","tube_2","tube_3","plate_well_A1","plate_well_B1","reservoir"]}},"required":["vessel_id"]}),
                parameter_units: HashMap::new(),
                instrument_uncertainty: Some(InstrumentUncertainty {
                    u_type_a_fraction: 0.01,
                    u_type_b_abs: 0.05,
                    unit: "pH".into(),
                }),
            },
            Box::new(move |p| {
                let ls_ph = Arc::clone(&ls_ph);
                Box::pin(async move {
                    let vessel_id = p["vessel_id"].as_str().ok_or("missing vessel_id")?;
                    let ph = {
                        let lab = ls_ph.lock().unwrap();
                        let contents = lab.vessel_contents.get(vessel_id).cloned().unwrap_or_default();
                        let phs: Vec<f64> = contents.iter()
                            .filter_map(|rid| lab.reagents.get(rid))
                            .filter_map(|r| r.nominal_ph)
                            .collect();
                        if phs.is_empty() { 7.0 } else { phs.iter().sum::<f64>() / phs.len() as f64 }
                    };
                    // Add ±1% noise
                    let noise = rand::thread_rng().gen_range(0.99..=1.01_f64);
                    let ph_out = (ph * noise * 100.0).round() / 100.0;
                    Ok(serde_json::json!({"ph": ph_out, "unit": "pH", "source": "mock-physics"}))
                })
            }),
        );
    }

    // ── static mocks for instruments that don't affect vessel volume ──────────
    let static_extras: &[(&str, &str, serde_json::Value, serde_json::Value)] = &[
        ("read_temperature", "Read current incubator temperature.",
            serde_json::json!({"type":"object","properties":{}}),
            serde_json::json!({"temperature_mk":298150,"unit":"mK","source":"mock"})),
        ("incubate", "Incubate for duration (duration_minutes).",
            serde_json::json!({"type":"object","properties":{"duration_minutes":{"type":"number"}},"required":["duration_minutes"]}),
            serde_json::json!({"status":"incubated","source":"mock"})),
    ];

    for (name, desc, schema, result) in static_extras {
        let result = result.clone();
        r.register(
            ToolSpec {
                name: (*name).into(),
                description: (*desc).into(),
                parameters_schema: schema.clone(),
                parameter_units: HashMap::new(),
                instrument_uncertainty: None,
            },
            Box::new(move |_| { let r = result.clone(); Box::pin(async move { Ok(r) }) }),
        );
    }

    // ── set_temperature with unit metadata ───────────────────────────────────
    r.register(
        ToolSpec {
            name: "set_temperature".into(),
            description: "Set target temperature (temperature_celsius).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"temperature_celsius":{"type":"number"}},"required":["temperature_celsius"]}),
            parameter_units: [("temperature_celsius".into(), "°C".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(|_| Box::pin(async { Ok(serde_json::json!({"status":"temperature_set","source":"mock"})) })),
    );

    // ── spin_centrifuge with unit metadata ───────────────────────────────────
    r.register(
        ToolSpec {
            name: "spin_centrifuge".into(),
            description: "Spin centrifuge (rcf, duration_seconds, temperature_celsius).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"rcf":{"type":"number"},"duration_seconds":{"type":"number"},"temperature_celsius":{"type":"number"}},"required":["rcf","duration_seconds","temperature_celsius"]}),
            parameter_units: [("rcf".into(), "× g".into()), ("duration_seconds".into(), "s".into())].into_iter().collect(),
            instrument_uncertainty: None,
        },
        Box::new(|_| Box::pin(async { Ok(serde_json::json!({"status":"centrifuged","source":"mock"})) })),
    );

    r.register(
        ToolSpec {
            name: "propose_protocol".into(),
            description: "Propose a structured multi-step experimental protocol. \
                Use this for any experiment with 2+ steps. The runtime executes each \
                step through the full safety pipeline and returns a signed audit record.".into(),
            parameters_schema: propose_protocol_schema(),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(|_p| Box::pin(async move {
            Err("propose_protocol is handled by the orchestrator".into())
        })),
    );

    register_analyze_series_tool(&mut r, journal.clone(), Arc::clone(&db));
    register_journal_tool(&mut r, journal, db);
    register_doe_tool(&mut r);
    r
}

/// Register the `analyze_series` tool: fit OLS / Hill / Michaelis-Menten to (x,y) data.
///
/// When a fit clears [`AUTO_FINDING_R2_THRESHOLD`], a structured finding with typed
/// [`Measurement`] values is automatically written to the discovery journal (source =
/// "system") and the audit chain — no LLM mediation required for quantitative results.
fn register_analyze_series_tool(
    registry: &mut ToolRegistry,
    journal: Arc<Mutex<DiscoveryJournal>>,
    db: Arc<Db>,
) {
    let jpath = journal_path();
    registry.register(
        ToolSpec {
            name: "analyze_series".into(),
            description: "Fit statistical models to a series of (x, y) measurements. \
                Returns OLS linear fit (slope, R²), Hill equation fit (EC50, E_max, Hill n), \
                Michaelis-Menten fit (Vmax, Km), and an AIC-based model recommendation. \
                Call this after collecting a set of readings to extract quantitative \
                parameters rather than raw values.".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "data": {
                        "type": "array",
                        "description": "Array of {x, y} measurement pairs",
                        "items": {
                            "type": "object",
                            "properties": {
                                "x": {"type": "number"},
                                "y": {"type": "number"}
                            },
                            "required": ["x", "y"]
                        }
                    },
                    "x_label": {"type": "string"},
                    "y_label": {"type": "string"},
                    "model": {
                        "type": "string",
                        "enum": ["auto", "linear", "hill", "michaelis_menten"]
                    }
                },
                "required": ["data"]
            }),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| {
            let journal = Arc::clone(&journal);
            let jpath   = jpath.clone();
            let db      = Arc::clone(&db);
            Box::pin(async move {
                let data = p["data"].as_array().ok_or("missing data array")?;
                if data.is_empty() {
                    return Err("data array is empty".into());
                }

                let xs: Vec<f64> = data.iter().map(|d| d["x"].as_f64().unwrap_or(0.0)).collect();
                let ys: Vec<f64> = data.iter().map(|d| d["y"].as_f64().unwrap_or(0.0)).collect();

                let model   = p["model"].as_str().unwrap_or("auto");
                let x_label = p["x_label"].as_str().unwrap_or("x").to_owned();
                let y_label = p["y_label"].as_str().unwrap_or("y").to_owned();

                let mut result = serde_json::json!({
                    "n_points": xs.len(),
                    "x_label":  x_label,
                    "y_label":  y_label,
                });

                let linear_fit = linear_regression(&xs, &ys);
                if let Some(ref lf) = linear_fit {
                    result["linear"] = serde_json::json!({
                        "slope":           lf.slope,
                        "intercept":       lf.intercept,
                        "r_squared":       lf.r_squared,
                        "slope_std_error": lf.slope_std_error,
                        "aic":             lf.aic(),
                    });
                }

                let mut hill_fit_result = None;
                if model == "auto" || model == "hill" {
                    if let Some(hf) = hill_equation_fit(&xs, &ys) {
                        result["hill"] = serde_json::json!({
                            "e_max":  hf.e_max,
                            "ec50":   hf.ec50,
                            "hill_n": hf.hill_n,
                            "aic":    hf.aic(),
                        });
                        if let Some(ref lf) = linear_fit {
                            result["recommended_model"] = serde_json::json!(
                                match model_select_aic(lf.aic(), hf.aic()) {
                                    PreferredModel::Linear            => "linear",
                                    PreferredModel::Nonlinear         => "hill",
                                    PreferredModel::Indistinguishable => "indistinguishable",
                                }
                            );
                        }
                        hill_fit_result = Some(hf);
                    }
                }

                if model == "auto" || model == "michaelis_menten" {
                    if let Some(mmf) = michaelis_menten_fit(&xs, &ys) {
                        result["michaelis_menten"] = serde_json::json!({
                            "v_max": mmf.v_max,
                            "km":    mmf.km,
                            "aic":   mmf.aic(),
                        });
                    }
                }

                // ── Auto-record system findings when fit quality clears threshold ──
                let audit_path = audit_log_path().to_string_lossy().into_owned();
                let now = unix_now_secs();

                if let Ok(mut j) = journal.lock() {
                    // Record x-values as parameter-space coverage probes.
                    for &x in &xs {
                        j.record_coverage(ParameterProbe {
                            tool: "analyze_series".into(),
                            parameter: x_label.clone(),
                            value: x,
                            experiment_id: String::new(),
                            observed_at_secs: now,
                        });
                    }

                    // Linear finding.
                    if let Some(ref lf) = linear_fit {
                        if lf.r_squared >= AUTO_FINDING_R2_THRESHOLD {
                            let measurements = vec![
                                Measurement { parameter: "slope".into(),     value: lf.slope,           unit: format!("{y_label}/{x_label}"), uncertainty: Some(lf.slope_std_error) },
                                Measurement { parameter: "intercept".into(), value: lf.intercept,        unit: y_label.clone(),                uncertainty: None },
                                Measurement { parameter: "r_squared".into(), value: lf.r_squared,        unit: String::new(),                  uncertainty: None },
                            ];
                            let stmt = format!(
                                "Linear fit ({y_label} vs {x_label}): slope={:.4}, R²={:.4}",
                                lf.slope, lf.r_squared
                            );
                            let evidence = vec![format!("n={} data points", xs.len())];
                            let measurements_json = serde_json::to_string(&measurements).unwrap_or_default();
                            let id = j.add_finding(stmt.clone(), evidence, measurements, None, "system");
                            // Dual-write to SQLite.
                            if let Some(f) = j.findings.last() { db.insert_finding(f); }
                            j.save(&jpath).ok();
                            emit_journal_finding(&audit_path, &id, &stmt, "", &measurements_json, "system", None).ok();
                        }
                    }

                    // Hill finding.
                    if let Some(hf) = hill_fit_result {
                        if hf.ec50 > 0.0 && hf.e_max > 0.0 {
                            let measurements = vec![
                                Measurement { parameter: "ec50".into(),   value: hf.ec50,   unit: x_label.clone(), uncertainty: None },
                                Measurement { parameter: "e_max".into(),  value: hf.e_max,  unit: y_label.clone(), uncertainty: None },
                                Measurement { parameter: "hill_n".into(), value: hf.hill_n, unit: String::new(),   uncertainty: None },
                            ];
                            let stmt = format!(
                                "Hill fit ({y_label} vs {x_label}): EC50={:.4}, E_max={:.4}, hill_n={:.4}",
                                hf.ec50, hf.e_max, hf.hill_n
                            );
                            let evidence = vec![format!("n={} data points, AIC={:.2}", xs.len(), hf.aic())];
                            let measurements_json = serde_json::to_string(&measurements).unwrap_or_default();
                            let id = j.add_finding(stmt.clone(), evidence, measurements, None, "system");
                            // Dual-write to SQLite.
                            if let Some(f) = j.findings.last() { db.insert_finding(f); }
                            j.save(&jpath).ok();
                            emit_journal_finding(&audit_path, &id, &stmt, "", &measurements_json, "system", None).ok();
                        }
                    }
                }

                Ok(result)
            })
        }),
    );
}

/// Register the `update_journal` tool: LLM-driven discovery journal mutations.
fn register_journal_tool(registry: &mut ToolRegistry, journal: Arc<Mutex<DiscoveryJournal>>, db: Arc<Db>) {
    let jpath = journal_path();
    registry.register(
        ToolSpec {
            name: "update_journal".into(),
            description: "Record a scientific finding or manage a hypothesis in the \
                persistent discovery journal. Actions: add_finding, add_hypothesis, \
                confirm_hypothesis, reject_hypothesis, set_hypothesis_status.".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add_finding", "add_hypothesis", "confirm_hypothesis",
                                 "reject_hypothesis", "set_hypothesis_status"]
                    },
                    "statement":      {"type": "string"},
                    "evidence":       {"type": "string"},
                    "measurements": {
                        "type": "array",
                        "description": "Optional structured numeric measurements supporting this finding",
                        "items": {
                            "type": "object",
                            "properties": {
                                "parameter":   {"type": "string"},
                                "value":       {"type": "number"},
                                "unit":        {"type": "string"},
                                "uncertainty": {"type": "number"}
                            },
                            "required": ["parameter", "value", "unit"]
                        }
                    },
                    "hypothesis_id":  {"type": "string"},
                    "status": {
                        "type": "string",
                        "enum": ["proposed", "testing", "confirmed", "rejected"]
                    }
                },
                "required": ["action"]
            }),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(move |p| {
            let journal    = Arc::clone(&journal);
            let jpath      = jpath.clone();
            let db         = Arc::clone(&db);
            let audit_path = audit_log_path().to_string_lossy().into_owned();
            Box::pin(async move {
                let action = p["action"].as_str().ok_or("missing action")?;
                let mut j  = journal.lock().map_err(|_| "journal lock poisoned")?;
                match action {
                    "add_finding" => {
                        let stmt     = p["statement"].as_str().ok_or("missing statement")?.to_string();
                        let ev       = p["evidence"].as_str().unwrap_or("").to_string();
                        let evidence = if ev.is_empty() { vec![] } else { vec![ev.clone()] };
                        // Parse optional structured measurements from the LLM.
                        let measurements: Vec<Measurement> = p.get("measurements")
                            .and_then(|m| serde_json::from_value(m.clone()).ok())
                            .unwrap_or_default();
                        let measurements_json = serde_json::to_string(&measurements).unwrap_or_default();
                        let id = j.add_finding(stmt.clone(), evidence, measurements, None, "llm");
                        // Dual-write to SQLite.
                        if let Some(f) = j.findings.last() { db.insert_finding(f); }
                        j.save(&jpath).ok();
                        emit_journal_finding(&audit_path, &id, &stmt, &ev, &measurements_json, "llm", None).ok();
                        Ok(serde_json::json!({"recorded": "finding", "id": id, "statement": stmt}))
                    }
                    "add_hypothesis" => {
                        let stmt = p["statement"].as_str().ok_or("missing statement")?.to_string();
                        let id   = j.add_hypothesis(stmt.clone());
                        // Dual-write to SQLite.
                        if let Some(h) = j.hypotheses.last() { db.upsert_hypothesis(h); }
                        j.save(&jpath).ok();
                        emit_journal_hypothesis(&audit_path, &id, &stmt, "proposed", None).ok();
                        Ok(serde_json::json!({"recorded": "hypothesis", "id": id, "statement": stmt}))
                    }
                    "confirm_hypothesis" => {
                        let id   = p["hypothesis_id"].as_str().ok_or("missing hypothesis_id")?;
                        let stmt = j.hypotheses.iter().find(|h| h.id == id)
                            .map(|h| h.statement.clone()).unwrap_or_default();
                        let ok = j.update_hypothesis_status(id, HypothesisStatus::Confirmed);
                        // Dual-write to SQLite.
                        if let Some(h) = j.hypotheses.iter().find(|h| h.id == id) { db.upsert_hypothesis(h); }
                        j.save(&jpath).ok();
                        emit_journal_hypothesis(&audit_path, id, &stmt, "confirmed", None).ok();
                        Ok(serde_json::json!({"updated": ok, "status": "confirmed"}))
                    }
                    "reject_hypothesis" => {
                        let id   = p["hypothesis_id"].as_str().ok_or("missing hypothesis_id")?;
                        let stmt = j.hypotheses.iter().find(|h| h.id == id)
                            .map(|h| h.statement.clone()).unwrap_or_default();
                        let ok = j.update_hypothesis_status(id, HypothesisStatus::Rejected);
                        // Dual-write to SQLite.
                        if let Some(h) = j.hypotheses.iter().find(|h| h.id == id) { db.upsert_hypothesis(h); }
                        j.save(&jpath).ok();
                        emit_journal_hypothesis(&audit_path, id, &stmt, "rejected", None).ok();
                        Ok(serde_json::json!({"updated": ok, "status": "rejected"}))
                    }
                    "set_hypothesis_status" => {
                        let id         = p["hypothesis_id"].as_str().ok_or("missing hypothesis_id")?;
                        let status_str = p["status"].as_str().ok_or("missing status")?;
                        let status = match status_str {
                            "proposed"  => HypothesisStatus::Proposed,
                            "testing"   => HypothesisStatus::Testing,
                            "confirmed" => HypothesisStatus::Confirmed,
                            "rejected"  => HypothesisStatus::Rejected,
                            s           => return Err(format!("unknown status: {s}")),
                        };
                        let stmt = j.hypotheses.iter().find(|h| h.id == id)
                            .map(|h| h.statement.clone()).unwrap_or_default();
                        let ok = j.update_hypothesis_status(id, status);
                        // Dual-write to SQLite.
                        if let Some(h) = j.hypotheses.iter().find(|h| h.id == id) { db.upsert_hypothesis(h); }
                        j.save(&jpath).ok();
                        emit_journal_hypothesis(&audit_path, id, &stmt, status_str, None).ok();
                        Ok(serde_json::json!({"updated": ok}))
                    }
                    _ => Err(format!("unknown action: {action}")),
                }
            })
        }),
    );
}

/// Register `design_experiment`: generate a DoE run matrix.
///
/// The LLM calls this tool with a design type and factor list.
/// The returned run matrix guides the subsequent protocol steps.
fn register_doe_tool(registry: &mut ToolRegistry) {
    use scientific_compute::doe::{Factor, central_composite, full_factorial, latin_hypercube};

    registry.register(
        ToolSpec {
            name: "design_experiment".into(),
            description: concat!(
                "Generate a Design of Experiments (DoE) run matrix. ",
                "design_type: 'full_factorial' (k≤5), 'central_composite' (2≤k≤4), or 'latin_hypercube'. ",
                "factors: array of {name, unit, low, high}. ",
                "n_runs: only for latin_hypercube (default 20). ",
                "Returns run matrix as JSON rows. ",
                "IMPORTANT: pass the entire returned JSON string as doe_design_json in your propose_protocol call ",
                "to link this design to the protocol for automatic one-way ANOVA at conclusion."
            ).into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "design_type": {
                        "type": "string",
                        "enum": ["full_factorial", "central_composite", "latin_hypercube"]
                    },
                    "factors": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "unit": {"type": "string"},
                                "low":  {"type": "number"},
                                "high": {"type": "number"}
                            },
                            "required": ["name", "unit", "low", "high"]
                        }
                    },
                    "n_runs": {"type": "number", "description": "Only for latin_hypercube (default 20)"},
                    "seed":   {"type": "number", "description": "Random seed for latin_hypercube (default 42)"}
                },
                "required": ["design_type", "factors"]
            }),
            parameter_units: HashMap::new(),
            instrument_uncertainty: None,
        },
        Box::new(|p| Box::pin(async move {
            let design_type = p["design_type"].as_str().ok_or("missing design_type")?;
            let raw_factors = p["factors"].as_array().ok_or("factors must be an array")?;

            let factors: Vec<Factor> = raw_factors.iter().map(|f| -> Result<Factor, String> {
                Ok(Factor {
                    name:   f["name"].as_str().ok_or("factor missing name")?.to_string(),
                    unit:   f["unit"].as_str().unwrap_or("").to_string(),
                    low:    f["low"].as_f64().ok_or("factor missing low")?,
                    high:   f["high"].as_f64().ok_or("factor missing high")?,
                    levels: None,
                })
            }).collect::<Result<Vec<_>, _>>()?;

            let design = match design_type {
                "full_factorial"      => full_factorial(&factors)?,
                "central_composite"   => central_composite(&factors)?,
                "latin_hypercube" => {
                    let n_runs = p["n_runs"].as_f64().unwrap_or(20.0) as usize;
                    let seed   = p["seed"].as_f64().unwrap_or(42.0) as u64;
                    latin_hypercube(&factors, n_runs, seed)?
                }
                other => return Err(format!("unknown design_type: {other}")),
            };

            Ok(serde_json::to_value(&design).unwrap())
        })),
    );
}
