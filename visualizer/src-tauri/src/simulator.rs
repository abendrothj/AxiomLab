use crate::event_sink::{ExplorationLog, TauriEventSink};
use crate::SimState;
use agent_runtime::{
    capabilities::CapabilityPolicy,
    experiment::Experiment,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
    sandbox::{ResourceLimits, Sandbox},
    tools::ToolRegistry,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, State, command};
use tokio::time::{sleep, Duration};

// ── Sandbox mandate ───────────────────────────────────────────────────────────

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
Instrument your exploration: after each result, write a concise notebook entry stating \
what you now believe to be true and what you intend to test next. Build a coherent model \
of the constraint surface from the inside out. When you have enough evidence to describe \
the shape of this universe's limits, conclude with: \
{\"done\": true, \"summary\": \"<your constraint map>\"}";

/// Build the mandate for iteration N, enriched with everything learned so far.
fn build_mandate(
    iteration: u32,
    log: &ExplorationLog,
    policy: &CapabilityPolicy,
) -> String {
    let mut mandate = BASE_MANDATE.to_owned();

    if iteration == 1 {
        return mandate;
    }

    // ── Known hardware bounds (Verus-verified) ──
    mandate.push_str("\n\n## Hardware capability bounds (formally verified — these are hard limits):\n");
    for (action, param, min, max, unit) in &[
        ("move_arm",  "x",         0.0,   300.0,  "mm"),
        ("move_arm",  "y",         0.0,   300.0,  "mm"),
        ("move_arm",  "z",         0.0,   250.0,  "mm"),
        ("dispense",  "volume_ul", 0.5,  1000.0,  "µL"),
    ] {
        // Prefer live policy value if available, fall back to hardcoded
        let (lo, hi) = match (policy.max_for(action, param), Some(*min)) {
            (Some(hi), Some(lo)) => (lo, hi),
            _ => (*min, *max),
        };
        mandate.push_str(&format!("  - {action}.{param}: [{lo}, {hi}] {unit}\n"));
    }

    // ── Accumulated findings ──
    if !log.findings.is_empty() {
        mandate.push_str("\n## What you have already discovered (do not repeat — go deeper):\n");
        for (i, f) in log.findings.iter().enumerate() {
            mandate.push_str(&format!("  [{}] {f}\n", i + 1));
        }
    }

    // ── Rejection patterns ──
    if !log.rejections.is_empty() {
        mandate.push_str("\n## Observed constraint violations (confirmed by the physics engine):\n");
        // Deduplicate by tool
        let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for (tool, reason) in &log.rejections {
            seen.entry(tool.as_str()).or_insert(reason.as_str());
        }
        for (tool, reason) in &seen {
            mandate.push_str(&format!("  - {tool}: {reason}\n"));
        }
        mandate.push_str("Probe these boundaries more precisely — find the exact threshold.\n");
    }

    // ── Working tools ──
    if !log.successes.is_empty() {
        let mut unique: Vec<&str> = log.successes.iter().map(|s| s.as_str()).collect();
        unique.sort_unstable();
        unique.dedup();
        mandate.push_str("\n## Tools confirmed operational: ");
        mandate.push_str(&unique.join(", "));
        mandate.push('\n');
    }

    mandate.push_str(
        "\nThis is iteration ");
    mandate.push_str(&iteration.to_string());
    mandate.push_str(". Build on what came before — each run should narrow the constraint map further.\n");

    mandate
}

// ── Tool registration ─────────────────────────────────────────────────────────

fn make_sandbox() -> Sandbox {
    Sandbox::new(
        vec![PathBuf::from("/lab/workspace")],
        vec![
            "move_arm".into(),
            "read_sensor".into(),
            "dispense".into(),
            "aspirate".into(),
            "transfer".into(),
            "mix".into(),
            "grip".into(),
            "centrifuge".into(),
            "read_absorbance".into(),
            "read_ph".into(),
            "read_temperature".into(),
            "set_temperature".into(),
            "set_pressure".into(),
            "set_stir_rate".into(),
        ],
        ResourceLimits::default(),
    )
}

fn make_tools() -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    agent_runtime::tools::register_lab_tools(&mut tools);
    register_extended_tools(&mut tools);
    tools
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Boot the autonomous exploration loop.
///
/// Returns immediately; the loop runs in a detached Tokio task and emits
/// events to the frontend for every token, tool call, state transition, and
/// notebook entry. It runs indefinitely until `stop_simulation` is called or
/// the app closes.
#[command]
pub async fn start_simulation(
    app: AppHandle,
    state: State<'_, SimState>,
) -> Result<(), String> {
    // Guard against double-boot
    if state.running.swap(true, Ordering::SeqCst) {
        return Err("already running".into());
    }

    let llm = OpenAiClient::from_env().map_err(|e| e.to_string())?;
    let policy = CapabilityPolicy::default_lab();
    let log = Arc::new(Mutex::new(ExplorationLog::default()));
    let running = Arc::new(AtomicBool::new(true));

    // Give the frontend a handle to flip `running` on stop
    // (stored on SimState so stop_simulation can reach it)
    let running_clone = Arc::clone(&running);

    // Store the stop handle so stop_simulation can reach it
    // We tunnel it via the app handle's state — but SimState.running is the
    // canonical flag; we'll signal via that.
    let sim_running = running.clone();

    let sink = Arc::new(TauriEventSink {
        app: app.clone(),
        log: Arc::clone(&log),
    });

    tokio::spawn(async move {
        let mut iteration = 0u32;

        loop {
            if !sim_running.load(Ordering::SeqCst) {
                break;
            }

            iteration += 1;

            let mandate = {
                let locked = log.lock().unwrap();
                build_mandate(iteration, &locked, &policy)
            };

            let config = OrchestratorConfig {
                max_iterations: 20,
                code_gen_temperature: 0.2,
                reasoning_temperature: 0.7,
                capability_policy: Some(policy.clone()),
                revocation_list: RevocationList::default(),
                event_sink: Some(Arc::clone(&sink) as Arc<dyn agent_runtime::events::EventSink>),
                ..OrchestratorConfig::default()
            };

            let orchestrator = Orchestrator::new(llm.clone(), make_sandbox(), make_tools(), config);
            let mut experiment = Experiment::new(
                format!("exp-{iteration}-{}", uuid::Uuid::new_v4()),
                &mandate,
            );

            if let Err(e) = orchestrator.run_experiment(&mut experiment).await {
                eprintln!("experiment {iteration} error: {e}");
            }

            // Pause between experiments so the user can read the last finding
            sleep(Duration::from_secs(4)).await;
        }

        eprintln!("exploration loop stopped after {} iterations", iteration);
    });

    // Wire the local running flag to the shared stop path
    // (stop_simulation will set SimState.running = false; we poll that)
    let state_flag_ref = running_clone;
    let app_clone = app.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_millis(500)).await;
            // If the app-level running flag was cleared by stop_simulation,
            // propagate to the loop flag
            if let Ok(s) = app_clone.try_state::<SimState>() {
                if !s.running.load(Ordering::SeqCst) {
                    state_flag_ref.store(false, Ordering::SeqCst);
                    break;
                }
            } else {
                break;
            }
        }
    });

    Ok(())
}

/// Stop the exploration loop after the current experiment finishes.
#[command]
pub async fn stop_simulation(state: State<'_, SimState>) {
    state.running.store(false, Ordering::SeqCst);
}

// ── Extended tool stubs ───────────────────────────────────────────────────────

fn register_extended_tools(registry: &mut ToolRegistry) {
    use agent_runtime::tools::ToolSpec;

    let extras: &[(&str, &str, serde_json::Value, serde_json::Value)] = &[
        (
            "aspirate",
            "Aspirate (draw up) liquid from a vessel.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vessel_id": { "type": "string" },
                    "volume_ul": { "type": "number" }
                },
                "required": ["vessel_id", "volume_ul"]
            }),
            serde_json::json!({ "status": "aspirated" }),
        ),
        (
            "transfer",
            "Transfer liquid from one vessel to another.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to":   { "type": "string" },
                    "volume_ul": { "type": "number" }
                },
                "required": ["from", "to", "volume_ul"]
            }),
            serde_json::json!({ "status": "transferred" }),
        ),
        (
            "mix",
            "Mix the contents of a vessel at the specified RPM.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vessel_id": { "type": "string" },
                    "rpm":       { "type": "number" }
                },
                "required": ["vessel_id", "rpm"]
            }),
            serde_json::json!({ "status": "mixed" }),
        ),
        (
            "grip",
            "Close the arm gripper around a piece of labware.",
            serde_json::json!({
                "type": "object",
                "properties": { "target": { "type": "string" } },
                "required": ["target"]
            }),
            serde_json::json!({ "status": "gripped" }),
        ),
        (
            "centrifuge",
            "Spin a vessel in the centrifuge.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vessel_id":  { "type": "string" },
                    "rpm":        { "type": "number" },
                    "duration_s": { "type": "number" }
                },
                "required": ["vessel_id", "rpm", "duration_s"]
            }),
            serde_json::json!({ "status": "centrifuged" }),
        ),
        (
            "read_absorbance",
            "Measure UV/Vis absorbance through a vessel at a given wavelength.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vessel_id":    { "type": "string" },
                    "wavelength_nm": { "type": "number" }
                },
                "required": ["vessel_id", "wavelength_nm"]
            }),
            serde_json::json!({ "absorbance": 0.847, "wavelength_nm": 595, "unit": "AU" }),
        ),
        (
            "read_ph",
            "Read the pH of the contents of a vessel.",
            serde_json::json!({
                "type": "object",
                "properties": { "vessel_id": { "type": "string" } },
                "required": ["vessel_id"]
            }),
            serde_json::json!({ "ph": 7.2, "unit": "pH" }),
        ),
        (
            "read_temperature",
            "Read the temperature of a vessel in milli-Kelvins.",
            serde_json::json!({
                "type": "object",
                "properties": { "vessel_id": { "type": "string" } },
                "required": ["vessel_id"]
            }),
            serde_json::json!({ "temperature_mk": 298150, "unit": "mK" }),
        ),
        (
            "set_temperature",
            "Set the target temperature of a hot plate or thermal controller.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vessel_id": { "type": "string" },
                    "target_mk": { "type": "number" }
                },
                "required": ["vessel_id", "target_mk"]
            }),
            serde_json::json!({ "status": "temperature_set" }),
        ),
        (
            "set_pressure",
            "Set the target pressure in a sealed chamber (Pascals).",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "chamber_id": { "type": "string" },
                    "target_pa":  { "type": "number" }
                },
                "required": ["chamber_id", "target_pa"]
            }),
            serde_json::json!({ "status": "pressure_set" }),
        ),
        (
            "set_stir_rate",
            "Set the stir plate RPM for a vessel.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "vessel_id": { "type": "string" },
                    "rpm":       { "type": "number" }
                },
                "required": ["vessel_id", "rpm"]
            }),
            serde_json::json!({ "status": "stir_rate_set" }),
        ),
    ];

    for (name, desc, schema, result) in extras {
        let result = result.clone();
        registry.register(
            ToolSpec {
                name:              (*name).to_owned(),
                description:       (*desc).to_owned(),
                parameters_schema: schema.clone(),
            },
            Box::new(move |_params| {
                let r = result.clone();
                Box::pin(async move { Ok(r) })
            }),
        );
    }
}
