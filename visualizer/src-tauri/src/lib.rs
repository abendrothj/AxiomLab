mod event_sink;
mod simulator;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![simulator::start_simulation])
        .run(tauri::generate_context!())
        .expect("error while running AxiomLab Visualizer");
}
