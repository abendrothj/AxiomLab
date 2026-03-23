//! HTTP handlers for the interactive human approval queue.
//!
//! `GET  /api/approvals/pending` — list actions waiting for operator approval.
//! `POST /api/approvals/submit`  — submit a signed bundle (or explicit denial).

use agent_runtime::approval_queue::{PendingApprovalQueue, SubmitError};
use agent_runtime::approvals::SignedApproval;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

// ── GET /api/approvals/pending ────────────────────────────────────────────────

pub async fn pending_handler(
    State(queue): State<Arc<PendingApprovalQueue>>,
) -> impl IntoResponse {
    Json(queue.list())
}

// ── POST /api/approvals/submit ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SubmitRequest {
    pub pending_id: String,
    /// `None` = operator explicitly denies. `Some` = approval bundle to validate.
    pub bundle: Option<Vec<SignedApproval>>,
}

pub async fn submit_handler(
    State(queue): State<Arc<PendingApprovalQueue>>,
    Json(req): Json<SubmitRequest>,
) -> impl IntoResponse {
    let decision = if req.bundle.is_some() { "approved" } else { "denied" };
    match queue.submit(&req.pending_id, req.bundle) {
        Ok(()) => {
            info!(pending_id = %req.pending_id, %decision, "approval submitted");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "submitted",
                    "pending_id": req.pending_id,
                })),
            )
        }
        Err(SubmitError::NotFound) => {
            warn!(pending_id = %req.pending_id, "approval submit: pending_id not found");
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "pending_id not found or already timed out",
                    "pending_id": req.pending_id,
                })),
            )
        }
        Err(SubmitError::AlreadyConsumed) => {
            warn!(pending_id = %req.pending_id, "approval submit: already consumed");
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "approval already submitted for this pending_id",
                    "pending_id": req.pending_id,
                })),
            )
        }
    }
}
