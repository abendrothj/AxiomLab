//! HTTP + WebSocket handlers. Routes only — every handler delegates to the
//! chain, the queues, or lab state. No business logic lives here.

use crate::auth;
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, Query, State, ws::WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::Ordering;

// ── /api/status ─────────────────────────────────────────────────────────────

pub async fn status(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "running": s.running.load(Ordering::Relaxed),
        "iteration": s.iteration.load(Ordering::Relaxed),
        "queue": s.protocol_queue.list().len(),
        "pending_approvals": s.approval_queue.list_pending().len(),
        "backend": if s.clients.is_simulator() { "simulator" } else { "hardware" },
    }))
}

// ── /api/audit ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn audit(State(s): State<AppState>, Query(p): Query<Pagination>) -> impl IntoResponse {
    let limit = p.limit.unwrap_or(50).min(500);
    let offset = p.offset.unwrap_or(0);
    match s.audit_chain.entries() {
        Ok(mut entries) => {
            entries.reverse(); // newest first
            let total = entries.len();
            let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
            let verify = s.audit_chain.verify().ok();
            Json(json!({
                "total": total,
                "limit": limit,
                "offset": offset,
                "verified": verify.as_ref().map(|v| v.signatures_verified == v.entries_checked),
                "tip_hash": verify.and_then(|v| v.tip_hash_hex),
                "entries": page,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("audit read failed: {e}")).into_response(),
    }
}

pub async fn audit_verify(State(s): State<AppState>) -> impl IntoResponse {
    match s.audit_chain.verify() {
        Ok(v) => Json(json!({
            "ok": true,
            "entries_checked": v.entries_checked,
            "signatures_verified": v.signatures_verified,
            "tip_hash": v.tip_hash_hex,
        }))
        .into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })).into_response(),
    }
}

// ── /api/agenda ─────────────────────────────────────────────────────────────

pub async fn agenda(State(s): State<AppState>) -> Json<serde_json::Value> {
    // A fixed commissioning agenda; an item is "completed" once a protocol
    // conclusion exists in the chain.
    let concluded = s
        .audit_chain
        .entries()
        .map(|e| e.iter().any(|x| x.action == "protocol_conclusion"))
        .unwrap_or(false);
    let item = |key: &str, statement: &str| {
        json!({ "key": key, "statement": statement, "status": if concluded { "completed" } else { "pending" } })
    };
    Json(json!([
        item("calibrate_spectrophotometer", "Establish a valid absorbance calibration"),
        item("dispense_accuracy", "Confirm dispense accuracy within capability bounds"),
        item("arm_reachability", "Verify arm reaches all registered vessels safely"),
    ]))
}

// ── /api/queue ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DirectiveBody {
    pub directive: String,
}

pub async fn queue_list(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(s.protocol_queue.list()))
}

pub async fn queue_push(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DirectiveBody>,
) -> impl IntoResponse {
    if let Err(e) = auth::verify(&headers, &s.jwt_secret) {
        return (StatusCode::UNAUTHORIZED, e).into_response();
    }
    let id = s.protocol_queue.push(body.directive.clone());
    s.broadcast(json!({ "event": "queued", "id": id, "directive": body.directive }));
    (StatusCode::ACCEPTED, Json(json!({ "id": id }))).into_response()
}

pub async fn queue_cancel(State(s): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    if s.protocol_queue.cancel(&id) {
        (StatusCode::OK, Json(json!({ "cancelled": id }))).into_response()
    } else {
        (StatusCode::CONFLICT, "item not pending or not found").into_response()
    }
}

// ── /api/approvals ──────────────────────────────────────────────────────────

pub async fn approvals_list(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(s.approval_queue.list_pending()))
}

#[derive(Debug, Deserialize)]
pub struct DecisionBody {
    pub approved: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub approver_id: String,
}

pub async fn approvals_resolve(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DecisionBody>,
) -> impl IntoResponse {
    let decision = axiom_gate::Decision {
        approved: body.approved,
        notes: body.notes,
        approver_id: if body.approver_id.is_empty() { "operator".into() } else { body.approver_id },
    };
    match s.approval_queue.resolve(&id, decision) {
        Ok(()) => {
            s.broadcast(json!({ "event": "approval_resolved", "id": id, "approved": body.approved }));
            (StatusCode::OK, Json(json!({ "resolved": id }))).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

// ── /api/lab ────────────────────────────────────────────────────────────────

pub async fn lab(State(s): State<AppState>) -> Json<serde_json::Value> {
    let lab = s.lab_state.lock().unwrap();
    Json(json!({ "reagents": lab.reagents, "vessel_contents": lab.vessel_contents }))
}

// ── /ready ──────────────────────────────────────────────────────────────────

pub async fn ready() -> &'static str {
    "ok"
}

// ── /ws ─────────────────────────────────────────────────────────────────────

pub async fn ws(State(s): State<AppState>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    let mut rx = s.tx.subscribe();
    upgrade.on_upgrade(move |mut socket| async move {
        use axum::extract::ws::Message;
        while let Ok(msg) = rx.recv().await {
            if socket.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    })
}
