use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

// ── Data directory & audit log path ──────────────────────────────────────────

/// Root directory for all AxiomLab persistent data.
///
/// Controlled by `AXIOMLAB_DATA_DIR` (default: `.artifacts`).
/// All audit logs, discovery journals, and proof artifacts are anchored here.
pub fn data_dir() -> PathBuf {
    std::env::var("AXIOMLAB_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".artifacts"))
}

/// Default path for the active audit log file.
///
/// Overridden by `AXIOMLAB_AUDIT_LOG`.
pub fn audit_log_path() -> PathBuf {
    std::env::var("AXIOMLAB_AUDIT_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir().join("audit").join("runtime_audit.jsonl"))
}

/// Rotate the audit log if it exceeds 100 MB or was last written on a previous day.
///
/// The active file is renamed to `runtime_audit_YYYY-MM-DD[_N].jsonl` and a
/// fresh file is started.  Returns the archived path if a rotation happened.
pub fn rotate_if_needed(path: &Path) -> std::io::Result<Option<PathBuf>> {
    const MAX_BYTES: u64 = 100 * 1024 * 1024; // 100 MB

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(None), // file doesn't exist yet — nothing to rotate
    };

    if meta.len() == 0 {
        return Ok(None);
    }

    let needs_rotation = meta.len() > MAX_BYTES || {
        // Check if the file was last written before today (UTC).
        let modified = meta.modified()?;
        let modified_secs = modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Seconds since midnight UTC for each timestamp.
        modified_secs / 86400 < now_secs / 86400
    };

    if !needs_rotation {
        return Ok(None);
    }

    // Build archive name: runtime_audit_YYYY-MM-DD[_N].jsonl
    let parent = path.parent().unwrap_or(Path::new("."));
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d) = unix_secs_to_ymd(now_secs);
    let base = format!("runtime_audit_{y:04}-{mo:02}-{d:02}");
    let mut archive = parent.join(format!("{base}.jsonl"));
    let mut n = 1u32;
    while archive.exists() {
        archive = parent.join(format!("{base}_{n}.jsonl"));
        n += 1;
    }
    std::fs::rename(path, &archive)?;
    tracing::info!(archived = %archive.display(), "Audit log rotated");
    Ok(Some(archive))
}

fn unix_secs_to_ymd(secs: u64) -> (u32, u32, u32) {
    // Minimal UTC date calculation (no external crate needed).
    let days = (secs / 86400) as u32;
    // Gregorian calendar from Julian Day Number.
    let z = days + 2440588; // Unix epoch is JDN 2440588
    let h = 4 * z + 3;
    let c = h / 146097;
    let r = h % 146097 / 4;
    let n = 5 * r + 2;
    let d = n % 153 / 5 + 1;
    let m = n / 153 % 12 + 3;
    let y = 100 * c + r / 365 - if m > 12 { 4699 } else { 4700 };
    let m = if m > 12 { m - 12 } else { m };
    (y, m, d)
}

/// Emit a `session_start` entry that links this process's signing key to the
/// previous chain, maintaining cross-restart continuity.
///
/// The `prev_tail_hash` is the `entry_hash` of the last line in the previous
/// (or current) audit file — proving the new session continues the same chain.
pub fn emit_session_start(
    path: &str,
    session_id: &str,
    pubkey_b64: &str,
    git_commit: &str,
    signer: Option<&AuditSigner>,
) -> Result<String, std::io::Error> {
    let details = serde_json::json!({
        "session_id":  session_id,
        "pubkey_b64":  pubkey_b64,
        "git_commit":  git_commit,
    });
    let event = AuditEvent {
        unix_secs: unix_secs_now(),
        trace_id:  format!("session_start-{session_id}"),
        action:    "session_start".into(),
        decision:  "allow".into(),
        reason:    details.to_string(),
        success:   true,
        approval_ids: None,
    };
    emit_jsonl(path, &event, signer)
}

/// Anchor the current chain tip to Sigstore Rekor.
///
/// Reads the last `entry_hash` from the audit file, signs it with the audit
/// signer, and submits a `hashedrekord` entry to Rekor.  On success, writes
/// a `rekor_checkpoint` audit entry containing the UUID and log index so the
/// anchor is itself part of the verifiable chain.
///
/// This is best-effort: failures are logged as warnings and do not affect the
/// local chain.
pub async fn anchor_chain_tip_to_rekor(path: &str, signer: &AuditSigner) {
    let tip = match last_entry_hash(path) {
        Ok(Some(h)) => h,
        Ok(None) => {
            tracing::debug!("rekor checkpoint: audit file empty, skipping");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "rekor checkpoint: failed to read chain tip");
            return;
        }
    };

    let sig_b64  = signer.sign(tip.as_bytes());
    let pubkey   = crate::rekor::ed25519_pubkey_pem(&signer.verifying_key_bytes());

    match crate::rekor::anchor(&tip, &sig_b64, &pubkey).await {
        Ok(anchor) => {
            tracing::info!(
                uuid       = %anchor.uuid,
                log_index  = anchor.log_index,
                chain_tip  = %tip,
                "Rekor checkpoint anchored"
            );
            // Record the anchor in the local chain so verifiers can correlate.
            let details = serde_json::json!({
                "chain_tip_hash": tip,
                "rekor_uuid":     anchor.uuid,
                "log_index":      anchor.log_index,
                "integrated_time": anchor.integrated_time,
            });
            let event = AuditEvent {
                unix_secs:    unix_secs_now(),
                trace_id:     format!("rekor_checkpoint-{}", anchor.log_index),
                action:       "rekor_checkpoint".into(),
                decision:     "allow".into(),
                reason:       details.to_string(),
                success:      true,
                approval_ids: None,
            };
            emit_jsonl(path, &event, Some(signer)).ok();
        }
        Err(e) => {
            tracing::warn!(error = %e, "Rekor checkpoint failed — local chain intact");
        }
    }
}

// ── Protocol audit helpers ────────────────────────────────────────────────────

/// Emit a protocol step record into the audit chain.
///
/// Protocol step records use `action = "protocol_step"` and encode structured
/// details as JSON in the `reason` field.  This keeps them in the same hash
/// chain as tool-call audit events without changing the chain verification code.
pub fn emit_protocol_step(
    path: &str,
    protocol_id: Uuid,
    run_id: Uuid,
    step_index: usize,
    tool: &str,
    description: &str,
    allowed: bool,
    rejection_reason: Option<&str>,
    proof_artifact_hash: &str,
    signer: Option<&AuditSigner>,
) -> Result<String, std::io::Error> {
    let details = serde_json::json!({
        "protocol_id": protocol_id,
        "run_id": run_id,
        "step_index": step_index,
        "tool": tool,
        "description": description,
        "proof_artifact_hash": proof_artifact_hash,
        "rejection_reason": rejection_reason,
    });
    let event = AuditEvent {
        unix_secs: unix_secs_now(),
        trace_id: format!("protocol_step-{run_id}-{step_index}"),
        action: "protocol_step".into(),
        decision: if allowed { "allow" } else { "deny" }.into(),
        reason: details.to_string(),
        success: allowed,
        approval_ids: None,
    };
    emit_jsonl(path, &event, signer)
}

/// Emit a signed protocol conclusion record into the audit chain.
///
/// The conclusion text is signed separately (over `run_id + conclusion`) in
/// addition to the standard per-entry Ed25519 signature, giving an independent
/// attestation that this specific scientific conclusion was produced for this run.
///
/// Returns the conclusion signature as a base64 string (empty if no signer).
pub fn emit_protocol_conclusion(
    path: &str,
    protocol_id: Uuid,
    run_id: Uuid,
    protocol_name: &str,
    conclusion: &str,
    steps_total: usize,
    steps_succeeded: usize,
    signer: Option<&AuditSigner>,
) -> Result<String, std::io::Error> {
    // Conclusion-specific signature: sign (run_id || "\n" || conclusion).
    let conclusion_sig = signer.map(|s| {
        let msg = format!("{run_id}\n{conclusion}");
        s.sign(msg.as_bytes())
    }).unwrap_or_default();

    let details = serde_json::json!({
        "protocol_id": protocol_id,
        "run_id": run_id,
        "protocol_name": protocol_name,
        "steps_total": steps_total,
        "steps_succeeded": steps_succeeded,
        "conclusion_sig_b64": conclusion_sig,
    });
    let event = AuditEvent {
        unix_secs: unix_secs_now(),
        trace_id: format!("protocol_conclusion-{run_id}"),
        action: "protocol_conclusion".into(),
        decision: "allow".into(),
        reason: details.to_string(),
        success: true,
        approval_ids: None,
    };
    let line = emit_jsonl(path, &event, signer)?;
    Ok(line)
}

/// Emit a journal finding record into the audit chain.
///
/// Called when the LLM records a confirmed scientific finding via `update_journal`.
/// The finding statement and evidence are part of the signed chain, giving the
/// scientific record the same tamper-evidence as hardware actions.
pub fn emit_journal_finding(
    path: &str,
    finding_id: &str,
    statement: &str,
    evidence: &str,
    signer: Option<&AuditSigner>,
) -> Result<String, std::io::Error> {
    let details = serde_json::json!({
        "finding_id": finding_id,
        "statement": statement,
        "evidence": evidence,
    });
    let event = AuditEvent {
        unix_secs: unix_secs_now(),
        trace_id: format!("journal_finding-{finding_id}"),
        action: "journal_finding".into(),
        decision: "allow".into(),
        reason: details.to_string(),
        success: true,
        approval_ids: None,
    };
    emit_jsonl(path, &event, signer)
}

/// Emit a journal hypothesis update record into the audit chain.
///
/// Called when the LLM adds or changes the status of a hypothesis via
/// `update_journal`.  Tracks the full lifecycle: Proposed → Testing →
/// Confirmed/Rejected.
pub fn emit_journal_hypothesis(
    path: &str,
    hypothesis_id: &str,
    statement: &str,
    status: &str,
    signer: Option<&AuditSigner>,
) -> Result<String, std::io::Error> {
    let details = serde_json::json!({
        "hypothesis_id": hypothesis_id,
        "statement": statement,
        "status": status,
    });
    let event = AuditEvent {
        unix_secs: unix_secs_now(),
        trace_id: format!("journal_hypothesis-{hypothesis_id}"),
        action: "journal_hypothesis".into(),
        decision: "allow".into(),
        reason: details.to_string(),
        success: true,
        approval_ids: None,
    };
    emit_jsonl(path, &event, signer)
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

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

    /// Raw 32-byte Ed25519 verifying (public) key — used to build a PEM for Rekor.
    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Base64-encoded public key string, e.g. for embedding in audit records.
    pub fn public_key_b64(&self) -> &str {
        &self.public_key_b64
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
