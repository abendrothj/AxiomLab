use crate::event_sink::TauriEventSink;
use agent_runtime::{
    capabilities::CapabilityPolicy,
    experiment::Experiment,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
    sandbox::{ResourceLimits, Sandbox},
    tools::ToolRegistry,
};
use std::{path::PathBuf, sync::Arc};
use tauri::{AppHandle, command};

/// The behavioral mandate given to the AI.
///
/// No specific goal is provided — only a mandate to explore freely and document
/// what it discovers. Verus capability bounds generate the drama organically.
const SANDBOX_MANDATE: &str = "\
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

/// Start the autonomous simulation.
///
/// Spawns a detached Tokio task that runs the real orchestrator. Returns
/// immediately — progress arrives via Tauri events (`state_transition`,
/// `tool_execution`, `llm_token`, `notebook_entry`).
#[command]
pub async fn start_simulation(app: AppHandle) -> Result<(), String> {
    let llm = OpenAiClient::from_env().map_err(|e| e.to_string())?;
    let sink = Arc::new(TauriEventSink { app });

    tokio::spawn(async move {
        // Allow all registered lab tools through the sandbox.
        let sandbox = Sandbox::new(
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
        );

        let mut tools = ToolRegistry::new();
        agent_runtime::tools::register_lab_tools(&mut tools);
        register_extended_tools(&mut tools);

        let config = OrchestratorConfig {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
            capability_policy: Some(CapabilityPolicy::default_lab()),
            revocation_list: RevocationList::default(),
            event_sink: Some(sink),
            ..OrchestratorConfig::default()
        };

        let orchestrator = Orchestrator::new(llm, sandbox, tools, config);
        let mut experiment = Experiment::new(
            format!("exp-{}", uuid::Uuid::new_v4()),
            SANDBOX_MANDATE,
        );

        if let Err(e) = orchestrator.run_experiment(&mut experiment).await {
            eprintln!("simulation ended: {e}");
        }
    });

    Ok(())
}

/// Register additional lab tools beyond the three built-in ones.
///
/// These return synthetic stub results — suitable for the visualizer where
/// the AI's interactions with hardware are simulated.
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
                    "to": { "type": "string" },
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
                    "rpm": { "type": "number" }
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
                    "vessel_id": { "type": "string" },
                    "rpm": { "type": "number" },
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
                    "vessel_id": { "type": "string" },
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
                    "target_pa": { "type": "number" }
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
                    "rpm": { "type": "number" }
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
                name: (*name).to_owned(),
                description: (*desc).to_owned(),
                parameters_schema: schema.clone(),
            },
            Box::new(move |_params| {
                let r = result.clone();
                Box::pin(async move { Ok(r) })
            }),
        );
    }
}
