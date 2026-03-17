mod simulator;
mod ws_sink;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::broadcast;
use tower_http::{cors::CorsLayer, services::ServeDir};
use ws_sink::{EventBuffer, ExplorationLog};

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    tx:        broadcast::Sender<String>,
    running:   Arc<AtomicBool>,
    iteration: Arc<AtomicU32>,
    notebook:  Arc<Mutex<Vec<serde_json::Value>>>,
    log:       Arc<Mutex<ExplorationLog>>,
    events:    EventBuffer,
}

// ── Routes ────────────────────────────────────────────────────────────────────

/// In-memory event history — used by the visualizer on load.
async fn history_handler(State(s): State<AppState>) -> impl IntoResponse {
    let (notebook, transitions, tools) = s.events.snapshot();
    axum::Json(serde_json::json!({
        "notebook":    notebook,
        "transitions": transitions,
        "tools":       tools,
    }))
}

async fn status_handler(State(s): State<AppState>) -> impl IntoResponse {
    let notebook = s.notebook.lock().unwrap().clone();
    axum::Json(serde_json::json!({
        "running":   s.running.load(Ordering::SeqCst),
        "iteration": s.iteration.load(Ordering::SeqCst),
        "notebook":  notebook,
    }))
}

// ── WebSocket ─────────────────────────────────────────────────────────────────

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(s): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, s))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    // Send current state snapshot to the new viewer immediately.
    let snapshot = serde_json::json!({
        "event": "snapshot",
        "payload": {
            "running":   state.running.load(Ordering::SeqCst),
            "iteration": state.iteration.load(Ordering::SeqCst),
            "notebook":  *state.notebook.lock().unwrap(),
        }
    });
    if socket.send(Message::Text(snapshot.to_string())).await.is_err() {
        return;
    }

    let mut rx = state.tx.subscribe();

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        if socket.send(Message::Text(text)).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS client lagged {n} messages");
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axiomlab_server=info,tower_http=info".into()),
        )
        .init();

    let (tx, _) = broadcast::channel::<String>(512);
    let events = EventBuffer::default();

    let state = AppState {
        tx,
        running:   Arc::new(AtomicBool::new(false)),
        iteration: Arc::new(AtomicU32::new(0)),
        notebook:  Arc::new(Mutex::new(Vec::new())),
        log:       Arc::new(Mutex::new(ExplorationLog::default())),
        events:    events.clone(),
    };

    // Auto-start the exploration loop immediately on server launch.
    {
        let sink = Arc::new(ws_sink::WebSocketSink {
            tx:       state.tx.clone(),
            log:      Arc::clone(&state.log),
            notebook: Arc::clone(&state.notebook),
            events,
        });
        state.running.store(true, Ordering::SeqCst);
        let running   = Arc::clone(&state.running);
        let iteration = Arc::clone(&state.iteration);
        tokio::spawn(async move {
            simulator::run_loop(sink, running.clone(), iteration).await;
            running.store(false, Ordering::SeqCst);
        });
    }

    // Static file serving — serves the built React app from ../visualizer/dist.
    let static_files = ServeDir::new("../visualizer/dist")
        .append_index_html_on_directories(true);

    let app = Router::new()
        .route("/ws",          get(ws_handler))
        .route("/api/status",  get(status_handler))
        .route("/api/history", get(history_handler))
        .fallback_service(static_files)
        .layer(CorsLayer::permissive())
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("AxiomLab server listening on http://{addr}");
    tracing::info!("Public viewing window: open http://{addr} in any browser");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
