use crate::discovery::{journal_path, DiscoveryJournal, HypothesisStatus};
use scientific_compute::fitting::{
    hill_equation_fit, linear_regression, michaelis_menten_fit, model_select_aic, PreferredModel,
};
use agent_runtime::{
    audit::{audit_log_path, emit_journal_finding, emit_journal_hypothesis},
    hardware::SiLA2Clients,
    protocol::propose_protocol_schema,
    sandbox::{ResourceLimits, Sandbox},
    tools::{ToolRegistry, ToolSpec},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

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
    clients: Arc<SiLA2Clients>,
    journal: Arc<Mutex<DiscoveryJournal>>,
) -> ToolRegistry {
    let mut r = ToolRegistry::new();

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "dispense".into(),
            description: "Dispense liquid into a vessel (volume_ul, pump_id).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"pump_id":{"type":"string"},"volume_ul":{"type":"number"}},"required":["pump_id","volume_ul"]}),
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
            parameters_schema: serde_json::json!({"type":"object","properties":{"source_vessel":{"type":"string"},"volume_ul":{"type":"number"}},"required":["source_vessel","volume_ul"]}),
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
            parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
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
        },
        Box::new(move |_p| { let c = c.clone(); Box::pin(async move { c.read_temperature().await })}),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "spin_centrifuge".into(),
            description: "Spin centrifuge (rcf, duration_seconds, temperature_celsius).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"rcf":{"type":"number"},"duration_seconds":{"type":"number"},"temperature_celsius":{"type":"number"}},"required":["rcf","duration_seconds","temperature_celsius"]}),
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
            parameters_schema: serde_json::json!({"type":"object","properties":{"sample_id":{"type":"string"}},"required":["sample_id"]}),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let sample = p["sample_id"].as_str().ok_or("missing sample_id")?;
            c.read_ph(sample).await
        })}),
    );

    r.register(
        ToolSpec {
            name: "read_sensor".into(),
            description: "Read a named sensor value.".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"sensor_id":{"type":"string"}},"required":["sensor_id"]}),
        },
        Box::new(|p| Box::pin(async move {
            let id = p["sensor_id"].as_str().ok_or("missing sensor_id")?;
            Ok(serde_json::json!({"sensor_id": id, "value": 7.04, "unit": "pH", "source": "STUB"}))
        })),
    );

    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "incubate".into(),
            description: "Incubate for a specified duration (duration_minutes).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"duration_minutes":{"type":"number"}},"required":["duration_minutes"]}),
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
        },
        Box::new(|_p| Box::pin(async move {
            Err("propose_protocol is handled by the orchestrator".into())
        })),
    );

    register_analyze_series_tool(&mut r);
    register_journal_tool(&mut r, journal);
    r
}

/// Fallback tool registry when no SiLA 2 server is available.
pub(crate) fn make_mock_tools(journal: Arc<Mutex<DiscoveryJournal>>) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    agent_runtime::tools::register_lab_tools(&mut r);

    let mock_extras: &[(&str, &str, serde_json::Value, serde_json::Value)] = &[
        ("aspirate", "Aspirate liquid from a vessel.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"volume_ul":{"type":"number"}},"required":["vessel_id","volume_ul"]}),
            serde_json::json!({"status":"aspirated"})),
        ("read_absorbance", "UV/Vis absorbance measurement.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
            serde_json::json!({"absorbance":0.847,"wavelength_nm":595,"unit":"AU"})),
        ("read_ph", "Read vessel pH.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"}},"required":["vessel_id"]}),
            serde_json::json!({"ph":7.2,"unit":"pH"})),
        ("read_temperature", "Read vessel temperature.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"}},"required":["vessel_id"]}),
            serde_json::json!({"temperature_mk":298150,"unit":"mK"})),
        ("set_temperature", "Set target temperature.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"target_mk":{"type":"number"}},"required":["vessel_id","target_mk"]}),
            serde_json::json!({"status":"temperature_set"})),
        ("spin_centrifuge", "Spin centrifuge.",
            serde_json::json!({"type":"object","properties":{"rcf":{"type":"number"},"duration_seconds":{"type":"number"},"temperature_celsius":{"type":"number"}},"required":["rcf","duration_seconds","temperature_celsius"]}),
            serde_json::json!({"status":"centrifuged"})),
        ("calibrate_ph", "Calibrate pH meter.",
            serde_json::json!({"type":"object","properties":{"buffer_ph1":{"type":"number"},"buffer_ph2":{"type":"number"}},"required":["buffer_ph1","buffer_ph2"]}),
            serde_json::json!({"status":"calibrated"})),
        ("incubate", "Incubate for duration.",
            serde_json::json!({"type":"object","properties":{"duration_minutes":{"type":"number"}},"required":["duration_minutes"]}),
            serde_json::json!({"status":"incubated"})),
    ];

    for (name, desc, schema, result) in mock_extras {
        let result = result.clone();
        r.register(
            ToolSpec { name: (*name).into(), description: (*desc).into(), parameters_schema: schema.clone() },
            Box::new(move |_| { let r = result.clone(); Box::pin(async move { Ok(r) }) }),
        );
    }

    r.register(
        ToolSpec {
            name: "propose_protocol".into(),
            description: "Propose a structured multi-step experimental protocol. \
                Use this for any experiment with 2+ steps. The runtime executes each \
                step through the full safety pipeline and returns a signed audit record.".into(),
            parameters_schema: propose_protocol_schema(),
        },
        Box::new(|_p| Box::pin(async move {
            Err("propose_protocol is handled by the orchestrator".into())
        })),
    );

    register_analyze_series_tool(&mut r);
    register_journal_tool(&mut r, journal);
    r
}

/// Register the `analyze_series` tool: fit OLS / Hill / Michaelis-Menten to (x,y) data.
fn register_analyze_series_tool(registry: &mut ToolRegistry) {
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
        },
        Box::new(|p| Box::pin(async move {
            let data = p["data"].as_array().ok_or("missing data array")?;
            if data.is_empty() {
                return Err("data array is empty".into());
            }

            let xs: Vec<f64> = data.iter().map(|d| d["x"].as_f64().unwrap_or(0.0)).collect();
            let ys: Vec<f64> = data.iter().map(|d| d["y"].as_f64().unwrap_or(0.0)).collect();

            let model   = p["model"].as_str().unwrap_or("auto");
            let x_label = p["x_label"].as_str().unwrap_or("x");
            let y_label = p["y_label"].as_str().unwrap_or("y");

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

            Ok(result)
        })),
    );
}

/// Register the `update_journal` tool: LLM-driven discovery journal mutations.
fn register_journal_tool(registry: &mut ToolRegistry, journal: Arc<Mutex<DiscoveryJournal>>) {
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
                    "hypothesis_id":  {"type": "string"},
                    "status": {
                        "type": "string",
                        "enum": ["proposed", "testing", "confirmed", "rejected"]
                    }
                },
                "required": ["action"]
            }),
        },
        Box::new(move |p| {
            let journal    = Arc::clone(&journal);
            let jpath      = jpath.clone();
            let audit_path = audit_log_path().to_string_lossy().into_owned();
            Box::pin(async move {
                let action = p["action"].as_str().ok_or("missing action")?;
                let mut j  = journal.lock().map_err(|_| "journal lock poisoned")?;
                match action {
                    "add_finding" => {
                        let stmt     = p["statement"].as_str().ok_or("missing statement")?.to_string();
                        let ev       = p["evidence"].as_str().unwrap_or("").to_string();
                        let evidence = if ev.is_empty() { vec![] } else { vec![ev.clone()] };
                        let id       = j.add_finding(stmt.clone(), evidence);
                        j.save(&jpath).ok();
                        emit_journal_finding(&audit_path, &id, &stmt, &ev, None).ok();
                        Ok(serde_json::json!({"recorded": "finding", "id": id, "statement": stmt}))
                    }
                    "add_hypothesis" => {
                        let stmt = p["statement"].as_str().ok_or("missing statement")?.to_string();
                        let id   = j.add_hypothesis(stmt.clone());
                        j.save(&jpath).ok();
                        emit_journal_hypothesis(&audit_path, &id, &stmt, "proposed", None).ok();
                        Ok(serde_json::json!({"recorded": "hypothesis", "id": id, "statement": stmt}))
                    }
                    "confirm_hypothesis" => {
                        let id   = p["hypothesis_id"].as_str().ok_or("missing hypothesis_id")?;
                        let stmt = j.hypotheses.iter().find(|h| h.id == id)
                            .map(|h| h.statement.clone()).unwrap_or_default();
                        let ok = j.update_hypothesis_status(id, HypothesisStatus::Confirmed);
                        j.save(&jpath).ok();
                        emit_journal_hypothesis(&audit_path, id, &stmt, "confirmed", None).ok();
                        Ok(serde_json::json!({"updated": ok, "status": "confirmed"}))
                    }
                    "reject_hypothesis" => {
                        let id   = p["hypothesis_id"].as_str().ok_or("missing hypothesis_id")?;
                        let stmt = j.hypotheses.iter().find(|h| h.id == id)
                            .map(|h| h.statement.clone()).unwrap_or_default();
                        let ok = j.update_hypothesis_status(id, HypothesisStatus::Rejected);
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
