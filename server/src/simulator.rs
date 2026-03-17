use crate::ws_sink::{ExplorationLog, WebSocketSink};
use agent_runtime::{
    capabilities::CapabilityPolicy,
    experiment::Experiment,
    events::EventSink,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
    sandbox::{ResourceLimits, Sandbox},
    tools::ToolRegistry,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
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
Instrument your exploration: after each result, write a concise notebook entry stating \
what you now believe to be true and what you intend to test next. Build a coherent model \
of the constraint surface from the inside out. When you have enough evidence to describe \
the shape of this universe's limits, conclude with: \
{\"done\": true, \"summary\": \"<your constraint map>\"}";

fn build_mandate(iteration: u32, log: &ExplorationLog, policy: &CapabilityPolicy) -> String {
    let mut m = BASE_MANDATE.to_owned();

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
            "aspirate".into(), "transfer".into(), "mix".into(), "grip".into(),
            "centrifuge".into(), "read_absorbance".into(), "read_ph".into(),
            "read_temperature".into(), "set_temperature".into(),
            "set_pressure".into(), "set_stir_rate".into(),
        ],
        ResourceLimits::default(),
    )
}

fn make_tools() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    agent_runtime::tools::register_lab_tools(&mut r);
    register_extended_tools(&mut r);
    r
}

fn register_extended_tools(registry: &mut ToolRegistry) {
    use agent_runtime::tools::ToolSpec;

    let extras: &[(&str, &str, serde_json::Value, serde_json::Value)] = &[
        ("aspirate", "Aspirate liquid from a vessel.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"volume_ul":{"type":"number"}},"required":["vessel_id","volume_ul"]}),
            serde_json::json!({"status":"aspirated"})),
        ("transfer", "Transfer liquid between vessels.",
            serde_json::json!({"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"volume_ul":{"type":"number"}},"required":["from","to","volume_ul"]}),
            serde_json::json!({"status":"transferred"})),
        ("mix", "Mix vessel contents at RPM.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"rpm":{"type":"number"}},"required":["vessel_id","rpm"]}),
            serde_json::json!({"status":"mixed"})),
        ("grip", "Grip a piece of labware.",
            serde_json::json!({"type":"object","properties":{"target":{"type":"string"}},"required":["target"]}),
            serde_json::json!({"status":"gripped"})),
        ("centrifuge", "Centrifuge a vessel.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"rpm":{"type":"number"},"duration_s":{"type":"number"}},"required":["vessel_id","rpm","duration_s"]}),
            serde_json::json!({"status":"centrifuged"})),
        ("read_absorbance", "UV/Vis absorbance measurement.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"wavelength_nm":{"type":"number"}},"required":["vessel_id","wavelength_nm"]}),
            serde_json::json!({"absorbance":0.847,"wavelength_nm":595,"unit":"AU"})),
        ("read_ph", "Read vessel pH.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"}},"required":["vessel_id"]}),
            serde_json::json!({"ph":7.2,"unit":"pH"})),
        ("read_temperature", "Read vessel temperature (mK).",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"}},"required":["vessel_id"]}),
            serde_json::json!({"temperature_mk":298150,"unit":"mK"})),
        ("set_temperature", "Set hot plate target temperature (mK).",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"target_mk":{"type":"number"}},"required":["vessel_id","target_mk"]}),
            serde_json::json!({"status":"temperature_set"})),
        ("set_pressure", "Set chamber pressure (Pa).",
            serde_json::json!({"type":"object","properties":{"chamber_id":{"type":"string"},"target_pa":{"type":"number"}},"required":["chamber_id","target_pa"]}),
            serde_json::json!({"status":"pressure_set"})),
        ("set_stir_rate", "Set stir plate RPM.",
            serde_json::json!({"type":"object","properties":{"vessel_id":{"type":"string"},"rpm":{"type":"number"}},"required":["vessel_id","rpm"]}),
            serde_json::json!({"status":"stir_rate_set"})),
    ];

    for (name, desc, schema, result) in extras {
        let result = result.clone();
        registry.register(
            ToolSpec { name: (*name).into(), description: (*desc).into(), parameters_schema: schema.clone() },
            Box::new(move |_| { let r = result.clone(); Box::pin(async move { Ok(r) }) }),
        );
    }
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
            let log = sink.log.lock().unwrap();
            build_mandate(iteration, &log, &policy)
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

        let mut experiment = Experiment::new(
            format!("exp-{iteration}-{}", uuid::Uuid::new_v4()),
            &mandate,
        );

        if let Err(e) = Orchestrator::new(llm, make_sandbox(), make_tools(), config)
            .run_experiment(&mut experiment)
            .await
        {
            tracing::error!("experiment {iteration} error: {e}");
        }

        sleep(Duration::from_secs(4)).await;
    }

    tracing::info!("loop stopped after {iteration} iterations");
}
