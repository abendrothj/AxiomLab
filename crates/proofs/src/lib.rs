//! Proof verification for the `ProofGate`.
//!
//! Two independent checks, both of which must pass before a high-risk action runs:
//!
//! 1. **Artifact check** ([`ProofChecker`]) — load the signed manifest, verify
//!    its Ed25519 signature against the embedded key, and confirm the artifacts
//!    the action's policy requires are present, `Passed`, and sorry-free (with
//!    high-risk actions requiring a Verus-backed artifact).
//!
//! 2. **Predicate check** ([`predicates::evaluate`]) — call the runtime twin of
//!    the Verus spec with the *actual* proposed parameters.

mod manifest;
pub mod predicates;
mod signature;

pub use manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, LeanArtifact, ProofArtifact, ProofManifest,
    VerusArtifact,
};
pub use predicates::{PredicateOutcome, evaluate as evaluate_predicate};
pub use signature::{
    MANIFEST_SIGNING_PUBLIC_KEY, ManifestSignature, SignatureError, SignedProofManifest,
    keygen, sha256_hex, sign_manifest, verify_signed_manifest,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// A loaded, signature-verified manifest that can answer artifact-coverage
/// questions for the `ProofGate`.
#[derive(Debug, Clone)]
pub struct ProofChecker {
    manifest: ProofManifest,
}

impl ProofChecker {
    /// Load a signed manifest from `path` and verify its signature against the
    /// embedded [`MANIFEST_SIGNING_PUBLIC_KEY`].
    ///
    /// Build with `--features unsafe-bypass` (dev/CI only) to skip verification
    /// with a loud warning.
    pub fn load_and_verify(path: &str) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("read manifest {path}: {e}"))?;
        let signed: SignedProofManifest =
            serde_json::from_str(&raw).map_err(|e| format!("parse signed manifest {path}: {e}"))?;

        #[cfg(feature = "unsafe-bypass")]
        {
            tracing::warn!(path, "UNSAFE: unsafe-bypass — manifest signature NOT verified");
            return Ok(Self { manifest: signed.manifest });
        }
        #[cfg(not(feature = "unsafe-bypass"))]
        {
            let pk = STANDARD
                .decode(MANIFEST_SIGNING_PUBLIC_KEY)
                .map_err(|e| format!("invalid embedded signing key: {e}"))?;
            verify_signed_manifest(&signed, &pk).map_err(|e| format!("manifest signature: {e}"))?;
            tracing::info!(path, key_id = %signed.signature.key_id, "Manifest signature verified");
            Ok(Self { manifest: signed.manifest })
        }
    }

    /// Construct directly from a manifest the caller has already established trust
    /// in (e.g. verified out-of-band, or assembled in tests). Prefer
    /// [`ProofChecker::load_and_verify`] for the production load path.
    pub fn from_manifest_trusted(manifest: ProofManifest) -> Self {
        Self { manifest }
    }

    pub fn manifest(&self) -> &ProofManifest {
        &self.manifest
    }

    /// Confirm the proof artifacts required for `tool` are present, `Passed`, and
    /// sorry-free; high-risk actions additionally require a Verus-backed artifact.
    pub fn check_artifact(&self, tool: &str) -> Result<(), String> {
        use axiom_types::RiskClass;

        let policy = self
            .manifest
            .policy_for(tool)
            .ok_or_else(|| format!("no proof policy mapping for action '{tool}'"))?;

        let mut has_verus = false;
        for id in &policy.required_artifacts {
            let a = self
                .manifest
                .artifact(id)
                .ok_or_else(|| format!("required artifact '{id}' missing from manifest"))?;
            if a.status != ArtifactStatus::Passed {
                return Err(format!("artifact '{id}' status is {:?}, not Passed", a.status));
            }
            if a.sorry_count > 0 {
                return Err(format!("artifact '{id}' has {} sorry placeholders", a.sorry_count));
            }
            if a.verus.is_some() {
                has_verus = true;
            }
        }

        if matches!(policy.risk_class, RiskClass::Actuation | RiskClass::Destructive) && !has_verus {
            return Err(format!(
                "high-risk action '{tool}' requires at least one Verus-backed artifact"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_types::RiskClass;
    use std::collections::BTreeMap;

    fn manifest() -> ProofManifest {
        ProofManifest {
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
            artifacts: vec![ProofArtifact {
                id: "lab_safety_verus".into(),
                source_path: "verus_verified/lab_safety.rs".into(),
                source_hash: "h".into(),
                mir_path: None,
                mir_hash: None,
                lean: vec![],
                verus: Some(VerusArtifact {
                    path: "verus_verified/lab_safety.rs".into(),
                    hash: "h".into(),
                    status: ArtifactStatus::Passed,
                }),
                theorem_count: 0,
                sorry_count: 0,
                status: ArtifactStatus::Passed,
                metadata: BTreeMap::new(),
            }],
            actions: vec![ActionPolicy {
                action: "move_arm".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["lab_safety_verus".into()],
                rationale: "hardware safety".into(),
            }],
        }
    }

    #[test]
    fn artifact_check_passes_for_valid_action() {
        let c = ProofChecker::from_manifest_trusted(manifest());
        assert!(c.check_artifact("move_arm").is_ok());
    }

    #[test]
    fn unknown_action_rejected() {
        let c = ProofChecker::from_manifest_trusted(manifest());
        assert!(c.check_artifact("teleport").is_err());
    }

    #[test]
    fn high_risk_requires_verus() {
        let mut m = manifest();
        m.artifacts[0].verus = None;
        let c = ProofChecker::from_manifest_trusted(m);
        assert!(c.check_artifact("move_arm").is_err());
    }

    #[test]
    fn failed_artifact_rejected() {
        let mut m = manifest();
        m.artifacts[0].status = ArtifactStatus::Failed;
        let c = ProofChecker::from_manifest_trusted(m);
        assert!(c.check_artifact("move_arm").is_err());
    }

    #[test]
    fn load_and_verify_roundtrip() {
        // Sign with a generated key and verify via load path using unsafe-bypass-free
        // verification against that same key (simulated by direct verify here).
        let (sk, pk) = keygen();
        let signed = sign_manifest(&manifest(), &sk, "test").unwrap();
        verify_signed_manifest(&signed, &pk).unwrap();
        // And the checker logic works on the manifest.
        let c = ProofChecker::from_manifest_trusted(signed.manifest);
        assert!(c.check_artifact("move_arm").is_ok());
    }
}
