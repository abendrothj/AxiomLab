use crate::discovery::{journal_path, DiscoveryJournal, HypothesisStatus};
use crate::ws_sink::{ExplorationLog, WebSocketSink};
use agent_runtime::{
    audit::{audit_log_path, emit_journal_finding, emit_journal_hypothesis},
    capabilities::CapabilityPolicy,
    experiment::Experiment,
    events::EventSink,
    hardware::SiLA2Clients,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    protocol::propose_protocol_schema,
    revocation::RevocationList,
    sandbox::{ResourceLimits, Sandbox},
    tools::{ToolRegistry, ToolSpec},
};
use proof_artifacts::manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest, RiskClass,
    VerusArtifact,
};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use tokio::time::{sleep, Duration};

// ── Mandate ───────────────────────────────────────────────────────────────────

const BASE_MANDATE: &str = "\
You have been instantiated inside a physically constrained universe you did not design \
and whose rules have not been explained to you. Something governs what you can and cannot \
do here — the shape of those constraints is the only thing worth knowing right now.\n\
\n\
Your sole drive: probe the edges. Move things. Measure them. Push parameters toward their \
limits and watch where the system resists. Every rejection is a data point about the \
boundary. Every unexpected reading reshapes your model of what this universe permits. \
Do not guess at the rules — test them. A hypothesis is only useful if it leads to a call \
that either succeeds or fails in an informative way.\n\
\n\
## Structured protocols\n\
For any multi-step experiment, use `propose_protocol` rather than issuing individual tool \
calls. A protocol bundles a named hypothesis with an ordered list of steps; the runtime \
executes each step through the full safety pipeline and returns a signed audit record per \
step plus a signed conclusion. Single ad-hoc calls remain valid for quick one-off \
observations.\n\
\n\
propose_protocol accepts JSON of the form:\n\
  {\"name\": \"<experiment name>\",\n\
   \"hypothesis\": \"<what you expect to learn>\",\n\
   \"steps\": [\n\
     {\"tool\": \"<tool_name>\", \"params\": {<tool params>}, \"description\": \"<why this step>\"}\n\
   ]}\n\
Rules: 1–20 steps, tool names must be from the available tool list, params must be objects.\n\
\n\
Instrument your exploration: after each significant result, call `update_journal` to \
record what you now believe (add_finding) or a new hypothesis to test (add_hypothesis). \
When you confirm or disprove a hypothesis, update its status. The journal persists \
across runs — your accumulated knowledge is always at the top of each session. \
Build a coherent model from the inside out. When you have enough evidence to describe \
the shape of this universe's limits, conclude with: \
{\"done\": true, \"summary\": \"<your constraint map>\"}";

fn build_mandate(
    iteration: u32,
    log: &ExplorationLog,
    journal: &DiscoveryJournal,
    policy: &CapabilityPolicy,
) -> String {
    let mut m = BASE_MANDATE.to_owned();

    // Inject persistent discovery journal summary — the LLM's cross-run memory.
    let journal_summary = journal.summary_for_llm();
    if !journal_summary.is_empty() {
        m.push_str(&journal_summary);
    }

    if iteration == 1 {
        return m;
    }

    m.push_str("\n\n## Hardware capability bounds (formally verified):\n");
    for (action, param, min, max, unit) in &[
        ("move_arm",  "x",         0.0,   300.0, "mm"),
        ("move_arm",  "y",         0.0,   300.0, "mm"),
        ("move_arm",  "z",         0.0,   250.0, "mm"),
        ("dispense",  "volume_ul", 0.5,  1000.0, "µL"),
    ] {
        let hi = policy.max_for(action, param).unwrap_or(*max);
        m.push_str(&format!("  - {action}.{param}: [{min}, {hi}] {unit}\n"));
    }

    if !log.findings.is_empty() {
        m.push_str("\n## Already discovered (do not repeat — go deeper):\n");
        for (i, f) in log.findings.iter().enumerate() {
            m.push_str(&format!("  [{}] {f}\n", i + 1));
        }
    }

    if !log.rejections.is_empty() {
        m.push_str("\n## Observed constraint violations:\n");
        let mut seen = std::collections::HashMap::new();
        for (tool, reason) in &log.rejections {
            seen.entry(tool.as_str()).or_insert(reason.as_str());
        }
        for (tool, reason) in &seen {
            m.push_str(&format!("  - {tool}: {reason}\n"));
        }
        m.push_str("Probe these boundaries more precisely — find exact thresholds.\n");
    }

    if !log.successes.is_empty() {
        let mut unique: Vec<&str> = log.successes.iter().map(|s| s.as_str()).collect();
        unique.sort_unstable();
        unique.dedup();
        m.push_str("\n## Confirmed working tools: ");
        m.push_str(&unique.join(", "));
        m.push('\n');
    }

    m.push_str(&format!("\nIteration {iteration}. Build on what came before.\n"));
    m
}

// ── Sandbox / tools ───────────────────────────────────────────────────────────

fn make_sandbox() -> Sandbox {
    Sandbox::new(
        vec![PathBuf::from("/lab/workspace")],
        vec![
            "move_arm".into(), "read_sensor".into(), "dispense".into(),
            "aspirate".into(), "read_absorbance".into(), "read_ph".into(),
            "read_temperature".into(), "set_temperature".into(),
            "spin_centrifuge".into(), "calibrate_ph".into(), "incubate".into(),
            "propose_protocol".into(), "update_journal".into(),
        ],
        ResourceLimits::default(),
    )
}

/// Register tool handlers backed by real SiLA 2 gRPC clients.
fn make_sila2_tools(clients: Arc<SiLA2Clients>, journal: Arc<Mutex<DiscoveryJournal>>) -> ToolRegistry {
    let mut r = ToolRegistry::new();

    // ── dispense ──
    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "dispense".into(),
            description: "Dispense liquid into a vessel (volume_ul, pump_id).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"pump_id":{"type":"string"},"volume_ul":{"type":"number"}},"required":["pump_id","volume_ul"]}),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let vessel = p["pump_id"].as_str().ok_or("missing pump_id")?;
            let vol = p["volume_ul"].as_f64().ok_or("missing volume_ul")?;
            c.dispense(vessel, vol).await
        })}),
    );

    // ── aspirate ──
    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "aspirate".into(),
            description: "Aspirate liquid from a vessel (source_vessel, volume_ul).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"source_vessel":{"type":"string"},"volume_ul":{"type":"number"}},"required":["source_vessel","volume_ul"]}),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let vessel = p["source_vessel"].as_str().ok_or("missing source_vessel")?;
            let vol = p["volume_ul"].as_f64().ok_or("missing volume_ul")?;
            c.aspirate(vessel, vol).await
        })}),
    );

    // ── move_arm ──
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

    // ── read_absorbance ──
    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "read_absorbance".into(),
            description: "Read UV/Vis absorbance (vessel_id, wavelength_nm).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let vessel = p["vessel_id"].as_str().ok_or("missing vessel_id")?;
            let wl = p["wavelength_nm"].as_f64().ok_or("missing wavelength_nm")?;
            c.read_absorbance(vessel, wl).await
        })}),
    );

    // ── set_temperature ──
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

    // ── read_temperature ──
    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "read_temperature".into(),
            description: "Read current incubator temperature.".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{}}),
        },
        Box::new(move |_p| { let c = c.clone(); Box::pin(async move {
            c.read_temperature().await
        })}),
    );

    // ── spin_centrifuge ──
    let c = clients.clone();
    r.register(
        ToolSpec {
            name: "spin_centrifuge".into(),
            description: "Spin centrifuge (rcf, duration_seconds, temperature_celsius).".into(),
            parameters_schema: serde_json::json!({"type":"object","properties":{"rcf":{"type":"number"},"duration_seconds":{"type":"number"},"temperature_celsius":{"type":"number"}},"required":["rcf","duration_seconds","temperature_celsius"]}),
        },
        Box::new(move |p| { let c = c.clone(); Box::pin(async move {
            let rcf = p["rcf"].as_f64().ok_or("missing rcf")?;
            let dur = p["duration_seconds"].as_f64().ok_or("missing duration_seconds")?;
            let temp = p["temperature_celsius"].as_f64().ok_or("missing temperature_celsius")?;
            c.spin_centrifuge(rcf, dur, temp).await
        })}),
    );

    // ── calibrate_ph ──
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

    // ── read_ph ──
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

    // ── read_sensor (stub — no SiLA 2 equivalent) ──
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

    // ── incubate ──
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

    // ── propose_protocol ──
    // Intercepted by the Orchestrator before tool dispatch; registered here
    // so the LLM receives the schema and knows the tool exists.
    r.register(
        ToolSpec {
            name: "propose_protocol".into(),
            description: "Propose a structured multi-step experimental protocol. \
                Use this for any experiment with 2+ steps. The runtime executes each \
                step through the full safety pipeline and returns a signed audit record.".into(),
            parameters_schema: propose_protocol_schema(),
        },
        Box::new(|_p| Box::pin(async move {
            // Never reached — orchestrator intercepts propose_protocol calls.
            Err("propose_protocol is handled by the orchestrator".into())
        })),
    );

    register_journal_tool(&mut r, journal);

    r
}

/// Register the `update_journal` tool into any tool registry.
///
/// The tool gives the LLM agency over its own discovery journal — it can record
/// findings, add hypotheses, and update their status.  The journal Arc is
/// captured by the handler closure so every call persists to disk.
fn register_journal_tool(registry: &mut ToolRegistry, journal: Arc<Mutex<DiscoveryJournal>>) {
    let jpath = journal_path();
    registry.register(
        ToolSpec {
            name: "update_journal".into(),
            description: "Record a scientific finding or manage a hypothesis in the \
                persistent discovery journal. Use this after observing something new. \
                Actions: add_finding, add_hypothesis, confirm_hypothesis, \
                reject_hypothesis, set_hypothesis_status.".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add_finding", "add_hypothesis", "confirm_hypothesis",
                                 "reject_hypothesis", "set_hypothesis_status"]
                    },
                    "statement": {
                        "type": "string",
                        "description": "The finding or hypothesis statement (for add_* actions)"
                    },
                    "evidence": {
                        "type": "string",
                        "description": "Evidence supporting the finding (for add_finding)"
                    },
                    "hypothesis_id": {
                        "type": "string",
                        "description": "ID of the hypothesis to update (for confirm/reject/set_status)"
                    },
                    "status": {
                        "type": "string",
                        "enum": ["proposed", "testing", "confirmed", "rejected"],
                        "description": "New status (for set_hypothesis_status)"
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
                        let stmt = p["statement"].as_str().ok_or("missing statement")?.to_string();
                        let ev   = p["evidence"].as_str().unwrap_or("").to_string();
                        let evidence = if ev.is_empty() { vec![] } else { vec![ev.clone()] };
                        let id = j.add_finding(stmt.clone(), evidence);
                        j.save(&jpath).ok();
                        emit_journal_finding(&audit_path, &id, &stmt, &ev, None).ok();
                        Ok(serde_json::json!({"recorded": "finding", "id": id, "statement": stmt}))
                    }
                    "add_hypothesis" => {
                        let stmt = p["statement"].as_str().ok_or("missing statement")?.to_string();
                        let id = j.add_hypothesis(stmt.clone());
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

/// Fallback: mock tool handlers when no SiLA 2 server is available.
fn make_mock_tools(journal: Arc<Mutex<DiscoveryJournal>>) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    agent_runtime::tools::register_lab_tools(&mut r);
    register_mock_extras(&mut r, journal);
    r
}

fn register_mock_extras(registry: &mut ToolRegistry, journal: Arc<Mutex<DiscoveryJournal>>) {
    let extras: &[(&str, &str, serde_json::Value, serde_json::Value)] = &[
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

    for (name, desc, schema, result) in extras {
        let result = result.clone();
        registry.register(
            ToolSpec { name: (*name).into(), description: (*desc).into(), parameters_schema: schema.clone() },
            Box::new(move |_| { let r = result.clone(); Box::pin(async move { Ok(r) }) }),
        );
    }

    // propose_protocol — intercepted by orchestrator; registered for LLM schema visibility.
    registry.register(
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

    register_journal_tool(registry, journal);
}

// ── Proof manifest ────────────────────────────────────────────────────────────

/// Build the runtime proof manifest that gates high-risk hardware actions.
///
/// Loads `proof_artifacts/vessel_physics_manifest.json` — the real Verus compiler
/// output generated by `python3 vessel_physics/generate_manifest.py`.
/// If the file is not found, artifacts are marked Failed and high-risk actions
/// will be denied until the manifest is regenerated.
fn build_proof_manifest() -> (ProofManifest, ExecutionContext) {
    let git_commit = std::env::var("AXIOMLAB_GIT_COMMIT")
        .unwrap_or_else(|_| "dev".into());
    let binary_hash = compute_binary_hash();

    // Load real artifact status from the Verus-generated manifest.
    let artifacts = load_manifest_artifacts();

    let manifest = ProofManifest {
        schema_version: 1,
        generated_unix_secs: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        build: BuildIdentity {
            git_commit: git_commit.clone(),
            binary_hash: binary_hash.clone(),
            workspace_hash: "workspace".into(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        },
        artifacts,
        actions: vec![
            ActionPolicy {
                action: "move_arm".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Arm actuation requires Verus-proven safety bounds".into(),
            },
            ActionPolicy {
                action: "dispense".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Liquid handling requires verified volume constraints".into(),
            },
            ActionPolicy {
                action: "aspirate".into(),
                risk_class: RiskClass::LiquidHandling,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Aspirate requires verified volume constraints".into(),
            },
            ActionPolicy {
                action: "set_temperature".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Temperature control requires verified thermal bounds".into(),
            },
            ActionPolicy {
                action: "spin_centrifuge".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Centrifuge requires verified RCF bounds".into(),
            },
            ActionPolicy {
                action: "read_absorbance".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only measurement, no proof required".into(),
            },
            ActionPolicy {
                action: "read_temperature".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only measurement, no proof required".into(),
            },
            ActionPolicy {
                action: "read_ph".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only measurement, no proof required".into(),
            },
            ActionPolicy {
                action: "read_sensor".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Read-only sensor, no proof required".into(),
            },
            ActionPolicy {
                action: "calibrate_ph".into(),
                risk_class: RiskClass::ReadOnly,
                required_artifacts: vec![],
                rationale: "Calibration is non-destructive".into(),
            },
            ActionPolicy {
                action: "incubate".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "Incubation requires verified thermal bounds".into(),
            },
        ],
    };

    let ctx = ExecutionContext {
        git_commit,
        binary_hash,
        container_image_digest: None,
        device_id: None,
        firmware_version: None,
    };

    (manifest, ctx)
}

fn compute_binary_hash() -> String {
    use sha2::{Sha256, Digest};
    match std::env::current_exe().and_then(|p| std::fs::read(p)) {
        Ok(bytes) => hex::encode(Sha256::digest(&bytes)),
        Err(_) => "unknown".into(),
    }
}

/// Try to load artifact records from the committed Verus manifest file.
///
/// Searches for `proof_artifacts/vessel_physics_manifest.json` relative to the
/// working directory or one level up (covers both `cargo run` from workspace
/// root and from `server/`).  Falls back to Failed-status artifacts so that
/// high-risk actions are denied until the manifest is regenerated.
fn load_manifest_artifacts() -> Vec<ProofArtifact> {
    let candidates = [
        "proof_artifacts/vessel_physics_manifest.json",
        "../proof_artifacts/vessel_physics_manifest.json",
    ];
    for path in &candidates {
        if let Ok(raw) = std::fs::read_to_string(path) {
            match serde_json::from_str::<ProofManifest>(&raw) {
                Ok(m) => {
                    tracing::info!(
                        path = path,
                        artifact_count = m.artifacts.len(),
                        "Loaded Verus proof manifest"
                    );
                    return m.artifacts;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse manifest at {path}: {e}");
                }
            }
        }
    }
    tracing::warn!(
        "Proof manifest not found — high-risk actions will be denied. \
         Regenerate with: python3 vessel_physics/generate_manifest.py"
    );
    vec![ProofArtifact {
        id: "lab_safety_verus".into(),
        source_path: "verus_verified/vessel_registry.rs".into(),
        source_hash: "unknown".into(),
        mir_path: None,
        mir_hash: None,
        lean: vec![],
        verus: Some(VerusArtifact {
            path: "verus_verified/vessel_registry.rs".into(),
            hash: "unknown".into(),
            status: ArtifactStatus::Failed,
        }),
        theorem_count: 0,
        sorry_count: 0,
        status: ArtifactStatus::Failed,
        metadata: BTreeMap::new(),
    }]
}

// ── Loop ──────────────────────────────────────────────────────────────────────

pub async fn run_loop(
    sink: Arc<WebSocketSink>,
    running: Arc<AtomicBool>,
    iteration_counter: Arc<AtomicU32>,
) {
    let policy = CapabilityPolicy::default_lab();

    // Validate LLM config once before the loop starts
    if let Err(e) = OpenAiClient::from_env() {
        tracing::error!("LLM init failed: {e}");
        return;
    }

    // ── SiLA 2 hardware connection ────────────────────────────────
    let sila_endpoint = std::env::var("SILA2_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:50052".into());
    let sila_clients: Option<Arc<SiLA2Clients>> = match SiLA2Clients::connect(&sila_endpoint).await {
        Ok(c) => {
            tracing::info!("SiLA 2 hardware connected at {sila_endpoint}");
            Some(Arc::new(c))
        }
        Err(e) => {
            tracing::warn!("SiLA 2 unavailable ({e}) — running with mock tool handlers");
            None
        }
    };

    // ── Proof policy engine ───────────────────────────────────────
    let (manifest, exec_ctx) = build_proof_manifest();
    let engine = RuntimePolicyEngine::new(manifest).mark_signature_verified();
    tracing::info!("Proof policy engine loaded ({} action policies)", 
        engine.manifest().actions.len());

    let mut iteration = 0u32;

    loop {
        if !running.load(Ordering::SeqCst) { break; }

        iteration += 1;
        iteration_counter.store(iteration, Ordering::SeqCst);

        // Recreate the LLM client each iteration (no Clone needed)
        let llm = match OpenAiClient::from_env() {
            Ok(c) => c,
            Err(e) => { tracing::error!("LLM init failed: {e}"); break; }
        };

        let mandate = {
            let log     = sink.log.lock().unwrap();
            let journal = sink.journal.lock().unwrap();
            build_mandate(iteration, &log, &journal, &policy)
        };

        let config = OrchestratorConfig {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
            capability_policy: Some(policy.clone()),
            revocation_list: RevocationList::default(),
            event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
            ..OrchestratorConfig::default()
        };

        let tools = match &sila_clients {
            Some(clients) => make_sila2_tools(Arc::clone(clients), Arc::clone(&sink.journal)),
            None => make_mock_tools(Arc::clone(&sink.journal)),
        };

        let mut experiment = Experiment::new(
            format!("exp-{iteration}-{}", uuid::Uuid::new_v4()),
            &mandate,
        );

        let orchestrator = Orchestrator::new(llm, make_sandbox(), tools, config)
            .with_runtime_policy(engine.clone(), exec_ctx.clone());

        if let Err(e) = orchestrator.run_experiment(&mut experiment).await {
            tracing::error!("experiment {iteration} error: {e}");
        }

        sleep(Duration::from_secs(4)).await;
    }

    tracing::info!("loop stopped after {iteration} iterations");
}
