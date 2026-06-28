//! The Ed25519 hash-chained, append-only audit log.

use crate::signer::Signer;
use axiom_types::Action;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ChainError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("chain integrity violation at line {line}: {detail}")]
    Integrity { line: usize, detail: String },
}

/// The mutable fields of one audit entry — everything that gets hashed and signed.
///
/// `unix_secs`, `trace_id` default to now / a generated id. The chain fills in
/// `prev_hash`, `entry_hash`, and the signature when [`Chain::append`] is called.
#[derive(Debug, Clone, Serialize)]
pub struct EntryData {
    pub unix_secs: u64,
    pub trace_id: String,
    pub action: String,
    /// `"allow"` or `"deny"`.
    pub decision: String,
    pub reason: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_text: Option<String>,
    /// Rekor entry UUID — metadata only, NOT part of the hash input.
    #[serde(skip)]
    pub rekor_uuid: Option<String>,
}

impl EntryData {
    /// A new entry stamped with the current time and a generated trace id.
    pub fn new(
        action: impl Into<String>,
        decision: impl Into<String>,
        reason: impl Into<String>,
        success: bool,
    ) -> Self {
        let action = action.into();
        Self {
            unix_secs: now_secs(),
            trace_id: format!("{action}-{}", Uuid::new_v4()),
            action,
            decision: decision.into(),
            reason: reason.into(),
            success,
            approval_ids: None,
            reasoning_text: None,
            rekor_uuid: None,
        }
    }

    /// Build an entry from a gate decision on an [`Action`].
    ///
    /// The params are recorded in the `reason` JSON. Callers that handle
    /// sensitive values should redact `action.params` before constructing this.
    pub fn from_action(action: &Action, allowed: bool, detail: impl Into<String>) -> Self {
        let reason = serde_json::json!({
            "tool": action.tool,
            "params": action.params,
            "detail": detail.into(),
        })
        .to_string();
        Self::new(
            action.tool.clone(),
            if allowed { "allow" } else { "deny" },
            reason,
            allowed,
        )
    }

    pub fn with_approval_ids(mut self, ids: Vec<String>) -> Self {
        self.approval_ids = Some(ids);
        self
    }
    pub fn with_reasoning_text(mut self, text: impl Into<String>) -> Self {
        self.reasoning_text = Some(text.into());
        self
    }
    pub fn with_rekor_uuid(mut self, uuid: impl Into<String>) -> Self {
        self.rekor_uuid = Some(uuid.into());
        self
    }
}

/// A fully persisted chain entry: [`EntryData`] plus chain links and signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainEntry {
    pub unix_secs: u64,
    pub trace_id: String,
    pub action: String,
    pub decision: String,
    pub reason: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub approval_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prev_hash: Option<String>,
    pub entry_hash: String,
    pub entry_sig_b64: String,
    pub entry_pubkey_b64: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rekor_uuid: Option<String>,
}

/// Outcome of a successful [`Chain::verify`].
#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub entries_checked: usize,
    pub signatures_verified: usize,
    pub tip_hash_hex: Option<String>,
}

/// An append-only, Ed25519-signed hash chain backed by a JSONL file.
///
/// Cloneable handles share the same underlying file and append lock.
#[derive(Clone)]
pub struct Chain {
    path: PathBuf,
    lock: std::sync::Arc<Mutex<()>>,
}

impl Chain {
    /// Open (or lazily create) a chain at `path`.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), lock: std::sync::Arc::new(Mutex::new(())) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a signed entry, linking it to the current tip. Returns the
    /// persisted [`ChainEntry`].
    pub fn append(&self, entry: EntryData, signer: &dyn Signer) -> Result<ChainEntry, ChainError> {
        let _guard = self.lock.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let prev_hash = self.last_entry_hash()?;
        let (entry_hash, canonical) = compute_hash(&entry, prev_hash.as_deref())?;
        let sig = signer.sign(&canonical);

        let persisted = ChainEntry {
            unix_secs: entry.unix_secs,
            trace_id: entry.trace_id,
            action: entry.action,
            decision: entry.decision,
            reason: entry.reason,
            success: entry.success,
            approval_ids: entry.approval_ids,
            reasoning_text: entry.reasoning_text,
            prev_hash,
            entry_hash,
            entry_sig_b64: STANDARD.encode(&sig),
            entry_pubkey_b64: STANDARD.encode(signer.public_key()),
            rekor_uuid: entry.rekor_uuid,
        };

        let line = serde_json::to_string(&persisted)?;
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{line}")?;
        Ok(persisted)
    }

    /// Walk the whole chain, verifying every hash link and every signature.
    pub fn verify(&self) -> Result<VerifyResult, ChainError> {
        if !self.path.exists() {
            return Ok(VerifyResult { entries_checked: 0, signatures_verified: 0, tip_hash_hex: None });
        }
        let content = std::fs::read_to_string(&self.path)?;
        let mut expected_prev: Option<String> = None;
        let mut checked = 0usize;
        let mut sigs = 0usize;
        let mut tip = None;

        for (idx, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let line_no = idx + 1;
            let e: ChainEntry = serde_json::from_str(line).map_err(|err| ChainError::Integrity {
                line: line_no,
                detail: format!("parse: {err}"),
            })?;

            if e.prev_hash != expected_prev {
                return Err(ChainError::Integrity {
                    line: line_no,
                    detail: format!("prev_hash mismatch: expected {expected_prev:?}, found {:?}", e.prev_hash),
                });
            }

            let data = entry_to_data(&e);
            let (recomputed, canonical) = compute_hash(&data, e.prev_hash.as_deref())
                .map_err(|err| ChainError::Integrity { line: line_no, detail: format!("hash: {err}") })?;
            if recomputed != e.entry_hash {
                return Err(ChainError::Integrity {
                    line: line_no,
                    detail: format!("entry_hash mismatch: recomputed {recomputed}, stored {}", e.entry_hash),
                });
            }

            verify_signature(&canonical, &e.entry_sig_b64, &e.entry_pubkey_b64).map_err(|err| {
                ChainError::Integrity { line: line_no, detail: format!("signature: {err}") }
            })?;
            sigs += 1;

            expected_prev = Some(e.entry_hash.clone());
            tip = Some(e.entry_hash);
            checked += 1;
        }

        Ok(VerifyResult { entries_checked: checked, signatures_verified: sigs, tip_hash_hex: tip })
    }

    /// The hex string of the current chain-tip hash, or `None` if empty.
    pub fn tip_hash_hex(&self) -> Result<Option<String>, ChainError> {
        self.last_entry_hash()
    }

    /// The current chain-tip hash as raw bytes, or `None` if empty.
    pub fn tip_hash(&self) -> Result<Option<[u8; 32]>, ChainError> {
        match self.last_entry_hash()? {
            None => Ok(None),
            Some(hex_str) => {
                let bytes = hex::decode(&hex_str)
                    .map_err(|e| ChainError::Integrity { line: 0, detail: format!("tip hex decode: {e}") })?;
                let arr: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| ChainError::Integrity { line: 0, detail: "tip hash not 32 bytes".into() })?;
                Ok(Some(arr))
            }
        }
    }

    /// Read all entries (for server-side query / summary derivation).
    pub fn entries(&self) -> Result<Vec<ChainEntry>, ChainError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.path)?;
        let mut out = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line)?);
        }
        Ok(out)
    }

    fn last_entry_hash(&self) -> Result<Option<String>, ChainError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&self.path)?;
        let Some(last) = content.lines().filter(|l| !l.trim().is_empty()).next_back() else {
            return Ok(None);
        };
        let v: serde_json::Value = serde_json::from_str(last)?;
        Ok(v.get("entry_hash").and_then(|h| h.as_str()).map(|s| s.to_string()))
    }
}

// ── Hashing helpers ────────────────────────────────────────────────────────

#[derive(Serialize)]
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
    reasoning_text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_hash: Option<&'a str>,
}

/// Returns `(hex_hash, canonical_bytes)`. The signature is over `canonical_bytes`.
fn compute_hash(e: &EntryData, prev_hash: Option<&str>) -> Result<(String, Vec<u8>), serde_json::Error> {
    let payload = HashInput {
        unix_secs: e.unix_secs,
        trace_id: &e.trace_id,
        action: &e.action,
        decision: &e.decision,
        reason: &e.reason,
        success: e.success,
        approval_ids: e.approval_ids.as_deref(),
        reasoning_text: e.reasoning_text.as_deref(),
        prev_hash,
    };
    let canonical = serde_json::to_vec(&payload)?;
    let digest = Sha256::digest(&canonical);
    Ok((format!("{digest:x}"), canonical))
}

fn entry_to_data(e: &ChainEntry) -> EntryData {
    EntryData {
        unix_secs: e.unix_secs,
        trace_id: e.trace_id.clone(),
        action: e.action.clone(),
        decision: e.decision.clone(),
        reason: e.reason.clone(),
        success: e.success,
        approval_ids: e.approval_ids.clone(),
        reasoning_text: e.reasoning_text.clone(),
        rekor_uuid: e.rekor_uuid.clone(),
    }
}

fn verify_signature(canonical: &[u8], sig_b64: &str, pk_b64: &str) -> Result<(), String> {
    let sig_bytes = STANDARD.decode(sig_b64).map_err(|e| format!("sig b64: {e}"))?;
    let pk_bytes = STANDARD.decode(pk_b64).map_err(|e| format!("pk b64: {e}"))?;
    let sig_arr: [u8; 64] = sig_bytes.try_into().map_err(|_| "sig not 64 bytes".to_string())?;
    let pk_arr: [u8; 32] = pk_bytes.try_into().map_err(|_| "pk not 32 bytes".to_string())?;
    let pk = VerifyingKey::from_bytes(&pk_arr).map_err(|e| format!("invalid pubkey: {e}"))?;
    pk.verify(canonical, &Signature::from_bytes(&sig_arr))
        .map_err(|e| format!("signature mismatch: {e}"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::LocalSigner;

    fn chain() -> (Chain, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::open(dir.path().join("audit.jsonl"));
        (chain, dir)
    }

    #[test]
    fn append_and_verify() {
        let (chain, _d) = chain();
        let s = LocalSigner::generate();
        chain.append(EntryData::new("read_absorbance", "allow", "ok", true), &s).unwrap();
        chain.append(EntryData::new("dispense", "deny", "out of range", false), &s).unwrap();
        let r = chain.verify().unwrap();
        assert_eq!(r.entries_checked, 2);
        assert_eq!(r.signatures_verified, 2);
        assert!(r.tip_hash_hex.is_some());
    }

    #[test]
    fn tamper_breaks_chain() {
        let (chain, _d) = chain();
        let s = LocalSigner::generate();
        chain.append(EntryData::new("a", "allow", "ok", true), &s).unwrap();
        chain.append(EntryData::new("b", "allow", "ok", true), &s).unwrap();
        let original = std::fs::read_to_string(chain.path()).unwrap();
        let tampered = original.replacen("\"reason\":\"ok\"", "\"reason\":\"tampered\"", 1);
        std::fs::write(chain.path(), tampered).unwrap();
        assert!(chain.verify().is_err());
    }

    #[test]
    fn tip_hash_bytes_match_hex() {
        let (chain, _d) = chain();
        let s = LocalSigner::generate();
        chain.append(EntryData::new("a", "allow", "ok", true), &s).unwrap();
        let hex_str = chain.tip_hash_hex().unwrap().unwrap();
        let bytes = chain.tip_hash().unwrap().unwrap();
        assert_eq!(hex::encode(bytes), hex_str);
    }

    #[test]
    fn from_action_records_params() {
        let (chain, _d) = chain();
        let s = LocalSigner::generate();
        let action = Action::new("dispense", serde_json::json!({"volume_ul": 5.0}), axiom_types::RiskClass::LiquidHandling);
        let e = chain.append(EntryData::from_action(&action, true, "executed"), &s).unwrap();
        assert!(e.reason.contains("volume_ul"));
        chain.verify().unwrap();
    }

    #[test]
    fn wrong_key_fails_verification() {
        let (chain, _d) = chain();
        let s = LocalSigner::generate();
        chain.append(EntryData::new("a", "allow", "ok", true), &s).unwrap();
        // Rewrite the pubkey to a different valid key — signature must now fail.
        let other = LocalSigner::generate();
        let original = std::fs::read_to_string(chain.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(original.lines().next().unwrap()).unwrap();
        let old_pk = v.get("entry_pubkey_b64").unwrap().as_str().unwrap();
        let new_pk = STANDARD.encode(other.public_key());
        std::fs::write(chain.path(), original.replace(old_pk, &new_pk)).unwrap();
        assert!(chain.verify().is_err());
    }
}
