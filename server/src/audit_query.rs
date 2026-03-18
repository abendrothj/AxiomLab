//! HTTP handlers for querying and verifying the audit log.
//!
//! `GET /api/audit`        — filter events by action / decision / since / limit
//! `GET /api/audit/verify` — verify the full hash chain, returning `{verified: true}` or an error
//! `GET /api/audit/raw`    — return the full JSONL audit log as `text/plain`

use agent_runtime::audit::{audit_log_path, verify_chain};
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::AppState;

// ── Query parameters ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct AuditQueryParams {
    /// Filter by `action` field (e.g. "journal_finding", "protocol_step").
    pub action: Option<String>,
    /// Filter by `decision` field ("allow" or "deny").
    pub decision: Option<String>,
    /// Unix timestamp (seconds): only return events at or after this time.
    pub since: Option<u64>,
    /// Maximum number of events to return (capped at 1 000).
    pub limit: Option<usize>,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AuditVerifyResponse {
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/audit` — stream-filter the JSONL audit log.
///
/// Reads the log file line-by-line and applies the query filters without loading
/// the full file into memory.  Returns a JSON array of matching event objects.
pub async fn audit_query_handler(
    Query(params): Query<AuditQueryParams>,
    _state: State<AppState>,
) -> impl IntoResponse {
    const MAX_LIMIT: usize = 1_000;
    let limit = params.limit.unwrap_or(MAX_LIMIT).min(MAX_LIMIT);

    let path = audit_log_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Json(serde_json::json!([]));
        }
        Err(e) => {
            return Json(serde_json::json!({
                "error": format!("failed to read audit log: {e}")
            }));
        }
    };

    let mut results: Vec<serde_json::Value> = Vec::new();
    for line in content.lines() {
        if results.len() >= limit {
            break;
        }
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        // action filter
        if let Some(ref action) = params.action {
            if entry.get("action").and_then(|v| v.as_str()) != Some(action.as_str()) {
                continue;
            }
        }
        // decision filter
        if let Some(ref decision) = params.decision {
            if entry.get("decision").and_then(|v| v.as_str()) != Some(decision.as_str()) {
                continue;
            }
        }
        // since filter
        if let Some(since) = params.since {
            let ts = entry.get("unix_secs").and_then(|v| v.as_u64()).unwrap_or(0);
            if ts < since {
                continue;
            }
        }

        results.push(entry);
    }

    Json(serde_json::json!(results))
}

/// `GET /api/audit/raw` — return the full JSONL audit log as `text/plain`.
///
/// Used by the Chain Explorer's "Download full log" button.
/// Returns 404 with a JSON error if the log file does not yet exist.
pub async fn audit_raw_handler(_state: State<AppState>) -> impl IntoResponse {
    let path = audit_log_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            content,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "audit log not found\n".to_string(),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("error reading audit log: {e}\n"),
        )
            .into_response(),
    }
}

/// `GET /api/audit/verify` — verify the hash chain integrity.
///
/// Calls `verify_chain` directly (no subprocess).  Returns
/// `{"verified": true}` on success or `{"verified": false, "error": "..."}` on failure.
pub async fn audit_verify_handler(_state: State<AppState>) -> impl IntoResponse {
    let path = audit_log_path().to_string_lossy().into_owned();
    match verify_chain(&path) {
        Ok(()) => (
            StatusCode::OK,
            Json(AuditVerifyResponse { verified: true, error: None }),
        ),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(AuditVerifyResponse { verified: false, error: Some(e) }),
        ),
    }
}
