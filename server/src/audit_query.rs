//! HTTP handlers for querying and verifying the audit log.
//!
//! `GET /api/audit`        — filter events by action / decision / since / limit
//! `GET /api/audit/verify` — verify the full hash chain, returning `{verified: true}` or an error
//! `GET /api/audit/raw`    — stream the full JSONL audit log as `text/plain`
//!
//! # Memory efficiency
//! `audit_query_handler` reads the JSONL file with a `BufReader` — it never
//! loads the full file into RAM.  `audit_raw_handler` streams the file using
//! `axum::body::Body::from_stream` so even very large logs (100 MB rotation
//! threshold) are served without memory spikes.

use agent_runtime::audit::{audit_log_path, verify_chain};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use tokio_util::io::ReaderStream;

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
/// Reads the log file line-by-line via `BufReader` (never loads the full file
/// into RAM) and applies query filters.  Returns a JSON array of matching events.
pub async fn audit_query_handler(
    Query(params): Query<AuditQueryParams>,
    _state: State<AppState>,
) -> impl IntoResponse {
    const MAX_LIMIT: usize = 1_000;
    let limit = params.limit.unwrap_or(MAX_LIMIT).min(MAX_LIMIT);

    let path = audit_log_path();
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Json(serde_json::json!([]));
        }
        Err(e) => {
            return Json(serde_json::json!({
                "error": format!("failed to read audit log: {e}")
            }));
        }
    };

    let reader = std::io::BufReader::new(file);
    let mut results: Vec<serde_json::Value> = Vec::new();

    for line_res in reader.lines() {
        if results.len() >= limit {
            break;
        }
        let Ok(line) = line_res else { continue };
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) else {
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

/// `GET /api/audit/raw` — stream the full JSONL audit log as `text/plain`.
///
/// Uses `tokio::fs::File` + `ReaderStream` so even large logs (100 MB rotation
/// threshold) are served without loading the full content into RAM.
/// Returns 404 if the log file does not yet exist.
pub async fn audit_raw_handler(_state: State<AppState>) -> impl IntoResponse {
    let path = audit_log_path();
    match tokio::fs::File::open(&path).await {
        Ok(file) => {
            let stream  = ReaderStream::new(file);
            let body    = Body::from_stream(stream);
            axum::http::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(body)
                .unwrap()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            axum::http::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from("audit log not found\n"))
                .unwrap()
        }
        Err(e) => {
            axum::http::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from(format!("error reading audit log: {e}\n")))
                .unwrap()
        }
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
