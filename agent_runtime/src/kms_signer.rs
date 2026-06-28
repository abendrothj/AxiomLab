//! AWS KMS–backed `AuditSigner` implementation.
//!
//! Enabled only when compiled with `--features kms`.
//!
//! The KMS key must be an asymmetric Ed25519 `SIGN_VERIFY` key.
//! Set `AXIOMLAB_KMS_KEY_ID` to the full ARN or alias (e.g. `alias/axiomlab-audit`).
//! AWS credentials are resolved via the standard SDK credential chain
//! (env vars, ~/.aws/credentials, instance profile, IRSA, etc.).
//!
//! # Usage (server main.rs)
//!
//! ```rust,ignore
//! #[cfg(feature = "kms")]
//! {
//!     use agent_runtime::kms_signer::KmsSigner;
//!     let signer = KmsSigner::from_env().await?;
//!     // Pass Box::new(signer) as the AuditSigner for OrchestratorConfig.
//! }
//! ```

#[cfg(feature = "kms")]
mod inner {
    use crate::audit::AuditSigner;
    use aws_sdk_kms::Client;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    /// AWS KMS–backed Ed25519 signing key.
    ///
    /// Each `sign()` call makes a synchronous KMS `Sign` API request.
    /// Calling `public_key_b64()` and `verifying_key_bytes()` are cached after
    /// the first `GetPublicKey` call at construction time, so they are free
    /// at every audit event.
    pub struct KmsSigner {
        client: Client,
        key_id: String,
        /// Cached raw 32-byte Ed25519 public key.
        raw_pub: [u8; 32],
        pub_b64: String,
    }

    impl KmsSigner {
        /// Build a `KmsSigner` from the environment.
        ///
        /// Reads `AXIOMLAB_KMS_KEY_ID` for the key ARN/alias.
        /// AWS credentials are resolved via the default SDK chain.
        ///
        /// Performs one `GetPublicKey` call to cache the public key bytes —
        /// fails fast if the key ID is wrong or permissions are missing.
        pub async fn from_env() -> Result<Self, String> {
            let key_id = std::env::var("AXIOMLAB_KMS_KEY_ID")
                .map_err(|_| "AXIOMLAB_KMS_KEY_ID must be set to use KMS signing".to_string())?;
            Self::new(&key_id).await
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

            // KMS returns the key as DER SubjectPublicKeyInfo.
            // Ed25519 SPKI DER is 44 bytes: 12-byte header + 32-byte key.
            let der = resp
                .public_key()
                .ok_or("KMS GetPublicKey returned no key material")?
                .as_ref()
                .to_vec();

            if der.len() < 44 {
                return Err(format!(
                    "KMS Ed25519 public key DER too short: {} bytes (expected ≥44)",
                    der.len()
                ));
            }
            let raw_pub: [u8; 32] = der[der.len() - 32..]
                .try_into()
                .map_err(|_| "KMS public key slice is not 32 bytes".to_string())?;

            let pub_b64 = STANDARD.encode(&raw_pub);
            tracing::info!(key_id, "KMS audit signer initialised");

            Ok(Self {
                client,
                key_id: key_id.to_string(),
                raw_pub,
                pub_b64,
            })
        }
    }

    impl AuditSigner for KmsSigner {
        fn sign(&self, data: &[u8]) -> String {
            // KMS sign is async; we block-in-place so the trait stays sync.
            // This is safe because we are always called from within a Tokio runtime.
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
                        Ok(r) => r
                            .signature()
                            .map(|b| STANDARD.encode(b.as_ref()))
                            .unwrap_or_else(|| {
                                tracing::error!("KMS Sign returned no signature bytes");
                                String::new()
                            }),
                        Err(e) => {
                            tracing::error!(error = %e, "KMS Sign failed");
                            String::new()
                        }
                    }
                })
            })
        }

        fn public_key_b64(&self) -> String {
            self.pub_b64.clone()
        }

        fn verifying_key_bytes(&self) -> [u8; 32] {
            self.raw_pub
        }
    }
}

#[cfg(feature = "kms")]
pub use inner::KmsSigner;
