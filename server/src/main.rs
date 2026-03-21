mod approvals;
mod approvals_ui;
mod audit_query;
mod auth;
mod db;
mod discovery;
mod eln;
mod lab_scheduler;
mod literature;
mod oidc;
mod simulator;
mod stall;
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
    anchor_chain_tip_to_rekor, audit_log_path, audit_signer_from_env, emit_emergency_stop,
    emit_session_start, rotate_if_needed,
};
use agent_runtime::hardware::SiLA2Clients;
use agent_runtime::lab_state::{LabState, Reagent};
use discovery::{MethodValidation, ReferenceMaterial};
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
    /// SQLite persistence — dual-write target for all journal mutations.
    pub db:          Arc<db::Db>,
    approval_queue:  Arc<PendingApprovalQueue>,
    audit_log_path:  String,
    /// IDs of approvals that stalled on the previous run (no dispatch_complete).
    pub stalled_ids: Arc<Mutex<Vec<String>>>,
    /// SiLA 2 hardware clients — `None` when running in simulator mode.
    sila_clients: Option<Arc<SiLA2Clients>>,
    /// Reagent inventory and vessel contents.
    pub lab_state: Arc<Mutex<LabState>>,
    /// Rich hypothesis state machine — shared with the orchestrator.
    pub hypothesis_manager: Arc<Mutex<HypothesisManager>>,
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
    axum::Json(serde_json::json!({
        "running":    s.running.load(Ordering::SeqCst),
        "iteration":  s.iteration.load(Ordering::SeqCst),
        "notebook":   notebook,
        "slot_count": slot_count,
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

// ── ZK proof status route ─────────────────────────────────────────────────────

async fn zk_status_handler() -> impl IntoResponse {
    let zk_enabled = zk_audit::types::ZkConfig::from_env().is_some();
    axum::Json(serde_json::json!({
        "status":  if zk_enabled { "enabled" } else { "disabled" },
        "details": if zk_enabled {
            "ZK proof anchoring is configured. Submit a proof via the prover binary to update status."
        } else {
            "ZK proof anchoring is disabled. Set AXIOMLAB_BASE_RPC_URL, AXIOMLAB_BASE_CONTRACT_ADDR, \
             and AXIOMLAB_BASE_WALLET_KEY to enable."
        },
        "use_case": zk_audit::types::ZkConfig::from_env()
            .map(|c| format!("{:?}", c.use_case))
            .unwrap_or_else(|| "n/a".into()),
    }))
}

// ── ELN export routes ─────────────────────────────────────────────────────────

async fn benchling_export_handler(
    Path(study_id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    use eln::ELNAdapter;

    let adapter = match eln::BenchlingAdapter::from_env() {
        Some(a) => a,
        None => return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "error": "ELN not configured — set AXIOMLAB_BENCHLING_TOKEN, \
                          AXIOMLAB_BENCHLING_TENANT, and AXIOMLAB_BENCHLING_PROJECT_ID"
            })),
        ).into_response(),
    };

    let (study, runs) = {
        let j = s.journal.lock().unwrap();
        let study = match j.studies.iter().find(|st| st.id == study_id) {
            Some(s) => s.clone(),
            None => return (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": "study not found"})),
            ).into_response(),
        };
        // Gather run summaries that belong to this study.
        let runs: Vec<_> = j.runs.iter()
            .filter(|r| study.run_ids.contains(&r.run_id))
            .cloned()
            .collect();
        (study, runs)
    };

    match adapter.export_study(&study, &runs).await {
        Ok(url) => axum::Json(serde_json::json!({
            "status":       "exported",
            "study_id":     study_id,
            "benchling_url": url,
        })).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({"error": e})),
        ).into_response(),
    }
}

// ── Literature / PubChem proxy route ──────────────────────────────────────────

#[derive(serde::Deserialize)]
struct LiteratureQuery {
    q: String,
}

async fn literature_search_handler(
    Query(params): Query<LiteratureQuery>,
) -> impl IntoResponse {
    match literature::search_pubchem(&params.q).await {
        Ok(summary) => axum::Json(serde_json::to_value(summary).unwrap_or_default()).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": e})),
        ).into_response(),
    }
}

// ── ISO 17025 — method validation routes ──────────────────────────────────────

async fn methods_list_handler(State(s): State<AppState>) -> impl IntoResponse {
    let methods = s.journal.lock().unwrap().method_validations.clone();
    axum::Json(methods)
}

async fn methods_create_handler(
    State(s): State<AppState>,
    Json(validation): Json<MethodValidation>,
) -> impl IntoResponse {
    let id = {
        let mut j = s.journal.lock().unwrap();
        let id = j.add_method_validation(validation);
        j.save(&journal_path()).unwrap_or_else(|e| {
            tracing::warn!("Failed to save journal after method validation: {e}");
        });
        id
    };
    (StatusCode::CREATED, axum::Json(serde_json::json!({"status": "created", "id": id})))
}

async fn method_get_handler(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let journal = s.journal.lock().unwrap();
    match journal.method_validations.iter().find(|v| v.id == id) {
        Some(v) => axum::Json(serde_json::to_value(v).unwrap_or_default()).into_response(),
        None    => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error": "not found"}))).into_response(),
    }
}

// ── ISO 17025 — reference material routes ─────────────────────────────────────

async fn ref_materials_list_handler(State(s): State<AppState>) -> impl IntoResponse {
    let materials = s.journal.lock().unwrap().reference_materials.clone();
    axum::Json(materials)
}

async fn ref_materials_create_handler(
    State(s): State<AppState>,
    Json(material): Json<ReferenceMaterial>,
) -> impl IntoResponse {
    let id = {
        let mut j = s.journal.lock().unwrap();
        let id = j.register_reference_material(material);
        j.save(&journal_path()).unwrap_or_else(|e| {
            tracing::warn!("Failed to save journal after reference material: {e}");
        });
        id
    };
    (StatusCode::CREATED, axum::Json(serde_json::json!({"status": "created", "id": id})))
}

// ── ISO 17025 — study record routes ───────────────────────────────────────────

#[derive(serde::Deserialize)]
struct CreateStudyRequest {
    title: String,
    study_director_id: String,
}

#[derive(serde::Deserialize)]
struct AddProtocolRequest {
    protocol_id: String,
}

#[derive(serde::Deserialize)]
struct QaReviewRequest {
    reviewer_id: String,
}

async fn studies_list_handler(State(s): State<AppState>) -> impl IntoResponse {
    let studies = s.journal.lock().unwrap().studies.clone();
    axum::Json(studies)
}

async fn studies_create_handler(
    State(s): State<AppState>,
    Json(req): Json<CreateStudyRequest>,
) -> impl IntoResponse {
    let id = {
        let mut j = s.journal.lock().unwrap();
        let id = j.create_study(req.title, req.study_director_id);
        j.save(&journal_path()).unwrap_or_else(|e| {
            tracing::warn!("Failed to save journal after study creation: {e}");
        });
        id
    };
    (StatusCode::CREATED, axum::Json(serde_json::json!({"status": "created", "id": id})))
}

async fn study_get_handler(
    Path(id): Path<String>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let journal = s.journal.lock().unwrap();
    match journal.studies.iter().find(|s| s.id == id) {
        Some(study) => axum::Json(serde_json::to_value(study).unwrap_or_default()).into_response(),
        None        => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error": "not found"}))).into_response(),
    }
}

async fn study_add_protocol_handler(
    Path(study_id): Path<String>,
    State(s): State<AppState>,
    Json(req): Json<AddProtocolRequest>,
) -> impl IntoResponse {
    let ok = {
        let mut j = s.journal.lock().unwrap();
        let ok = j.add_protocol_to_study(&study_id, req.protocol_id.clone());
        if ok {
            j.save(&journal_path()).unwrap_or_else(|e| {
                tracing::warn!("Failed to save journal after protocol registration: {e}");
            });
        }
        ok
    };
    if ok {
        axum::Json(serde_json::json!({"status": "registered", "study_id": study_id, "protocol_id": req.protocol_id})).into_response()
    } else {
        (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error": "study not found"}))).into_response()
    }
}

async fn study_qa_review_handler(
    Path(study_id): Path<String>,
    State(s): State<AppState>,
    Json(req): Json<QaReviewRequest>,
) -> impl IntoResponse {
    let result = {
        let mut j = s.journal.lock().unwrap();
        let hash = j.qa_sign_off(&study_id, &req.reviewer_id);
        if hash.is_some() {
            j.save(&journal_path()).unwrap_or_else(|e| {
                tracing::warn!("Failed to save journal after QA sign-off: {e}");
            });
        }
        hash
    };
    match result {
        Some(hash) => {
            tracing::info!(study_id, reviewer = %req.reviewer_id, hash, "QA sign-off recorded");
            axum::Json(serde_json::json!({
                "status": "signed_off",
                "study_id": study_id,
                "qa_sign_off_hash": hash,
            })).into_response()
        }
        None => (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "sign-off failed — study not found or reviewer is the study director"
            })),
        ).into_response(),
    }
}

// ── POST /api/emergency-stop ──────────────────────────────────────────────────

async fn emergency_stop_handler(State(s): State<AppState>) -> impl IntoResponse {
    // 1. Halt the software exploration loop immediately.
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

    tracing::warn!("EMERGENCY STOP triggered — exploration loop halted");

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
        .route("/api/approvals/recover/:id",        post(recovery_resume_handler))
        .route("/api/approvals/recover/:id/cancel",  post(recovery_cancel_handler))
        .route("/api/emergency-stop",                post(emergency_stop_handler))
        .route("/api/audit/raw",                     get(audit_query::audit_raw_handler))
        .route("/api/lab/reagents",                  post(lab_register_reagent_handler))
        .route("/api/lab/reagents/:id",             delete(lab_remove_reagent_handler))
        .route("/api/lab/vessels/:id/contents",     put(lab_set_vessel_contents_handler))
        .route("/api/export/benchling/:study_id",   post(benchling_export_handler))
        .route("/api/methods",                       post(methods_create_handler))
        .route("/api/lab/reference-materials",       post(ref_materials_create_handler))
        .route("/api/studies",                       post(studies_create_handler))
        .route("/api/studies/:id/protocols",        post(study_add_protocol_handler))
        .route("/api/studies/:id/qa-review",        post(study_qa_review_handler))
        // Auth layer — applies ONLY to the routes registered above.
        .route_layer(middleware::from_fn(auth::require_operator_jwt))
        // ── Open (unauthenticated) routes ────────────────────────────────────
        .route("/approvals",                       get(approvals_ui::approvals_ui_handler))
        .route("/ws",                              get(ws_handler))
        .route("/api/status",                      get(status_handler))
        .route("/api/history",                     get(history_handler))
        .route("/api/journal",                     get(journal_handler))
        .route("/api/journal/findings",            get(findings_handler))
        .route("/api/audit",                       get(audit_query::audit_query_handler))
        .route("/api/audit/verify",                get(audit_query::audit_verify_handler))
        .route("/api/approvals/stalled",           get(stalled_handler))
        .route("/api/lab/reagents",                get(lab_reagents_handler))
        .route("/api/lab/vessels",                 get(lab_vessels_handler))
        .route("/api/lab/calibration-status",      get(lab_calibration_status_handler))
        .route("/api/audit/zk-status",             get(zk_status_handler))
        .route("/api/literature/search",           get(literature_search_handler))
        .route("/api/methods",                     get(methods_list_handler))
        .route("/api/methods/:id",                get(method_get_handler))
        .route("/api/lab/reference-materials",     get(ref_materials_list_handler))
        .route("/api/studies",                     get(studies_list_handler))
        .route("/api/studies/:id",                get(study_get_handler))
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
        let log     = Arc::new(Mutex::new(ws_sink::ExplorationLog::default()));
        let approval_queue = PendingApprovalQueue::new();

        let state = AppState {
            tx,
            running:            Arc::new(AtomicBool::new(true)),
            stalled:            Arc::new(AtomicBool::new(false)),
            iteration:          Arc::new(AtomicU32::new(0)),
            notebook:           Arc::new(Mutex::new(Vec::new())),
            log,
            events,
            journal,
            db:                 Arc::clone(&db),
            approval_queue:     Arc::clone(&approval_queue),
            audit_log_path:     "/dev/null".into(),
            stalled_ids:        Arc::new(Mutex::new(Vec::new())),
            sila_clients:       None,
            lab_state:          Arc::new(Mutex::new(LabState::default())),
            hypothesis_manager: Arc::new(Mutex::new(HypothesisManager::default())),
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

    // ── Study QA review ───────────────────────────────────────────────────────
    //
    // These tests use a minimal single-purpose router (no `route_layer` split)
    // to avoid the axum 0.7 behaviour where `route_layer` places the protected
    // routes behind an inner fallback router that the outer matchit trie can
    // shadow when open routes share the same path-parameter prefix.

    #[tokio::test]
    async fn create_study_then_qa_review_requires_different_reviewer() {
        let (state, _aq) = test_state().await;

        // Pre-populate the study directly in the shared journal.
        let study_id = state.journal.lock().unwrap()
            .create_study("Test Study".into(), "alice".into());

        // Minimal router: only the QA-review route — no route_layer split.
        let app = Router::new()
            .route("/api/studies/:id/qa-review", post(study_qa_review_handler))
            .with_state(state);

        // QA review by a *different* reviewer "bob" — must succeed (200).
        let qa_body = serde_json::json!({"reviewer_id": "bob"});
        let qa_req = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/studies/{study_id}/qa-review"))
            .header("Content-Type", "application/json")
            .body(Body::from(qa_body.to_string()))
            .unwrap();
        let qa_resp = app.oneshot(qa_req).await.unwrap();
        assert_eq!(qa_resp.status(), StatusCode::OK);
        let qa_json = body_json(qa_resp.into_body()).await;
        assert_eq!(qa_json["status"], "signed_off");
    }

    #[tokio::test]
    async fn create_study_then_qa_review_same_reviewer_returns_error() {
        let (state, _aq) = test_state().await;

        // Pre-populate the study directly in the shared journal.
        let study_id = state.journal.lock().unwrap()
            .create_study("Conflict Study".into(), "alice".into());

        // Minimal router: only the QA-review route — no route_layer split.
        let app = Router::new()
            .route("/api/studies/:id/qa-review", post(study_qa_review_handler))
            .with_state(state);

        // QA review by the *same* director "alice" — must be rejected (400).
        let qa_body = serde_json::json!({"reviewer_id": "alice"});
        let qa_req = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/studies/{study_id}/qa-review"))
            .header("Content-Type", "application/json")
            .body(Body::from(qa_body.to_string()))
            .unwrap();
        let qa_resp = app.oneshot(qa_req).await.unwrap();
        assert_eq!(qa_resp.status(), StatusCode::BAD_REQUEST);
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
            "Could not initialize audit signing key — entries will be unsigned \
             and Rekor checkpointing is disabled. \
             Set AXIOMLAB_AUDIT_SIGNING_KEY or AXIOMLAB_AUDIT_SIGNING_KEY_PATH, \
             or ensure ~/.config/axiomlab/ is writable."
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
        db:             Arc::clone(&sqlite_db),
        approval_queue: Arc::clone(&approval_queue),
        audit_log_path: audit_path_str.clone(),
        stalled_ids:    Arc::new(Mutex::new(stalled_ids)),
        sila_clients:   sila_clients.clone(),
        lab_state:      Arc::clone(&lab_state),
        hypothesis_manager: Arc::new(Mutex::new(
            sqlite_db.load_hypothesis_manager().unwrap_or_default()
        )),
    };

    // Auto-start the exploration loop immediately on server launch.
    {
        let sink = Arc::new(ws_sink::WebSocketSink {
            tx:       state.tx.clone(),
            log:      Arc::clone(&state.log),
            notebook: Arc::clone(&state.notebook),
            events,
            journal:  Arc::clone(&journal),
            db:       Arc::clone(&sqlite_db),
        });
        state.running.store(true, Ordering::SeqCst);
        let running         = Arc::clone(&state.running);
        let stalled         = Arc::clone(&state.stalled);
        let iteration       = Arc::clone(&state.iteration);
        let db_for_loop        = Arc::clone(&sqlite_db);
        let sila_for_loop      = sila_clients.clone();
        let lab_for_loop       = Arc::clone(&state.lab_state);
        let hyp_mgr_for_loop   = Arc::clone(&state.hypothesis_manager);
        tokio::spawn(async move {
            simulator::run_loop(sink, running.clone(), stalled, iteration, approval_queue, db_for_loop, sila_for_loop, lab_for_loop, hyp_mgr_for_loop).await;
            running.store(false, Ordering::SeqCst);
        });
    }

    // Static file serving — serves the built React app from ../visualizer/dist.
    let static_files = ServeDir::new("../visualizer/dist")
        .append_index_html_on_directories(true);

    // OIDC routes — optional; only registered when OIDC is configured.
    let oidc_router: Router<AppState> = if let Some(cfg) = oidc::OidcConfig::from_env() {
        tracing::info!(issuer = %cfg.issuer_url, "OIDC authentication enabled");
        let oidc_state = oidc::OidcState { config: cfg, store: oidc::PkceStore::default() };
        Router::new()
            .route("/api/auth/oidc/start",    get(oidc::oidc_start_handler))
            .route("/api/auth/oidc/callback", get(oidc::oidc_callback_handler))
            .route("/api/auth/logout",        post(oidc::logout_handler))
            .with_state(oidc_state)
    } else {
        tracing::info!("OIDC not configured (set AXIOMLAB_OIDC_* vars to enable)");
        Router::new()
    };

    let app = build_router(Arc::clone(&state.approval_queue))
        .merge(oidc_router)
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
