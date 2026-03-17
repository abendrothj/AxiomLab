mod event_sink;
mod simulator;

use std::sync::atomic::AtomicBool;

/// Shared runtime state — injected via Tauri's managed state system.
pub struct SimState {
    /// Set to false to signal the exploration loop to stop after the current experiment.
    pub running: AtomicBool,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(SimState {
            running: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            simulator::start_simulation,
            simulator::stop_simulation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AxiomLab Visualizer");
}
