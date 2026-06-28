//! Sigstore Rekor transparency-log anchoring of the chain tip.
//!
//! On protocol conclusion the `AuditGate` submits the chain-tip hash here to get
//! an independent, timestamped, publicly-verifiable witness. Default-on: set
//! `AXIOMLAB_REKOR_DISABLED=1` to skip (e.g. in offline CI).

use crate::signer::Signer;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum RekorError {
    #[error("http: {0}")]
    Http(String),
    #[error("rekor returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("parse: {0}")]
    Parse(String),
}

/// A Rekor log entry reference returned after anchoring.
#[derive(Debug, Clone)]
pub struct LogId {
    /// Rekor entry UUID.
    pub uuid: String,
    /// Integrated timestamp (Unix seconds) returned by the log.
    pub integrated_time: i64,
    /// SHA-256 hex of the submitted artifact (the chain-tip hash).
    pub artifact_hash: String,
}

/// Client for submitting chain-tip hashes to Sigstore Rekor.
pub struct RekorClient {
    url: String,
    timeout: std::time::Duration,
}

impl Default for RekorClient {
    fn default() -> Self {
        Self::from_env()
    }
}

impl RekorClient {
    pub fn from_env() -> Self {
        let url = std::env::var("AXIOMLAB_REKOR_URL")
            .unwrap_or_else(|_| "https://rekor.sigstore.dev/api/v1/log/entries".to_string());
        Self { url, timeout: std::time::Duration::from_secs(30) }
    }

    /// True unless `AXIOMLAB_REKOR_DISABLED=1`. Anchoring is on by default.
    pub fn enabled() -> bool {
        std::env::var("AXIOMLAB_REKOR_DISABLED").as_deref() != Ok("1")
    }

    /// Submit `hash` (the chain-tip bytes) to Rekor as a `hashedrekord`.
    ///
    /// Returns `Ok(None)` when anchoring is disabled, so callers need not
    /// special-case the unconfigured path.
    pub async fn checkpoint(
        &self,
        hash: &[u8; 32],
        signer: &dyn Signer,
    ) -> Result<Option<LogId>, RekorError> {
        if !Self::enabled() {
            return Ok(None);
        }

        let artifact_sha256 = format!("{:x}", Sha256::digest(hash));
        let sig_b64 = STANDARD.encode(signer.sign(hash));
        let pem_b64 = ed25519_spki_pem_b64(&signer.public_key());

        let body = serde_json::json!({
            "apiVersion": "0.0.1",
            "kind": "hashedrekord",
            "spec": {
                "data": { "hash": { "algorithm": "sha256", "value": artifact_sha256 } },
                "signature": { "content": sig_b64, "publicKey": { "content": pem_b64 } }
            }
        });

        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| RekorError::Http(e.to_string()))?;

        let resp = client
            .post(&self.url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| RekorError::Http(e.to_string()))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| RekorError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(RekorError::Status { status: status.as_u16(), body: text });
        }

        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| RekorError::Parse(e.to_string()))?;
        let (uuid, entry) = parsed
            .as_object()
            .and_then(|m| m.iter().next())
            .ok_or_else(|| RekorError::Parse("Rekor response had no entries".into()))?;
        let integrated_time = entry.pointer("/integratedTime").and_then(|v| v.as_i64()).unwrap_or(0);

        tracing::info!(uuid = %uuid, integrated_time, "Rekor chain-tip anchor submitted");
        Ok(Some(LogId { uuid: uuid.clone(), integrated_time, artifact_hash: artifact_sha256 }))
    }
}

/// Encode a raw Ed25519 public key as a base64'd PEM SubjectPublicKeyInfo,
/// which is what Rekor's `hashedrekord` expects.
fn ed25519_spki_pem_b64(raw_key: &[u8; 32]) -> String {
    const DER_PREFIX: [u8; 12] =
        [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    let mut der = DER_PREFIX.to_vec();
    der.extend_from_slice(raw_key);
    let pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        STANDARD.encode(&der)
    );
    STANDARD.encode(pem.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::LocalSigner;

    #[tokio::test]
    async fn disabled_returns_none() {
        // SAFETY: single-threaded test.
        unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
        let client = RekorClient::from_env();
        let s = LocalSigner::generate();
        let r = client.checkpoint(&[0u8; 32], &s).await.unwrap();
        unsafe { std::env::remove_var("AXIOMLAB_REKOR_DISABLED") };
        assert!(r.is_none());
    }

    #[test]
    fn enabled_by_default() {
        unsafe { std::env::remove_var("AXIOMLAB_REKOR_DISABLED") };
        assert!(RekorClient::enabled());
    }

    #[test]
    fn pem_encoding_is_base64() {
        let s = LocalSigner::generate();
        let pem_b64 = ed25519_spki_pem_b64(&s.public_key());
        let decoded = STANDARD.decode(pem_b64).unwrap();
        let pem = String::from_utf8(decoded).unwrap();
        assert!(pem.contains("BEGIN PUBLIC KEY"));
    }
}
