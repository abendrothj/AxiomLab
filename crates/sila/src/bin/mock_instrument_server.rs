//! Standalone mock instrument server (instruments.proto, SimLab-backed).
//!
//! Run it, then point the system at it for a real end-to-end gRPC run:
//!   cargo run -p axiom-sila --bin mock-instrument-server      # listens on :50051
//!   AXIOMLAB_SILA_ENDPOINT=http://127.0.0.1:50051 cargo run -p axiomlab-server

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("axiom_sila=info").init();
    let addr = std::env::var("AXIOMLAB_SILA_BIND")
        .unwrap_or_else(|_| "127.0.0.1:50051".into())
        .parse()
        .expect("invalid AXIOMLAB_SILA_BIND address");
    if let Err(e) = axiom_sila::serve_mock(addr).await {
        eprintln!("mock instrument server failed: {e}");
        std::process::exit(1);
    }
}
