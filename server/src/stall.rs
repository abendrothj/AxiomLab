//! Startup stall detection for approval sidecars.
//!
//! On startup, each `.json` file in `.artifacts/approvals/` represents a
//! pending approval that was enqueued in a previous run.  We cross-reference
//! each sidecar against the audit log:
//!
//! - If a `dispatch_complete` entry exists for that `approval_id`: the
//!   dispatch completed normally and the sidecar is an orphan — delete it.
//! - If no `dispatch_complete` exists: the dispatch stalled (process crashed
//!   between approval and dispatch, or between dispatch and dispatch_complete).
//!   Emit a `stalled_dispatch` audit entry and add the id to the stalled list.
//!
//! The returned list of stalled IDs drives the `AppState::stalled` flag and
//! the operator recovery endpoints.

use agent_runtime::audit::AuditSigner;

/// Detect stalled approval sidecars from a previous run.
///
/// Returns a list of `approval_id` strings that have a sidecar but no
/// `dispatch_complete` audit entry.  Sidecars that do have a corresponding
/// `dispatch_complete` are silently deleted.
pub fn detect_stalled_approvals(
    audit_path: &str,
    signer: Option<&dyn AuditSigner>,
) -> Vec<String> {
    let dir = agent_runtime::audit::data_dir().join("approvals");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    // Read the audit log once and collect all dispatch_complete approval_ids.
    let completed_ids = collect_completed_ids(audit_path);

    let mut stalled = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        // Parse the sidecar to get approval_id and tool_name.
        let (approval_id, tool_name) = match read_sidecar(&path) {
            Some(pair) => pair,
            None => {
                tracing::warn!(
                    path = %path.display(),
                    "Could not parse approval sidecar — removing it"
                );
                std::fs::remove_file(&path).ok();
                continue;
            }
        };

        if completed_ids.contains(&approval_id) {
            // Clean completion: dispatch_complete was written; sidecar is orphaned.
            tracing::debug!(
                approval_id,
                "Approval sidecar matches dispatch_complete — removing orphan"
            );
            std::fs::remove_file(&path).ok();
        } else {
            // Stalled: no dispatch_complete found for this approval_id.
            tracing::warn!(
                approval_id,
                tool = tool_name,
                "Stalled dispatch detected: sidecar present without dispatch_complete"
            );
            agent_runtime::audit::emit_stalled_dispatch(
                audit_path,
                &approval_id,
                &tool_name,
                "process crashed before dispatch_complete was written",
                signer,
            )
            .ok();
            stalled.push(approval_id);
        }
    }

    stalled
}

/// Parse an approval sidecar file and return `(pending_id, tool_name)`.
fn read_sidecar(path: &std::path::Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let id = v.get("pending_id")?.as_str()?.to_string();
    let tool = v.get("tool_name")?.as_str()?.to_string();
    Some((id, tool))
}

/// Read the audit JSONL and collect all `approval_id` values from
/// `dispatch_complete` entries.
fn collect_completed_ids(audit_path: &str) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    let content = match std::fs::read_to_string(audit_path) {
        Ok(c) => c,
        Err(_) => return ids,
    };
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("action").and_then(|a| a.as_str()) != Some("dispatch_complete") {
            continue;
        }
        // The approval_id is stored in the `approval_ids` array.
        if let Some(arr) = v.get("approval_ids").and_then(|a| a.as_array()) {
            for id in arr {
                if let Some(s) = id.as_str() {
                    ids.insert(s.to_string());
                }
            }
        }
    }
    ids
}
