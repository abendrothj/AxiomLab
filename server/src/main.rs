mod approvals;
mod audit_query;
mod discovery;
mod simulator;
mod ws_sink;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use agent_runtime::approval_queue::PendingApprovalQueue;
use agent_runtime::audit::{
    anchor_chain_tip_to_rekor, audit_log_path, emit_session_start, rotate_if_needed, AuditSigner,
};
use discovery::{journal_path, DiscoveryJournal};
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
pub(crate) struct AppState {
    tx:             broadcast::Sender<String>,
    running:        Arc<AtomicBool>,
    iteration:      Arc<AtomicU32>,
    notebook:       Arc<Mutex<Vec<serde_json::Value>>>,
    log:            Arc<Mutex<ExplorationLog>>,
    events:         EventBuffer,
    journal:        Arc<Mutex<DiscoveryJournal>>,
    approval_queue: Arc<PendingApprovalQueue>,
    audit_log_path: String,
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

/// The persistent discovery journal — findings, hypotheses, run history.
async fn journal_handler(State(s): State<AppState>) -> impl IntoResponse {
    let journal = s.journal.lock().unwrap().clone();
    axum::Json(journal)
}

/// `GET /api/journal/findings` — return only the findings array.
async fn findings_handler(State(s): State<AppState>) -> impl IntoResponse {
    let findings = s.journal.lock().unwrap().findings.clone();
    axum::Json(findings)
}

/// Warn about (and remove) stale approval sidecar files left by a previous crash.
///
/// Any `.json` file in `.artifacts/approvals/` at startup indicates an approval
/// that was queued but never resolved — the operator should be notified.
fn scan_stale_approvals() {
    let dir = agent_runtime::audit::data_dir().join("approvals");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        tracing::warn!(
            path = %path.display(),
            "Stale approval sidecar found — was the server interrupted mid-approval? \
             The pending action was never resolved. Removing sidecar."
        );
        std::fs::remove_file(&path).ok();
    }
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
    let events  = EventBuffer::default();

    // ── Audit log setup ───────────────────────────────────────────────────────
    let audit_path = audit_log_path();
    rotate_if_needed(&audit_path).unwrap_or_else(|e| {
        tracing::warn!("Audit log rotation failed: {e}");
        None
    });
    let audit_path_str = audit_path.to_string_lossy().into_owned();
    let session_id     = uuid::Uuid::new_v4().to_string();
    let git_commit     = std::env::var("AXIOMLAB_GIT_COMMIT").unwrap_or_else(|_| "dev".into());
    let audit_signer   = AuditSigner::from_env();
    emit_session_start(
        &audit_path_str,
        &session_id,
        audit_signer.as_ref().map(|s| s.public_key_b64()).unwrap_or("unsigned"),
        &git_commit,
        audit_signer.as_ref(),
    ).unwrap_or_else(|e| {
        tracing::warn!("Failed to write session_start audit entry: {e}");
        String::new()
    });
    tracing::info!(
        path  = %audit_path_str,
        session = %session_id,
        "Audit log ready"
    );

    // Periodic Rekor checkpoint — anchor the chain tip every 15 minutes.
    if let Some(signer) = audit_signer {
        let path_for_rekor = audit_path_str.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                tokio::time::Duration::from_secs(15 * 60)
            );
            interval.tick().await; // skip the immediate first tick
            loop {
                interval.tick().await;
                anchor_chain_tip_to_rekor(&path_for_rekor, &signer).await;
            }
        });
    } else {
        tracing::warn!(
            "No AXIOMLAB_AUDIT_SIGNING_KEY set — audit entries will be unsigned \
             and Rekor checkpointing is disabled. Set the key for production use."
        );
    }

    // Load the persistent discovery journal (or start fresh).
    let path    = journal_path();
    let journal = Arc::new(Mutex::new(DiscoveryJournal::load(&path)));
    {
        let j = journal.lock().unwrap();
        tracing::info!(
            runs = j.runs.len(),
            findings = j.findings.len(),
            hypotheses = j.hypotheses.len(),
            "Discovery journal loaded"
        );
    }

    let approval_queue = PendingApprovalQueue::new();

    // Scan for stale approval sidecars from a previous (crashed) run.
    scan_stale_approvals();

    let state = AppState {
        tx,
        running:        Arc::new(AtomicBool::new(false)),
        iteration:      Arc::new(AtomicU32::new(0)),
        notebook:       Arc::new(Mutex::new(Vec::new())),
        log:            Arc::new(Mutex::new(ExplorationLog::from_journal(&journal.lock().unwrap()))),
        events:         events.clone(),
        journal:        Arc::clone(&journal),
        approval_queue: Arc::clone(&approval_queue),
        audit_log_path: audit_path_str.clone(),
    };

    // Auto-start the exploration loop immediately on server launch.
    {
        let sink = Arc::new(ws_sink::WebSocketSink {
            tx:       state.tx.clone(),
            log:      Arc::clone(&state.log),
            notebook: Arc::clone(&state.notebook),
            events,
            journal:  Arc::clone(&journal),
        });
        state.running.store(true, Ordering::SeqCst);
        let running   = Arc::clone(&state.running);
        let iteration = Arc::clone(&state.iteration);
        tokio::spawn(async move {
            simulator::run_loop(sink, running.clone(), iteration, approval_queue).await;
            running.store(false, Ordering::SeqCst);
        });
    }

    // Static file serving — serves the built React app from ../visualizer/dist.
    let static_files = ServeDir::new("../visualizer/dist")
        .append_index_html_on_directories(true);

    let approvals_router = Router::new()
        .route("/api/approvals/pending", get(approvals::pending_handler))
        .route("/api/approvals/submit",  post(approvals::submit_handler))
        .with_state(Arc::clone(&state.approval_queue));

    let app = Router::new()
        .route("/ws",                    get(ws_handler))
        .route("/api/status",            get(status_handler))
        .route("/api/history",           get(history_handler))
        .route("/api/journal",           get(journal_handler))
        .route("/api/journal/findings",  get(findings_handler))
        .route("/api/audit",             get(audit_query::audit_query_handler))
        .route("/api/audit/verify",      get(audit_query::audit_verify_handler))
        .merge(approvals_router)
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
