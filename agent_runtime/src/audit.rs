use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{Duration, sleep};

#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub unix_secs: u64,
    pub trace_id: String,
    pub action: String,
    pub decision: String,
    pub reason: String,
    pub success: bool,
}

#[derive(Debug, Clone)]
struct RemoteAuditConfig {
    url: String,
    bearer_token: Option<String>,
    retries: u32,
    backoff_ms: u64,
    timeout_ms: u64,
}

#[derive(Debug, Serialize)]
struct PersistedAuditEvent<'a> {
    unix_secs: u64,
    trace_id: &'a str,
    action: &'a str,
    decision: &'a str,
    reason: &'a str,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_hash: Option<&'a str>,
    entry_hash: String,
}

#[derive(Debug, Serialize)]
struct HashInput<'a> {
    unix_secs: u64,
    trace_id: &'a str,
    action: &'a str,
    decision: &'a str,
    reason: &'a str,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_hash: Option<&'a str>,
}

pub fn emit_jsonl(path: &str, event: &AuditEvent) -> Result<String, std::io::Error> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let prev_hash = last_entry_hash(path)?;
    let entry_hash = compute_entry_hash(event, prev_hash.as_deref())?;
    let persisted = PersistedAuditEvent {
        unix_secs: event.unix_secs,
        trace_id: &event.trace_id,
        action: &event.action,
        decision: &event.decision,
        reason: &event.reason,
        success: event.success,
        prev_hash: prev_hash.as_deref(),
        entry_hash,
    };

    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(&persisted)
        .map_err(|e| std::io::Error::other(format!("serialize audit event: {e}")))?;
    writeln!(f, "{}", line)?;
    Ok(line)
}

pub async fn emit_remote_with_retry(payload_line: &str) -> Result<(), String> {
    let Some(cfg) = remote_config_from_env() else {
        return Ok(());
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(cfg.timeout_ms))
        .build()
        .map_err(|e| format!("build audit HTTP client: {e}"))?;

    for attempt in 0..=cfg.retries {
        let mut req = client
            .post(&cfg.url)
            .header("content-type", "application/json")
            .body(payload_line.to_owned());
        if let Some(token) = &cfg.bearer_token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                if attempt == cfg.retries {
                    return Err(format!(
                        "remote audit sink returned HTTP {} after {} attempts",
                        resp.status(),
                        cfg.retries + 1
                    ));
                }
            }
            Err(e) => {
                if attempt == cfg.retries {
                    return Err(format!(
                        "remote audit sink request failed after {} attempts: {}",
                        cfg.retries + 1,
                        e
                    ));
                }
            }
        }

        let backoff = cfg.backoff_ms.saturating_mul((attempt + 1) as u64);
        sleep(Duration::from_millis(backoff)).await;
    }

    Ok(())
}

pub fn verify_chain(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read audit log {path}: {e}"))?;

    let mut expected_prev: Option<String> = None;
    for (idx, line) in content.lines().enumerate() {
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("parse audit line {}: {e}", idx + 1))?;

        let entry_hash = value
            .get("entry_hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("line {} missing entry_hash", idx + 1))?;

        let prev_hash = value
            .get("prev_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if prev_hash != expected_prev {
            return Err(format!(
                "line {} prev_hash mismatch: expected {:?}, found {:?}",
                idx + 1,
                expected_prev,
                prev_hash
            ));
        }

        let event = AuditEvent {
            unix_secs: value
                .get("unix_secs")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| format!("line {} missing unix_secs", idx + 1))?,
            trace_id: value
                .get("trace_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("line {} missing trace_id", idx + 1))?
                .to_string(),
            action: value
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("line {} missing action", idx + 1))?
                .to_string(),
            decision: value
                .get("decision")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("line {} missing decision", idx + 1))?
                .to_string(),
            reason: value
                .get("reason")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("line {} missing reason", idx + 1))?
                .to_string(),
            success: value
                .get("success")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| format!("line {} missing success", idx + 1))?,
        };

        let recomputed = compute_entry_hash(&event, prev_hash.as_deref())
            .map_err(|e| format!("line {} hash computation failed: {e}", idx + 1))?;
        if recomputed != entry_hash {
            return Err(format!(
                "line {} entry_hash mismatch: expected {}, found {}",
                idx + 1,
                recomputed,
                entry_hash
            ));
        }

        expected_prev = Some(entry_hash.to_string());
    }

    Ok(())
}

fn last_entry_hash(path: &str) -> Result<Option<String>, std::io::Error> {
    if !Path::new(path).exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path)?;
    let Some(last_line) = content.lines().last() else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(last_line)
        .map_err(|e| std::io::Error::other(format!("parse previous audit line: {e}")))?;
    let hash = value
        .get("entry_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| std::io::Error::other("previous audit line missing entry_hash"))?;
    Ok(Some(hash.to_string()))
}

fn compute_entry_hash(event: &AuditEvent, prev_hash: Option<&str>) -> Result<String, std::io::Error> {
    let payload = HashInput {
        unix_secs: event.unix_secs,
        trace_id: &event.trace_id,
        action: &event.action,
        decision: &event.decision,
        reason: &event.reason,
        success: event.success,
        prev_hash,
    };
    let canonical = serde_json::to_vec(&payload)
        .map_err(|e| std::io::Error::other(format!("serialize hash payload: {e}")))?;
    let digest = Sha256::digest(&canonical);
    Ok(format!("{:x}", digest))
}

pub fn trace_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{}", prefix, nanos)
}

fn remote_config_from_env() -> Option<RemoteAuditConfig> {
    let url = std::env::var("AXIOMLAB_AUDIT_REMOTE_URL").ok()?;
    let retries = std::env::var("AXIOMLAB_AUDIT_REMOTE_RETRIES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(3);
    let backoff_ms = std::env::var("AXIOMLAB_AUDIT_REMOTE_BACKOFF_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(200);
    let timeout_ms = std::env::var("AXIOMLAB_AUDIT_REMOTE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(2000);

    Some(RemoteAuditConfig {
        url,
        bearer_token: std::env::var("AXIOMLAB_AUDIT_REMOTE_TOKEN").ok(),
        retries,
        backoff_ms,
        timeout_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_chain_verifies_and_tamper_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.jsonl");
        let path_str = path.to_string_lossy().to_string();

        emit_jsonl(
            &path_str,
            &AuditEvent {
                unix_secs: 1,
                trace_id: "t-1".into(),
                action: "read_sensor".into(),
                decision: "allow".into(),
                reason: "ok".into(),
                success: true,
            },
        )
        .expect("emit first");

        emit_jsonl(
            &path_str,
            &AuditEvent {
                unix_secs: 2,
                trace_id: "t-2".into(),
                action: "move_arm".into(),
                decision: "deny".into(),
                reason: "policy".into(),
                success: false,
            },
        )
        .expect("emit second");

        verify_chain(&path_str).expect("valid chain");

        let original = std::fs::read_to_string(&path).expect("read chain");
        let tampered = original.replacen("\"reason\":\"policy\"", "\"reason\":\"tampered\"", 1);
        std::fs::write(&path, tampered).expect("write tampered chain");

        assert!(verify_chain(&path_str).is_err());
    }
}
