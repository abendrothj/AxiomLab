//! Sigstore Rekor transparency-log anchoring.
//!
//! After each signed protocol conclusion, the SHA-256 entry hash is submitted
//! to the public Rekor log.  Rekor returns a log entry UUID and an
//! `integratedTime` (Unix seconds) backed by a Merkle tree signed by Rekor's
//! own key.  The UUID can be used at any time to prove that the hash existed
//! at that timestamp, without trusting AxiomLab itself.
//!
//! # Verification
//! ```sh
//! rekor-cli verify --uuid <uuid> --artifact-hash <sha256_hex>
//! ```
//! Or via the REST API:
//! ```sh
//! curl https://rekor.sigstore.dev/api/v1/log/entries/<uuid>
//! ```
//!
//! Rekor anchoring is best-effort: a network failure does not affect the local
//! audit chain, which remains cryptographically intact.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};

const REKOR_API: &str = "https://rekor.sigstore.dev/api/v1/log/entries";

// ── Public types ──────────────────────────────────────────────────────────────

/// A successfully created Rekor log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RekorAnchor {
    /// Rekor-assigned UUID for this log entry.
    pub uuid: String,
    /// Sequential index in the Rekor transparency log.
    pub log_index: u64,
    /// Unix seconds at which Rekor included the entry.
    pub integrated_time: i64,
}

#[derive(Debug)]
pub enum RekorError {
    Http(reqwest::Error),
    BadResponse(String),
}

impl std::fmt::Display for RekorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RekorError::Http(e)         => write!(f, "HTTP: {e}"),
            RekorError::BadResponse(s)  => write!(f, "bad response: {s}"),
        }
    }
}

impl From<reqwest::Error> for RekorError {
    fn from(e: reqwest::Error) -> Self { RekorError::Http(e) }
}

// ── PEM helper ────────────────────────────────────────────────────────────────

/// Build a SubjectPublicKeyInfo PEM from 32 raw Ed25519 public key bytes.
///
/// The DER prefix encodes the OID 1.3.101.112 (id-EdDSA, RFC 8410):
///   SEQUENCE { SEQUENCE { OID } BIT STRING { key } }
pub fn ed25519_pubkey_pem(raw: &[u8; 32]) -> String {
    // Fixed DER header for Ed25519 SubjectPublicKeyInfo (44 bytes total)
    const PREFIX: [u8; 12] = [
        0x30, 0x2a,             // SEQUENCE, 42 bytes
        0x30, 0x05,             // SEQUENCE, 5 bytes
        0x06, 0x03, 0x2b, 0x65, 0x70,  // OID 1.3.101.112
        0x03, 0x21, 0x00,       // BIT STRING, 33 bytes, 0 unused bits
    ];
    let mut der = Vec::with_capacity(44);
    der.extend_from_slice(&PREFIX);
    der.extend_from_slice(raw);
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        B64.encode(&der)
    )
}

// ── Rekor submission ──────────────────────────────────────────────────────────

/// Submit a SHA-256 hash + Ed25519 signature to the Sigstore Rekor transparency log.
///
/// # Arguments
/// - `hash_hex`   — lowercase hex SHA-256 of the artifact (the audit entry hash)
/// - `sig_b64`    — standard Base64 of the Ed25519 signature over the artifact
/// - `pubkey_pem` — PEM SubjectPublicKeyInfo; use [`ed25519_pubkey_pem`]
///
/// On success, returns a [`RekorAnchor`] with the log UUID and timestamp.
/// On failure, logs a warning and returns an error — the local audit chain
/// is unaffected.
pub async fn anchor(
    hash_hex: &str,
    sig_b64: &str,
    pubkey_pem: &str,
) -> Result<RekorAnchor, RekorError> {
    let body = serde_json::json!({
        "kind": "hashedrekord",
        "apiVersion": "0.0.1",
        "spec": {
            "data": {
                "hash": {
                    "algorithm": "sha256",
                    "value": hash_hex
                }
            },
            "signature": {
                "content": sig_b64,
                "publicKey": {
                    // Rekor expects the PEM base64-encoded (base64 of PEM text)
                    "content": B64.encode(pubkey_pem.as_bytes())
                }
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let resp = client
        .post(REKOR_API)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(RekorError::BadResponse(format!("{status}: {text}")));
    }

    // Rekor responds with { "<uuid>": { "logIndex": N, "integratedTime": T, ... } }
    let map: serde_json::Map<String, serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| RekorError::BadResponse(format!("parse: {e}")))?;

    let (uuid, entry) = map
        .into_iter()
        .next()
        .ok_or_else(|| RekorError::BadResponse("empty response map".into()))?;

    Ok(RekorAnchor {
        uuid,
        log_index:       entry["logIndex"].as_u64().unwrap_or(0),
        integrated_time: entry["integratedTime"].as_i64().unwrap_or(0),
    })
}
