use crate::manifest::ProofManifest;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Embedded manifest-signing public key (Ed25519, base64).
///
/// Generated with: `cargo run -p proof_artifacts --bin keygen`
/// The corresponding private key is at ~/Documents/axiomlab_manifest_signing.private
///
/// To rotate: delete the private key file, run keygen again, paste the new public
/// key here, re-sign the manifest with:
///   python3 vessel_physics/generate_manifest.py --sign ~/Documents/axiomlab_manifest_signing.private
pub const MANIFEST_SIGNING_PUBLIC_KEY: &str =
    "uosEBKUMFKXGSaE7w0Quk67C6Ab9KUagim0uaicKB1o=";

/// Signatures provide cryptographic provenance.
/// SECURITY: Implement HSM/KMS signing and key rotation. See OPERATOR_GUIDE.md.

#[derive(Debug, Error)]
pub enum SignatureError {
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid private key bytes")]
    InvalidPrivateKey,
    #[error("invalid public key bytes")]
    InvalidPublicKey,
    #[error("invalid signature bytes")]
    InvalidSignature,
    #[error("manifest digest mismatch")]
    DigestMismatch,
    #[error("signature verification failed")]
    VerificationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSignature {
    pub algorithm: String,
    pub key_id: String,
    pub manifest_sha256: String,
    pub signature_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedProofManifest {
    pub manifest: ProofManifest,
    pub signature: ManifestSignature,
}

pub fn keygen() -> (Vec<u8>, Vec<u8>) {
    let mut rng = OsRng;
    let sk = SigningKey::generate(&mut rng);
    let pk = sk.verifying_key();
    (sk.to_bytes().to_vec(), pk.to_bytes().to_vec())
}

pub fn sign_manifest(
    manifest: &ProofManifest,
    private_key_bytes: &[u8],
    key_id: &str,
) -> Result<SignedProofManifest, SignatureError> {
    let sk_arr: [u8; 32] = private_key_bytes
        .try_into()
        .map_err(|_| SignatureError::InvalidPrivateKey)?;
    let sk = SigningKey::from_bytes(&sk_arr);

    let bytes = serde_json::to_vec(manifest)?;
    let digest = sha256_hex(&bytes);
    let sig = sk.sign(&bytes);

    Ok(SignedProofManifest {
        manifest: manifest.clone(),
        signature: ManifestSignature {
            algorithm: "ed25519".into(),
            key_id: key_id.into(),
            manifest_sha256: digest,
            signature_b64: STANDARD.encode(sig.to_bytes()),
        },
    })
}

pub fn verify_signed_manifest(
    signed: &SignedProofManifest,
    public_key_bytes: &[u8],
) -> Result<(), SignatureError> {
    let pk_arr: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| SignatureError::InvalidPublicKey)?;
    let pk = VerifyingKey::from_bytes(&pk_arr).map_err(|_| SignatureError::InvalidPublicKey)?;

    let bytes = serde_json::to_vec(&signed.manifest)?;
    let digest = sha256_hex(&bytes);
    if digest != signed.signature.manifest_sha256 {
        return Err(SignatureError::DigestMismatch);
    }

    let sig_bytes = STANDARD
        .decode(&signed.signature.signature_b64)
        .map_err(|_| SignatureError::InvalidSignature)?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| SignatureError::InvalidSignature)?;

    pk.verify(&bytes, &sig)
        .map_err(|_| SignatureError::VerificationFailed)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BuildIdentity, ProofManifest};

    #[test]
    fn sign_and_verify_roundtrip() {
        let (sk, pk) = keygen();
        let manifest = ProofManifest {
            schema_version: 1,
            generated_unix_secs: 0,
            build: BuildIdentity {
                git_commit: "g".into(),
                binary_hash: "b".into(),
                workspace_hash: "w".into(),
                container_image_digest: None,
                device_id: None,
                firmware_version: None,
            },
            artifacts: vec![],
            actions: vec![],
        };
        let signed = sign_manifest(&manifest, &sk, "test").unwrap();
        verify_signed_manifest(&signed, &pk).unwrap();
    }
}
