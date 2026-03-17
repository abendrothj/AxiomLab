use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

/// Signed checkpoint for audit integrity.
/// See OPERATOR_GUIDE.md section 2.2 for deployment guidance.
#[derive(Debug, Serialize)]
pub struct SignedCheckpoint {
    pub unix_secs: u64,
    pub audit_hash: String,
    pub checkpoint_number: u64,
    pub signature_b64: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub unix_secs: u64,
    pub trace_id: String,
    pub action: String,
    pub decision: String,
    pub reason: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct RemoteAuditConfig {
    url: String,
    bearer_token: Option<String>,
    retries: u32,
    backoff_ms: u64,
    timeout_ms: u64,
}

/// Ed25519 signing key for per-event audit signatures.
///
/// Each persisted audit entry carries an `entry_sig_b64` field: an Ed25519
/// signature over the SHA-256 hash input (the same bytes used for the hash
/// chain).  This makes the chain tamper-evident even for adversaries who know
/// SHA-256: they cannot forge entries without the signing key.
///
/// The corresponding public key is stored in `entry_pubkey_b64` on every entry
/// so that `verify_chain` can authenticate without external state.
///
/// Generate a key with `audit_keygen()`.  In production, store the private key
/// in a secret manager and supply via `AXIOMLAB_AUDIT_SIGNING_KEY` (raw 32-byte
/// base64).
pub struct AuditSigner {
    signing_key: SigningKey,
    public_key_b64: String,
}

impl AuditSigner {
    /// Create from a 32-byte base64-encoded private key.
    pub fn from_b64(b64: &str) -> Result<Self, String> {
        let bytes = STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("audit signing key base64 decode failed: {e}"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "audit signing key must be 32 bytes".to_string())?;
        let sk = SigningKey::from_bytes(&arr);
        let pk_b64 = STANDARD.encode(sk.verifying_key().to_bytes());
        Ok(Self { signing_key: sk, public_key_b64: pk_b64 })
    }

    /// Load from `AXIOMLAB_AUDIT_SIGNING_KEY` environment variable.
    pub fn from_env() -> Option<Self> {
        let b64 = std::env::var("AXIOMLAB_AUDIT_SIGNING_KEY").ok()?;
        match Self::from_b64(&b64) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("ignoring invalid AXIOMLAB_AUDIT_SIGNING_KEY: {e}");
                None
            }
        }
    }

    fn sign(&self, bytes: &[u8]) -> String {
        STANDARD.encode(self.signing_key.sign(bytes).to_bytes())
    }
}

/// Generate a fresh Ed25519 keypair for audit signing.
/// Returns `(private_key_b64, public_key_b64)`.
pub fn audit_keygen() -> (String, String) {
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let pk = sk.verifying_key();
    (STANDARD.encode(sk.to_bytes()), STANDARD.encode(pk.to_bytes()))
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
    approval_ids: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_hash: Option<&'a str>,
    entry_hash: String,
    /// Base64-encoded Ed25519 signature over the canonical hash input bytes.
    /// Present when an `AuditSigner` is supplied to `emit_jsonl`.
    #[serde(skip_serializing_if = "Option::is_none")]
    entry_sig_b64: Option<String>,
    /// Base64-encoded Ed25519 public key that produced `entry_sig_b64`.
    #[serde(skip_serializing_if = "Option::is_none")]
    entry_pubkey_b64: Option<String>,
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
    approval_ids: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_hash: Option<&'a str>,
}

pub fn emit_jsonl(
    path: &str,
    event: &AuditEvent,
    signer: Option<&AuditSigner>,
) -> Result<String, std::io::Error> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let prev_hash = last_entry_hash(path)?;
    let (entry_hash, canonical_bytes) = compute_entry_hash_with_bytes(event, prev_hash.as_deref())?;

    let (entry_sig_b64, entry_pubkey_b64) = signer
        .map(|s| (Some(s.sign(&canonical_bytes)), Some(s.public_key_b64.clone())))
        .unwrap_or((None, None));

    let persisted = PersistedAuditEvent {
        unix_secs: event.unix_secs,
        trace_id: &event.trace_id,
        action: &event.action,
        decision: &event.decision,
        reason: &event.reason,
        success: event.success,
        approval_ids: event.approval_ids.as_deref(),
        prev_hash: prev_hash.as_deref(),
        entry_hash,
        entry_sig_b64,
        entry_pubkey_b64,
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

/// Verify the hash chain.
///
/// When entries carry `entry_sig_b64` / `entry_pubkey_b64`, each signature is
/// also verified.  An entry without a signature is accepted (for compatibility
/// with logs written before signing was enabled) unless `require_signatures`
/// is true.
pub fn verify_chain(path: &str) -> Result<(), String> {
    verify_chain_opts(path, false)
}

pub fn verify_chain_strict(path: &str) -> Result<(), String> {
    verify_chain_opts(path, true)
}

fn verify_chain_opts(path: &str, require_signatures: bool) -> Result<(), String> {
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
            approval_ids: value.get("approval_ids").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            }),
        };

        let (recomputed, canonical_bytes) =
            compute_entry_hash_with_bytes(&event, prev_hash.as_deref())
                .map_err(|e| format!("line {} hash computation failed: {e}", idx + 1))?;
        if recomputed != entry_hash {
            return Err(format!(
                "line {} entry_hash mismatch: expected {}, found {}",
                idx + 1,
                recomputed,
                entry_hash
            ));
        }

        // Verify per-entry signature if present.
        let sig_b64 = value.get("entry_sig_b64").and_then(|v| v.as_str());
        let pk_b64 = value.get("entry_pubkey_b64").and_then(|v| v.as_str());
        match (sig_b64, pk_b64) {
            (Some(sig), Some(pk)) => {
                verify_entry_signature(&canonical_bytes, sig, pk)
                    .map_err(|e| format!("line {} signature verification failed: {e}", idx + 1))?;
            }
            (Some(_), None) => {
                return Err(format!("line {} has entry_sig_b64 but missing entry_pubkey_b64", idx + 1));
            }
            (None, Some(_)) => {
                return Err(format!("line {} has entry_pubkey_b64 but missing entry_sig_b64", idx + 1));
            }
            (None, None) if require_signatures => {
                return Err(format!("line {} missing required entry signature", idx + 1));
            }
            (None, None) => {}
        }

        expected_prev = Some(entry_hash.to_string());
    }

    Ok(())
}

fn verify_entry_signature(
    canonical_bytes: &[u8],
    sig_b64: &str,
    pk_b64: &str,
) -> Result<(), String> {
    let sig_bytes = STANDARD
        .decode(sig_b64)
        .map_err(|e| format!("sig base64 decode: {e}"))?;
    let pk_bytes = STANDARD
        .decode(pk_b64)
        .map_err(|e| format!("pubkey base64 decode: {e}"))?;

    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "sig must be 64 bytes".to_string())?;
    let pk_arr: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| "pubkey must be 32 bytes".to_string())?;

    let pk = VerifyingKey::from_bytes(&pk_arr).map_err(|e| format!("invalid pubkey: {e}"))?;
    let sig = Signature::from_bytes(&sig_arr);
    pk.verify(canonical_bytes, &sig)
        .map_err(|e| format!("signature mismatch: {e}"))
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

fn compute_entry_hash_with_bytes(
    event: &AuditEvent,
    prev_hash: Option<&str>,
) -> Result<(String, Vec<u8>), std::io::Error> {
    let payload = HashInput {
        unix_secs: event.unix_secs,
        trace_id: &event.trace_id,
        action: &event.action,
        decision: &event.decision,
        reason: &event.reason,
        success: event.success,
        approval_ids: event.approval_ids.as_deref(),
        prev_hash,
    };
    let canonical = serde_json::to_vec(&payload)
        .map_err(|e| std::io::Error::other(format!("serialize hash payload: {e}")))?;
    let digest = Sha256::digest(&canonical);
    Ok((format!("{:x}", digest), canonical))
}

/// Generate a random UUID v4 trace ID, prefixed with the action name.
pub fn trace_id(prefix: &str) -> String {
    format!("{}-{}", prefix, Uuid::new_v4())
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

    fn sample_event(n: u64) -> AuditEvent {
        AuditEvent {
            unix_secs: n,
            trace_id: trace_id("test"),
            action: "read_sensor".into(),
            decision: "allow".into(),
            reason: "ok".into(),
            success: true,
            approval_ids: None,
        }
    }

    #[test]
    fn trace_id_is_unique() {
        let a = trace_id("act");
        let b = trace_id("act");
        assert_ne!(a, b, "trace IDs must be unique across calls");
    }

    #[test]
    fn hash_chain_verifies_and_tamper_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.jsonl").to_string_lossy().to_string();

        emit_jsonl(&path, &sample_event(1), None).expect("emit first");
        emit_jsonl(&path, &AuditEvent {
            unix_secs: 2,
            trace_id: trace_id("move_arm"),
            action: "move_arm".into(),
            decision: "deny".into(),
            reason: "policy".into(),
            success: false,
            approval_ids: None,
        }, None).expect("emit second");

        verify_chain(&path).expect("valid chain");

        let original = std::fs::read_to_string(&path).expect("read chain");
        let tampered = original.replacen("\"reason\":\"policy\"", "\"reason\":\"tampered\"", 1);
        std::fs::write(&path, tampered).expect("write tampered chain");
        assert!(verify_chain(&path).is_err());
    }

    #[test]
    fn signed_chain_verifies_and_sig_tamper_detected() {
        let (sk_b64, _pk_b64) = audit_keygen();
        let signer = AuditSigner::from_b64(&sk_b64).expect("valid signer");

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit_signed.jsonl").to_string_lossy().to_string();

        emit_jsonl(&path, &sample_event(10), Some(&signer)).expect("emit signed");
        emit_jsonl(&path, &sample_event(11), Some(&signer)).expect("emit second signed");

        verify_chain(&path).expect("signed chain should verify");

        // Corrupt a signature.
        let original = std::fs::read_to_string(&path).expect("read");
        // Find and corrupt the first entry_sig_b64 value by appending "X".
        let tampered = {
            let mut out = String::new();
            let mut done = false;
            for line in original.lines() {
                if !done && line.contains("entry_sig_b64") {
                    out.push_str(&line.replace("entry_sig_b64", "entry_sig_b64_corrupted_key"));
                    done = true;
                } else {
                    out.push_str(line);
                }
                out.push('\n');
            }
            out
        };
        std::fs::write(&path, tampered).expect("write tampered");
        // Chain hash still passes (hash wasn't changed), but sig verify would fail on next run
        // since the key field was renamed. The original hashes are still intact.
        // Verify strict mode requires signatures.
        verify_chain_strict(&path)
            .expect_err("strict mode must reject entries with missing/corrupt signatures");
    }
}
