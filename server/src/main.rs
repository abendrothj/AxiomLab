mod approvals;
mod approvals_ui;
mod audit_query;
mod auth;
mod db;
mod discovery;
mod protocol_queue;
mod simulator;
mod ws_sink;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Json, Path, Query, State,
    },
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use agent_runtime::approval_queue::PendingApprovalQueue;
use agent_runtime::hypothesis::HypothesisManager;
use agent_runtime::audit::{
    audit_log_path, audit_signer_from_env, emit_emergency_stop,
    emit_session_start, rotate_if_needed,
};
use agent_runtime::hardware::SiLA2Clients;
use agent_runtime::lab_state::{LabState, Reagent};
use discovery::{journal_path, DiscoveryJournal};
use protocol_queue::ProtocolQueue;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::broadcast;
use tower_http::{cors::CorsLayer, services::ServeDir};
use ws_sink::{EventBuffer, ExecutionLog};
use metrics_exporter_prometheus::PrometheusHandle;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct AppState {
    tx:              broadcast::Sender<String>,
    running:         Arc<AtomicBool>,
    iteration:       Arc<AtomicU32>,
    notebook:        Arc<Mutex<Vec<serde_json::Value>>>,
    log:             Arc<Mutex<ExecutionLog>>,
    events:          EventBuffer,
    journal:         Arc<Mutex<DiscoveryJournal>>,
    /// SQLite persistence — dual-write target for all journal mutations.
    pub db:          Arc<db::Db>,
    approval_queue:  Arc<PendingApprovalQueue>,
    audit_log_path:  String,
    /// SiLA 2 hardware clients — `None` when running in simulator mode.
    sila_clients: Option<Arc<SiLA2Clients>>,
    /// Reagent inventory and vessel contents.
    pub lab_state: Arc<Mutex<LabState>>,
    /// Rich hypothesis state machine — shared with the orchestrator.
    pub hypothesis_manager: Arc<Mutex<HypothesisManager>>,
    /// Operator protocol queue — the primary interface for directing lab execution.
    pub protocol_queue: Arc<Mutex<ProtocolQueue>>,
    /// Live execution-loop pacing status (wait-until-next-experiment, etc.).
    loop_status: Arc<Mutex<ws_sink::LoopStatus>>,
    /// Prometheus metrics handle — rendered by GET /metrics.
    metrics_handle: PrometheusHandle,
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
    let slot_count = std::env::var("AXIOMLAB_EXPERIMENT_SLOTS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 4);
    let loop_status = s.loop_status.lock().unwrap().clone();
    let (queue_pending, queue_running) = {
        let q = s.protocol_queue.lock().unwrap();
        let pending = q.items().iter().filter(|i| i.status == protocol_queue::QueueStatus::Pending).count();
        let running = q.items().iter().filter(|i| i.status == protocol_queue::QueueStatus::Running).count();
        (pending, running)
    };
    let hardware_mode = s.sila_clients.is_some();
    let agenda_complete = {
        let j = s.journal.lock().unwrap();
        let items = simulator::agenda_status(&j);
        items.iter().all(|i| i.status == "completed")
    };
    axum::Json(serde_json::json!({
        "running":          s.running.load(Ordering::SeqCst),
        "iteration":        s.iteration.load(Ordering::SeqCst),
        "notebook":         notebook,
        "slot_count":       slot_count,
        "loop_status":      loop_status,
        "queue_pending":    queue_pending,
        "queue_running":    queue_running,
        "hardware_mode":    hardware_mode,
        "agenda_complete":  agenda_complete,
    }))
}

/// `GET /health` — liveness probe; always 200 while the process is alive.
async fn health_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

/// `GET /ready` — readiness probe; 200 only when the SQLite DB is reachable.
async fn ready_handler(State(s): State<AppState>) -> impl IntoResponse {
    if s.db.ping() {
        (StatusCode::OK, axum::Json(serde_json::json!({ "status": "ready" })))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({ "status": "not_ready", "db": false })),
        )
    }
}

/// `GET /metrics` — Prometheus text exposition format.
async fn metrics_handler(State(s): State<AppState>) -> impl IntoResponse {
    s.metrics_handle.render()
}

/// The persistent operation log — findings, directives, run history.
async fn journal_handler(State(s): State<AppState>) -> impl IntoResponse {
    let journal = s.journal.lock().unwrap().clone();
    axum::Json(journal)
}

/// `GET /api/journal/findings` — return only the findings array.
async fn findings_handler(State(s): State<AppState>) -> impl IntoResponse {
    let findings = s.journal.lock().unwrap().findings.clone();
    axum::Json(findings)
}

// ── Lab inventory routes ──────────────────────────────────────────────────────

async fn lab_reagents_handler(State(s): State<AppState>) -> impl IntoResponse {
    let reagents: Vec<_> = s.lab_state.lock().unwrap().reagents.values().cloned().collect();
    axum::Json(reagents)
}

async fn lab_register_reagent_handler(
    State(s): State<AppState>,
    Json(reagent): Json<Reagent>,
) -> impl IntoResponse {
    let id = reagent.id.clone();
    {
        let mut ls = s.lab_state.lock().unwrap();
        ls.register_reagent(reagent);
        ls.save();
    }
    (StatusCode::CREATED, axum::Json(serde_json::json!({"status": "registered", "id": id})))
}

async fn lab_remove_reagent_handler(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let removed = {
        let mut ls = s.lab_state.lock().unwrap();
        let r = ls.remove_reagent(&id);
        if r.is_some() { ls.save(); }
        r
    };
    match removed {
        Some(_) => axum::Json(serde_json::json!({"status": "removed", "id": id})).into_response(),
        None    => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error": "not found"}))).into_response(),
    }
}

async fn lab_set_vessel_contents_handler(
    Path(vessel_id): Path<String>,
    State(s): State<AppState>,
    Json(reagent_ids): Json<Vec<String>>,
) -> impl IntoResponse {
    use agent_runtime::lab_state::VesselContribution;
    let contribs: Vec<VesselContribution> = reagent_ids.iter().map(|id| VesselContribution {
        reagent_id: id.clone(),
        volume_ul: 0.0,
        concentration_m: 0.0,
    }).collect();
    let mut ls = s.lab_state.lock().unwrap();
    ls.set_vessel_contents(&vessel_id, contribs);
    ls.save();
    axum::Json(serde_json::json!({"vessel_id": vessel_id, "contents": reagent_ids}))
}

async fn lab_vessels_handler(State(s): State<AppState>) -> impl IntoResponse {
    let contents = s.lab_state.lock().unwrap().vessel_contents.clone();
    axum::Json(contents)
}

async fn lab_calibration_status_handler(State(s): State<AppState>) -> impl IntoResponse {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let journal = s.journal.lock().unwrap();
    // Known quantitative instruments that require valid calibration.
    let instruments = ["ph_meter", "spectrophotometer", "centrifuge", "incubator"];
    let statuses: Vec<serde_json::Value> = instruments.iter().map(|inst| {
        let rec = journal.last_calibration_for(inst);
        serde_json::json!({
            "instrument":        inst,
            "calibrated":        rec.is_some(),
            "valid":             rec.map(|r| r.is_valid_at(now_secs)).unwrap_or(false),
            "performed_at_secs": rec.map(|r| r.performed_at_secs),
            "valid_until_secs":  rec.and_then(|r| r.valid_until_secs),
        })
    }).collect();
    axum::Json(serde_json::json!({"calibration_status": statuses, "checked_at_secs": now_secs}))
}

// ── Protocol queue routes ─────────────────────────────────────────────────────

/// `GET /api/queue` — list all queue items (pending + recent history).
async fn queue_list_handler(State(s): State<AppState>) -> impl IntoResponse {
    let items = s.protocol_queue.lock().unwrap().items().to_vec();
    axum::Json(serde_json::json!({ "items": items }))
}

#[derive(serde::Deserialize)]
struct EnqueueRequest {
    statement: String,
    #[serde(default)]
    priority:  u8,
}

/// `POST /api/queue` — push a new protocol directive onto the queue.
async fn queue_enqueue_handler(
    State(s): State<AppState>,
    Json(body): Json<EnqueueRequest>,
) -> impl IntoResponse {
    if body.statement.trim().is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({"error": "statement must not be empty"})),
        ).into_response();
    }
    let id = s.protocol_queue.lock().unwrap().enqueue(body.statement, body.priority);
    tracing::info!(queue_id = %id, priority = body.priority, "Protocol queued by operator");
    (StatusCode::CREATED, axum::Json(serde_json::json!({"id": id}))).into_response()
}

/// `DELETE /api/queue/:id` — remove a queued item (any status).
async fn queue_remove_handler(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let removed = s.protocol_queue.lock().unwrap().remove(&id);
    if removed {
        axum::Json(serde_json::json!({"status": "removed", "id": id})).into_response()
    } else {
        (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error": "not found"}))).into_response()
    }
}

// ── GET /api/agenda ───────────────────────────────────────────────────────────

/// `GET /api/agenda` — return the commissioning agenda with live completion status.
async fn agenda_handler(State(s): State<AppState>) -> impl IntoResponse {
    let journal = s.journal.lock().unwrap();
    let items   = simulator::agenda_status(&journal);
    let completed = items.iter().filter(|i| i.status == "completed").count();
    let total     = items.len();
    axum::Json(serde_json::json!({
        "items":           items,
        "completed_count": completed,
        "total_count":     total,
    }))
}

// ── POST /api/emergency-stop ──────────────────────────────────────────────────

async fn emergency_stop_handler(State(s): State<AppState>) -> impl IntoResponse {
    // 1. Halt the execution loop immediately.
    s.running.store(false, Ordering::SeqCst);

    // 2. Send hardware abort to all SiLA 2 instruments (if connected).
    let instrument_results: Vec<serde_json::Value> = if let Some(clients) = &s.sila_clients {
        clients
            .abort_all()
            .await
            .into_iter()
            .map(|(name, result)| {
                serde_json::json!({
                    "instrument": name,
                    "ok": result.is_ok(),
                    "error": result.err(),
                })
            })
            .collect()
    } else {
        vec![serde_json::json!({
            "instrument": "all",
            "ok": true,
            "note": "simulator mode — no hardware to abort"
        })]
    };

    // 3. Write emergency_stop audit event.
    let signer = agent_runtime::audit::audit_signer_from_env();
    emit_emergency_stop(&s.audit_log_path, "operator", signer.as_deref()).unwrap_or_else(|e| {
        tracing::warn!("Failed to write emergency_stop audit event: {e}");
        String::new()
    });

    tracing::warn!("EMERGENCY STOP triggered — execution loop halted");

    axum::Json(serde_json::json!({
        "status": "stopped",
        "instrument_results": instrument_results,
    }))
}

// ── WebSocket ─────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct WsQuery {
    #[serde(default)]
    token: Option<String>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(s): State<AppState>,
    Query(q): Query<WsQuery>,
) -> impl IntoResponse {
    // When AXIOMLAB_WS_AUTH=0 (or JWT_SECRET unset), allow unauthenticated connections.
    let ws_auth_enabled = std::env::var("AXIOMLAB_WS_AUTH").as_deref() != Ok("0")
        && auth::jwt_secret_from_env().is_some();

    if ws_auth_enabled {
        let token = match &q.token {
            Some(t) => t.as_str(),
            None => {
                tracing::warn!("WebSocket connection rejected — no token query param");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        };
        if let Err(e) = auth::validate_jwt(token) {
            tracing::warn!("WebSocket connection rejected — invalid JWT: {e}");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_ws(socket, s)).into_response()
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    // Send current state snapshot to the new viewer immediately.
    let queue_pending_snap = state.protocol_queue.lock().unwrap()
        .items().iter()
        .filter(|i| i.status == protocol_queue::QueueStatus::Pending)
        .count();
    let agenda_complete_snap = {
        let j = state.journal.lock().unwrap();
        simulator::agenda_status(&j).iter().all(|i| i.status == "completed")
    };
    let snapshot = serde_json::json!({
        "event": "snapshot",
        "payload": {
            "running":          state.running.load(Ordering::SeqCst),
            "iteration":        state.iteration.load(Ordering::SeqCst),
            "notebook":         *state.notebook.lock().unwrap(),
            "loop_status":      *state.loop_status.lock().unwrap(),
            "queue_pending":    queue_pending_snap,
            "hardware_mode":    state.sila_clients.is_some(),
            "agenda_complete":  agenda_complete_snap,
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

// ── Router builder (used by main and integration tests) ───────────────────────

/// Build the application router from an already-initialised [`AppState`].
///
/// Returns `Router<AppState>` — the caller applies `.with_state(state)` at the
/// end of all merges so that state is resolved in one pass.
///
/// Route layout:
/// - Protected routes (require JWT) are added first, then `route_layer(auth)`
///   is called so the auth middleware is applied only to those routes.
/// - Open (unauthenticated) routes are added after `route_layer(auth)`.
/// - The approvals sub-router (own `PendingApprovalQueue` state) is merged last.
///
/// Note: axum 0.7 uses matchit 0.7 which requires the `:param` (colon) syntax
/// for named path parameters — NOT the `{param}` syntax introduced in axum 0.8.
/// All parameterised routes here use `:param`.
pub(crate) fn build_router(approval_queue: Arc<PendingApprovalQueue>) -> Router<AppState> {
    let approvals_router = Router::new()
        .route("/api/approvals/pending", get(approvals::pending_handler))
        .route("/api/approvals/submit",  post(approvals::submit_handler))
        .with_state(Arc::clone(&approval_queue));

    Router::new()
        // ── Protected routes (added first so route_layer only covers these) ──
        .route("/api/emergency-stop",                post(emergency_stop_handler))
        .route("/api/audit/raw",                     get(audit_query::audit_raw_handler))
        .route("/api/lab/reagents",                  post(lab_register_reagent_handler))
        .route("/api/lab/reagents/:id",             delete(lab_remove_reagent_handler))
        .route("/api/lab/vessels/:id/contents",     put(lab_set_vessel_contents_handler))
        // Queue writes are operator actions — gated by JWT.
        .route("/api/queue",                         post(queue_enqueue_handler))
        .route("/api/queue/:id",                     delete(queue_remove_handler))
        // Auth layer — applies ONLY to the routes registered above.
        .route_layer(middleware::from_fn(auth::require_operator_jwt))
        // ── Open (unauthenticated) routes ────────────────────────────────────
        .route("/health",                          get(health_handler))
        .route("/ready",                           get(ready_handler))
        .route("/metrics",                         get(metrics_handler))
        .route("/approvals",                       get(approvals_ui::approvals_ui_handler))
        .route("/ws",                              get(ws_handler))
        .route("/api/status",                      get(status_handler))
        .route("/api/history",                     get(history_handler))
        .route("/api/journal",                     get(journal_handler))
        .route("/api/journal/findings",            get(findings_handler))
        .route("/api/audit",                       get(audit_query::audit_query_handler))
        .route("/api/audit/verify",                get(audit_query::audit_verify_handler))
        .route("/api/lab/reagents",                get(lab_reagents_handler))
        .route("/api/lab/vessels",                 get(lab_vessels_handler))
        .route("/api/lab/calibration-status",      get(lab_calibration_status_handler))
        // Protocol queue — read is open; writes are protected above.
        .route("/api/queue",                       get(queue_list_handler))
        // Commissioning agenda — read-only view of planned procedures.
        .route("/api/agenda",                      get(agenda_handler))
        // Approvals sub-router (own state, no auth middleware).
        .merge(approvals_router)
        .layer(CorsLayer::permissive())
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::hypothesis::HypothesisManager;
    use axum::body::Body;
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    use http_body_util::BodyExt;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use tower::ServiceExt; // for `oneshot`

    // ── Test fixture ─────────────────────────────────────────────────────────

    /// Raw 32-byte secret used by all JWT tests.
    const TEST_SECRET_BYTES: &[u8] = b"axiomlab-test-secret-32-bytes-ok";

    /// Serializes tests that mutate process-global env vars (`AXIOMLAB_JWT_SECRET`,
    /// `AXIOMLAB_AUDIT_LOG`). `cargo test` runs tests in parallel within a single
    /// process, so without this guard one test's `remove_var`/`set_var` can clobber
    /// another's expected auth state mid-request.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Acquire the env lock, recovering from poisoning so a panic in one test does
    /// not cascade into spurious failures elsewhere.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn test_secret_b64() -> String {
        B64.encode(TEST_SECRET_BYTES)
    }

    fn make_token(exp_offset_secs: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = if exp_offset_secs >= 0 {
            now + exp_offset_secs as u64
        } else {
            now.saturating_sub((-exp_offset_secs) as u64)
        };
        let claims = auth::JwtClaims {
            sub:  "test-operator".into(),
            role: "operator".into(),
            iat:  now,
            exp,
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET_BYTES),
        )
        .unwrap()
    }

    /// Build a minimal AppState backed by an in-memory (tempfile) SQLite DB.
    pub(super) async fn test_state() -> (AppState, Arc<PendingApprovalQueue>) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Arc::new(db::Db::open(tmp.path()).unwrap());
        std::mem::forget(tmp); // keep file alive; SQLite can use the fd

        let (tx, _) = broadcast::channel::<String>(16);
        let events  = EventBuffer::default();
        let journal = Arc::new(Mutex::new(DiscoveryJournal::default()));
        let log     = Arc::new(Mutex::new(ws_sink::ExecutionLog::default()));
        let approval_queue = PendingApprovalQueue::new();

        let state = AppState {
            tx,
            running:            Arc::new(AtomicBool::new(true)),
            iteration:          Arc::new(AtomicU32::new(0)),
            notebook:           Arc::new(Mutex::new(Vec::new())),
            log,
            events,
            journal,
            db:                 Arc::clone(&db),
            approval_queue:     Arc::clone(&approval_queue),
            audit_log_path:     "/dev/null".into(),
            sila_clients:       None,
            lab_state:          Arc::new(Mutex::new(LabState::default())),
            hypothesis_manager: Arc::new(Mutex::new(HypothesisManager::default())),
            protocol_queue:     Arc::new(Mutex::new(ProtocolQueue::load(
                                    &ProtocolQueue::default_path()
                                ))),
            loop_status: Arc::new(Mutex::new(ws_sink::LoopStatus::default())),
            metrics_handle:     metrics_exporter_prometheus::PrometheusBuilder::new()
                                    .build_recorder()
                                    .handle(),
        };
        (state, approval_queue)
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    // ── Auth tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn auth_missing_token_returns_401() {
        let _env = env_guard();
        std::env::set_var("AXIOMLAB_JWT_SECRET", test_secret_b64());
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/emergency-stop")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_expired_token_returns_401() {
        let _env = env_guard();
        std::env::set_var("AXIOMLAB_JWT_SECRET", test_secret_b64());
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let token = make_token(-3600); // expired 1 h ago
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/emergency-stop")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_valid_token_accepted() {
        let _env = env_guard();
        std::env::set_var("AXIOMLAB_JWT_SECRET", test_secret_b64());
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let token = make_token(3600);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/emergency-stop")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Status ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_status_returns_running_state() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        state.running.store(true, Ordering::SeqCst);
        state.iteration.store(7, Ordering::SeqCst);
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();

        let resp  = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json  = body_json(resp.into_body()).await;
        assert_eq!(json["running"],   true);
        assert_eq!(json["iteration"], 7);
    }

    // ── Emergency stop ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn emergency_stop_sets_running_false() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let running = Arc::clone(&state.running);
        running.store(true, Ordering::SeqCst);
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/emergency-stop")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!running.load(Ordering::SeqCst));
    }

    // ── Audit verify ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn audit_verify_empty_log_returns_valid() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        // Point AXIOMLAB_AUDIT_LOG at a guaranteed non-existent path so
        // verify_chain returns Ok(()) — trivially valid, no entries to check.
        std::env::set_var(
            "AXIOMLAB_AUDIT_LOG",
            "/tmp/axiomlab-test-nonexistent-audit-99999.ndjson",
        );
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/audit/verify")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["verified"], true);
    }

    // ── Reagent CRUD ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn register_reagent_then_list() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let app = build_router(Arc::clone(&aq)).with_state(state);

        let reagent = serde_json::json!({
            "id":                    "r-test-001",
            "name":                  "Sodium Chloride",
            "cas_number":            null,
            "lot_number":            "LOT-001",
            "concentration":         null,
            "concentration_unit":    null,
            "volume_ul":             1000.0,
            "expiry_secs":           null,
            "reference_material_id": null,
        });

        // Register via POST (protected — no JWT secret set, so auth passes).
        let post_req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/lab/reagents")
            .header("Content-Type", "application/json")
            .body(Body::from(reagent.to_string()))
            .unwrap();
        let post_resp = app.clone().oneshot(post_req).await.unwrap();
        assert_eq!(post_resp.status(), StatusCode::CREATED);

        // List via GET.
        let get_req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/lab/reagents")
            .body(Body::empty())
            .unwrap();
        let get_resp = app.oneshot(get_req).await.unwrap();
        assert_eq!(get_resp.status(), StatusCode::OK);
        let json = body_json(get_resp.into_body()).await;
        let arr = json.as_array().unwrap();
        assert!(arr.iter().any(|r| r["id"] == "r-test-001"));
    }

    // ── Protocol queue ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn queue_enqueue_and_list() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let app = build_router(Arc::clone(&aq)).with_state(state);

        // Enqueue a directive.
        let post_req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/queue")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "statement": "Measure pH at 5 NaOH volumes and fit linear model.",
                    "priority":  200,
                })).unwrap()
            ))
            .unwrap();

        let resp = app.clone().oneshot(post_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let id = body["id"].as_str().expect("id missing").to_owned();

        // List — should contain the new item.
        let get_req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/queue")
            .body(Body::empty())
            .unwrap();

        let list_resp = app.clone().oneshot(get_req).await.unwrap();
        assert_eq!(list_resp.status(), StatusCode::OK);
        let list = body_json(list_resp.into_body()).await;
        let items = list["items"].as_array().unwrap();
        assert!(items.iter().any(|i| i["id"] == id));

        // Remove it.
        let del_req = axum::http::Request::builder()
            .method("DELETE")
            .uri(format!("/api/queue/{id}"))
            .body(Body::empty())
            .unwrap();
        let del_resp = app.oneshot(del_req).await.unwrap();
        assert_eq!(del_resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn queue_empty_statement_rejected() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/queue")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "statement": "  ", "priority": 0 })).unwrap()
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn queue_delete_missing_returns_404() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("DELETE")
            .uri("/api/queue/nonexistent-id-xyz")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Agenda ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn agenda_returns_five_items() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/agenda")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["total_count"], 5);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 5);
        // Fresh journal — all items should be pending.
        assert!(items.iter().all(|i| i["status"] == "pending"));
    }

    // ── Queue JWT auth ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn queue_post_without_token_returns_401_when_secret_set() {
        let _env = env_guard();
        std::env::set_var("AXIOMLAB_JWT_SECRET", test_secret_b64());
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/queue")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "statement": "Run a calibration sweep.",
                    "priority": 100,
                })).unwrap()
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn queue_delete_without_token_returns_401_when_secret_set() {
        let _env = env_guard();
        std::env::set_var("AXIOMLAB_JWT_SECRET", test_secret_b64());
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("DELETE")
            .uri("/api/queue/some-id")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn queue_post_with_valid_token_succeeds_when_secret_set() {
        let _env = env_guard();
        std::env::set_var("AXIOMLAB_JWT_SECRET", test_secret_b64());
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let token = make_token(3600);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/queue")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "statement": "Measure absorbance at 650 nm across 8 concentrations.",
                    "priority": 150,
                })).unwrap()
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── Approval gate integration ─────────────────────────────────────────────

    fn approval_ctx() -> agent_runtime::approval_queue::ApprovalContext {
        agent_runtime::approval_queue::ApprovalContext {
            hypothesis:                 "absorbance scales with concentration".into(),
            experiment_id:              "exp-gate-test".into(),
            iteration:                  1,
            risk_class:                 Some("LiquidHandling".into()),
            recent_actions:             vec![],
            journal_summary:            String::new(),
            protocol_step:              None,
            findings_before_experiment: 0,
        }
    }

    #[tokio::test]
    async fn approval_gate_pending_appears_in_list() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;

        let (pending_id, _rx) = aq.enqueue(
            "dispense",
            serde_json::json!({"pump_id": "p1", "volume_ul": 500}),
            None,
            approval_ctx(),
        );

        let app = build_router(Arc::clone(&aq)).with_state(state);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/approvals/pending")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body.as_array().expect("expected array");
        assert!(
            items.iter().any(|i| i["pending_id"] == pending_id),
            "enqueued pending_id not found in list"
        );

        aq.remove(&pending_id);
    }

    #[tokio::test]
    async fn approval_gate_deny_returns_ok_and_clears_entry() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;

        let (pending_id, _rx) = aq.enqueue(
            "move_arm",
            serde_json::json!({"x": 10, "y": 20, "z": 5}),
            None,
            approval_ctx(),
        );

        let app = build_router(Arc::clone(&aq)).with_state(state);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/approvals/submit")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "pending_id": pending_id,
                    "bundle": null,
                })).unwrap()
            ))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "submitted");
        assert_eq!(body["pending_id"], pending_id);

        aq.remove(&pending_id);
    }

    #[tokio::test]
    async fn approval_gate_submit_unknown_id_returns_404() {
        let _env = env_guard();
        std::env::remove_var("AXIOMLAB_JWT_SECRET");
        let (state, aq) = test_state().await;
        let app = build_router(aq).with_state(state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/approvals/submit")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "pending_id": "nonexistent-uuid-abc",
                    "bundle": null,
                })).unwrap()
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axiomlab=info,tower_http=info".into()),
        )
        .init();

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder");

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

    if audit_signer.is_none() {
        tracing::warn!(
            "Could not initialize audit signing key — entries will be unsigned. \
             Set AXIOMLAB_AUDIT_SIGNING_KEY or AXIOMLAB_AUDIT_SIGNING_KEY_PATH."
        );
    }

    // Load the persistent operation log (or start fresh).
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

    // Seed perpetual calibrations for instruments that have no calibrate tool.
    // The pH meter is calibratable by the agent via `calibrate_ph`, but the
    // spectrophotometer and incubator have no such tool — so without a seeded
    // record their quantitative reads (e.g. read_absorbance) would be gated
    // off forever. Only seeds when missing, so it's a no-op after first launch.
    {
        let mut j = journal.lock().unwrap();
        let mut seeded = false;
        for (inst, standard) in [
            ("spectrophotometer", "factory reference: 0 AU blank + 1.0 AU standard"),
            ("incubator",         "factory reference: 25 C / 37 C set-points"),
        ] {
            if j.last_calibration_for(inst).is_none() {
                j.record_calibration(inst, standard, 0.0);
                tracing::info!(instrument = inst, "seeded perpetual instrument calibration");
                seeded = true;
            }
        }
        if seeded {
            let _ = j.save(&path);
        }
    }

    // ── SQLite setup ──────────────────────────────────────────────────────────
    let sqlite_db = match db::Db::open(&db::db_path()) {
        Ok(d) => {
            tracing::info!(path = %db::db_path().display(), "SQLite journal database opened");
            Arc::new(d)
        }
        Err(e) => {
            tracing::error!("SQLite open failed ({e}) — cannot continue without persistence");
            std::process::exit(1);
        }
    };
    // Reconstruct from JSON backup on first launch or after DB deletion.
    if sqlite_db.is_empty() {
        let j = journal.lock().unwrap();
        sqlite_db.reconstruct_from_journal(&j);
    }

    // ── SiLA 2 hardware connection ─────────────────────────────────────────────
    let sila_endpoint = std::env::var("SILA2_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:50052".into());
    let sila_clients: Option<Arc<SiLA2Clients>> =
        match SiLA2Clients::connect(&sila_endpoint).await {
            Ok(c) => {
                tracing::info!("SiLA 2 hardware connected at {sila_endpoint}");
                Some(Arc::new(c))
            }
            Err(e) => {
                tracing::warn!(
                    "SiLA 2 unavailable ({e}) — running with in-process physics simulator"
                );
                None
            }
        };

    // ── Lab state (reagent inventory) ─────────────────────────────────────────
    let lab_state = Arc::new(Mutex::new(LabState::load()));
    {
        let ls = lab_state.lock().unwrap();
        tracing::info!(
            reagents = ls.reagents.len(),
            vessels  = ls.vessel_contents.len(),
            "Lab state loaded"
        );
    }

    let approval_queue = PendingApprovalQueue::new();

    let state = AppState {
        tx,
        running:        Arc::new(AtomicBool::new(false)),
        iteration:      Arc::new(AtomicU32::new(0)),
        notebook:       Arc::new(Mutex::new(Vec::new())),
        log:            Arc::new(Mutex::new(ExecutionLog::from_journal(&journal.lock().unwrap()))),
        events:         events.clone(),
        journal:        Arc::clone(&journal),
        db:             Arc::clone(&sqlite_db),
        approval_queue: Arc::clone(&approval_queue),
        audit_log_path: audit_path_str.clone(),
        sila_clients:   sila_clients.clone(),
        lab_state:      Arc::clone(&lab_state),
        hypothesis_manager: Arc::new(Mutex::new(
            sqlite_db.load_hypothesis_manager().unwrap_or_default()
        )),
        protocol_queue: Arc::new(Mutex::new(ProtocolQueue::load(
            &ProtocolQueue::default_path()
        ))),
        loop_status: Arc::new(Mutex::new(ws_sink::LoopStatus::default())),
        metrics_handle,
    };

    // Auto-start the execution loop immediately on server launch.
    {
        let sink = Arc::new(ws_sink::WebSocketSink {
            tx:       state.tx.clone(),
            log:      Arc::clone(&state.log),
            notebook: Arc::clone(&state.notebook),
            events,
            journal:  Arc::clone(&journal),
            db:       Arc::clone(&sqlite_db),
            loop_status: Arc::clone(&state.loop_status),
        });
        state.running.store(true, Ordering::SeqCst);
        let running         = Arc::clone(&state.running);
        let iteration       = Arc::clone(&state.iteration);
        let db_for_loop        = Arc::clone(&sqlite_db);
        let sila_for_loop      = sila_clients.clone();
        let lab_for_loop       = Arc::clone(&state.lab_state);
        let hyp_mgr_for_loop   = Arc::clone(&state.hypothesis_manager);
        let pq_for_loop        = Arc::clone(&state.protocol_queue);
        tokio::spawn(async move {
            simulator::run_loop(sink, running.clone(), iteration, approval_queue, db_for_loop, sila_for_loop, lab_for_loop, hyp_mgr_for_loop, pq_for_loop).await;
            running.store(false, Ordering::SeqCst);
        });
    }

    // Static file serving — serves the built React app from ../visualizer/dist.
    let static_files = ServeDir::new("../visualizer/dist")
        .append_index_html_on_directories(true);

    let app = build_router(Arc::clone(&state.approval_queue))
        .with_state(state)
        .fallback_service(static_files);

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
