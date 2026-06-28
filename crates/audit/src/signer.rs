//! Pluggable Ed25519 signing backends.
//!
//! Every audit entry is signed. The [`Signer`] trait abstracts where the key
//! lives: a local file (default fallback), an inline env var (CI), or AWS KMS
//! (production default when `AXIOMLAB_KMS_KEY_ID` is set).

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer as _, SigningKey};

/// A signing backend for audit entries and Rekor submissions.
///
/// Returns raw bytes; the chain base64-encodes them at persistence time.
pub trait Signer: Send + Sync {
    /// Sign `data`, returning the raw 64-byte Ed25519 signature.
    fn sign(&self, data: &[u8]) -> Vec<u8>;
    /// The raw 32-byte Ed25519 verifying (public) key.
    fn public_key(&self) -> [u8; 32];
    /// A stable identifier for this key, used for revocation checks.
    ///
    /// Defaults to the base64 of the public key.
    fn key_id(&self) -> String {
        STANDARD.encode(self.public_key())
    }
}

// ── Local Ed25519 key ──────────────────────────────────────────────────────

/// A [`Signer`] backed by an in-process Ed25519 key.
///
/// The key may be generated, loaded from a base64 string, or persisted to a
/// file so it survives restarts (preserving chain continuity).
pub struct LocalSigner {
    key: SigningKey,
    key_id: String,
}

impl LocalSigner {
    /// Generate a fresh random key (useful for tests).
    pub fn generate() -> Self {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        Self::with_default_id(key)
    }

    /// Load a key from a raw 32-byte base64 string.
    pub fn from_b64(b64: &str) -> Result<Self, String> {
        let bytes = STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("signing key base64 decode failed: {e}"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "signing key must be 32 bytes".to_string())?;
        Ok(Self::with_default_id(SigningKey::from_bytes(&arr)))
    }

    /// Load the key from `path`, or generate and persist a fresh one (mode 0600).
    pub fn load_or_create(path: &std::path::Path) -> Result<Self, String> {
        if path.exists() {
            let b64 = std::fs::read_to_string(path)
                .map_err(|e| format!("read signing key {}: {e}", path.display()))?;
            return Self::from_b64(b64.trim());
        }
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let b64 = STANDARD.encode(key.to_bytes());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create key dir {}: {e}", parent.display()))?;
        }
        std::fs::write(path, &b64).map_err(|e| format!("write signing key {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        tracing::info!(path = %path.display(), "Generated new persistent audit signing key (0600)");
        Ok(Self::with_default_id(key))
    }

    fn with_default_id(key: SigningKey) -> Self {
        let key_id = STANDARD.encode(key.verifying_key().to_bytes());
        Self { key, key_id }
    }

    /// The base64-encoded private key (for persisting / test fixtures).
    pub fn private_key_b64(&self) -> String {
        STANDARD.encode(self.key.to_bytes())
    }
}

impl Signer for LocalSigner {
    fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.key.sign(data).to_bytes().to_vec()
    }
    fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }
    fn key_id(&self) -> String {
        self.key_id.clone()
    }
}

// ── Environment resolution ─────────────────────────────────────────────────

/// Resolve the audit signer from the environment, following the rewrite policy:
///
/// 1. `AXIOMLAB_KMS_KEY_ID` set **and** built with `--features kms` → [`KmsSigner`].
/// 2. `AXIOMLAB_AUDIT_SIGNING_KEY` — inline base64 private key (CI / legacy).
/// 3. `AXIOMLAB_AUDIT_SIGNING_KEY_PATH` or `~/.config/axiomlab/audit_signing.key`
///    — file-backed [`LocalSigner`], created on first use.
///
/// KMS is the preferred production default; the local key is the fallback.
pub fn signer_from_env() -> Result<Box<dyn Signer>, String> {
    if std::env::var("AXIOMLAB_KMS_KEY_ID").is_ok() {
        #[cfg(feature = "kms")]
        {
            return KmsSigner::from_env().map(|s| Box::new(s) as Box<dyn Signer>);
        }
        #[cfg(not(feature = "kms"))]
        {
            tracing::warn!(
                "AXIOMLAB_KMS_KEY_ID is set but the binary was not built with --features kms; \
                 falling back to a local signing key"
            );
        }
    }

    if let Ok(b64) = std::env::var("AXIOMLAB_AUDIT_SIGNING_KEY") {
        return LocalSigner::from_b64(&b64).map(|s| Box::new(s) as Box<dyn Signer>);
    }

    let path = std::env::var("AXIOMLAB_AUDIT_SIGNING_KEY_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("axiomlab")
                .join("audit_signing.key")
        });
    LocalSigner::load_or_create(&path).map(|s| Box::new(s) as Box<dyn Signer>)
}

// ── AWS KMS backend (feature = "kms") ──────────────────────────────────────

#[cfg(feature = "kms")]
pub use kms_impl::KmsSigner;

#[cfg(feature = "kms")]
mod kms_impl {
    use super::Signer;
    use aws_sdk_kms::Client;

    /// AWS KMS–backed Ed25519 signing key (asymmetric `SIGN_VERIFY`).
    ///
    /// `AXIOMLAB_KMS_KEY_ID` selects the key (ARN or alias). Credentials come
    /// from the standard AWS SDK chain.
    pub struct KmsSigner {
        client: Client,
        key_id: String,
        raw_pub: [u8; 32],
    }

    impl KmsSigner {
        pub fn from_env() -> Result<Self, String> {
            let key_id = std::env::var("AXIOMLAB_KMS_KEY_ID")
                .map_err(|_| "AXIOMLAB_KMS_KEY_ID must be set for KMS signing".to_string())?;
            // Construction does a GetPublicKey round-trip; block on it once.
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(Self::new(&key_id))
            })
        }

        pub async fn new(key_id: &str) -> Result<Self, String> {
            let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let client = Client::new(&config);
            let resp = client
                .get_public_key()
                .key_id(key_id)
                .send()
                .await
                .map_err(|e| format!("KMS GetPublicKey failed: {e}"))?;
            let der = resp
                .public_key()
                .ok_or("KMS GetPublicKey returned no key material")?
                .as_ref()
                .to_vec();
            if der.len() < 44 {
                return Err(format!("KMS Ed25519 SPKI DER too short: {} bytes", der.len()));
            }
            let raw_pub: [u8; 32] = der[der.len() - 32..]
                .try_into()
                .map_err(|_| "KMS public key slice not 32 bytes".to_string())?;
            tracing::info!(key_id, "KMS audit signer initialised");
            Ok(Self { client, key_id: key_id.to_string(), raw_pub })
        }
    }

    impl Signer for KmsSigner {
        fn sign(&self, data: &[u8]) -> Vec<u8> {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let resp = self
                        .client
                        .sign()
                        .key_id(&self.key_id)
                        .message(aws_sdk_kms::primitives::Blob::new(data.to_vec()))
                        .message_type(aws_sdk_kms::types::MessageType::Raw)
                        .signing_algorithm(aws_sdk_kms::types::SigningAlgorithmSpec::EdDsa)
                        .send()
                        .await;
                    match resp {
                        Ok(r) => r.signature().map(|b| b.as_ref().to_vec()).unwrap_or_default(),
                        Err(e) => {
                            tracing::error!(error = %e, "KMS Sign failed");
                            Vec::new()
                        }
                    }
                })
            })
        }
        fn public_key(&self) -> [u8; 32] {
            self.raw_pub
        }
        fn key_id(&self) -> String {
            self.key_id.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_signer_roundtrip() {
        let s = LocalSigner::generate();
        let sig = s.sign(b"hello");
        assert_eq!(sig.len(), 64);
        assert_eq!(s.public_key().len(), 32);
        // key_id defaults to base64 of pubkey
        assert_eq!(s.key_id(), STANDARD.encode(s.public_key()));
    }

    #[test]
    fn from_b64_reloads_same_key() {
        let s = LocalSigner::generate();
        let b64 = s.private_key_b64();
        let s2 = LocalSigner::from_b64(&b64).unwrap();
        assert_eq!(s.public_key(), s2.public_key());
    }

    #[test]
    fn load_or_create_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.key");
        let a = LocalSigner::load_or_create(&path).unwrap();
        let b = LocalSigner::load_or_create(&path).unwrap();
        assert_eq!(a.public_key(), b.public_key());
    }
}
