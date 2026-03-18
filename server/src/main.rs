mod approvals;
mod audit_query;
mod discovery;
mod simulator;
mod stall;
mod ws_sink;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use agent_runtime::approval_queue::PendingApprovalQueue;
use agent_runtime::audit::{
    anchor_chain_tip_to_rekor, audit_log_path, audit_signer_from_env, emit_session_start, rotate_if_needed,
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
    tx:              broadcast::Sender<String>,
    running:         Arc<AtomicBool>,
    /// Set to `true` on startup when a stalled approval sidecar is found without
    /// a `dispatch_complete` audit entry.  The exploration loop polls this flag
    /// and pauses until cleared by an operator recovery action.
    pub stalled:     Arc<AtomicBool>,
    iteration:       Arc<AtomicU32>,
    notebook:        Arc<Mutex<Vec<serde_json::Value>>>,
    log:             Arc<Mutex<ExplorationLog>>,
    events:          EventBuffer,
    journal:         Arc<Mutex<DiscoveryJournal>>,
    approval_queue:  Arc<PendingApprovalQueue>,
    audit_log_path:  String,
    /// IDs of approvals that stalled on the previous run (no dispatch_complete).
    pub stalled_ids: Arc<Mutex<Vec<String>>>,
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

// ── Recovery: cancel a stalled dispatch ───────────────────────────────────────

async fn recovery_cancel_handler(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let signer = agent_runtime::audit::audit_signer_from_env();
    agent_runtime::audit::emit_dispatch_cancelled(
        &s.audit_log_path,
        &id,
        signer.as_deref(),
    ).ok();
    // Delete the sidecar and remove from the stalled list.
    s.approval_queue.purge_sidecar(&id);
    {
        let mut ids = s.stalled_ids.lock().unwrap();
        ids.retain(|x| x != &id);
        if ids.is_empty() {
            s.stalled.store(false, Ordering::SeqCst);
            tracing::info!("All stalled dispatches cleared — exploration loop unblocked");
        }
    }
    axum::Json(serde_json::json!({
        "status": "cancelled",
        "approval_id": id,
    }))
}

/// Resume a stalled dispatch by clearing the stall block and letting the
/// exploration loop re-propose the action organically on the next iteration.
///
/// The operator is expected to have verified the intent of the stalled dispatch.
/// This handler does NOT re-dispatch the tool call directly — it simply unblocks
/// the loop so a new LLM iteration can propose the action again if appropriate.
async fn recovery_resume_handler(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    // Purge sidecar and remove from stalled list (marks it as operator-reviewed).
    s.approval_queue.purge_sidecar(&id);
    {
        let mut ids = s.stalled_ids.lock().unwrap();
        ids.retain(|x| x != &id);
        if ids.is_empty() {
            s.stalled.store(false, Ordering::SeqCst);
            tracing::info!("Stall cleared by operator resume — exploration loop unblocked");
        }
    }
    axum::Json(serde_json::json!({
        "status": "cleared",
        "approval_id": id,
        "note": "Stall cleared. The exploration loop will resume; \
                 the LLM may re-propose this action on the next iteration.",
    }))
}

// ── GET /api/approvals/stalled ────────────────────────────────────────────────

async fn stalled_handler(State(s): State<AppState>) -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "stalled": s.stalled.load(Ordering::SeqCst),
        "approval_ids": *s.stalled_ids.lock().unwrap(),
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
    let audit_signer   = audit_signer_from_env();
    let pubkey_display = audit_signer
        .as_deref()
        .map(|s| s.public_key_b64())
        .unwrap_or_else(|| "unsigned".to_string());
    emit_session_start(
        &audit_path_str,
        &session_id,
        &pubkey_display,
        &git_commit,
        audit_signer.as_deref(),
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
                anchor_chain_tip_to_rekor(&path_for_rekor, signer.as_ref()).await;
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

    // Detect stalled approval sidecars from a previous (crashed) run.
    let stall_signer = audit_signer_from_env();
    let stalled_ids = stall::detect_stalled_approvals(
        &audit_path_str,
        stall_signer.as_deref(),
    );
    let is_stalled = !stalled_ids.is_empty();
    if is_stalled {
        tracing::warn!(
            count = stalled_ids.len(),
            ids   = ?stalled_ids,
            "Stalled dispatch(es) detected — exploration loop BLOCKED until operator resolves.\n\
             To cancel: POST /api/approvals/recover/<id>/cancel\n\
             To clear:  POST /api/approvals/recover/<id>"
        );
    }

    let state = AppState {
        tx,
        running:        Arc::new(AtomicBool::new(false)),
        stalled:        Arc::new(AtomicBool::new(is_stalled)),
        iteration:      Arc::new(AtomicU32::new(0)),
        notebook:       Arc::new(Mutex::new(Vec::new())),
        log:            Arc::new(Mutex::new(ExplorationLog::from_journal(&journal.lock().unwrap()))),
        events:         events.clone(),
        journal:        Arc::clone(&journal),
        approval_queue: Arc::clone(&approval_queue),
        audit_log_path: audit_path_str.clone(),
        stalled_ids:    Arc::new(Mutex::new(stalled_ids)),
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
        let stalled   = Arc::clone(&state.stalled);
        let iteration = Arc::clone(&state.iteration);
        tokio::spawn(async move {
            simulator::run_loop(sink, running.clone(), stalled, iteration, approval_queue).await;
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
        .route("/ws",                              get(ws_handler))
        .route("/api/status",                      get(status_handler))
        .route("/api/history",                     get(history_handler))
        .route("/api/journal",                     get(journal_handler))
        .route("/api/journal/findings",            get(findings_handler))
        .route("/api/audit",                       get(audit_query::audit_query_handler))
        .route("/api/audit/verify",                get(audit_query::audit_verify_handler))
        .route("/api/audit/raw",                   get(audit_query::audit_raw_handler))
        .route("/api/approvals/stalled",           get(stalled_handler))
        .route("/api/approvals/recover/{id}",       post(recovery_resume_handler))
        .route("/api/approvals/recover/{id}/cancel", post(recovery_cancel_handler))
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
