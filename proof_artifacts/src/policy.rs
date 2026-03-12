use crate::manifest::{ArtifactStatus, ProofArtifact, ProofManifest, RiskClass};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub git_commit: String,
    pub binary_hash: String,
    pub container_image_digest: Option<String>,
    pub device_id: Option<String>,
    pub firmware_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone)]
pub struct ActionExplainReport {
    pub action: String,
    pub decision: PolicyDecision,
    pub reason: String,
    pub matched_policy: Option<String>,
    pub artifacts_checked: Vec<(String, ArtifactStatus, u32)>,
}

#[derive(Debug, Error)]
pub enum RuntimePolicyError {
    #[error("build identity mismatch: {0}")]
    BuildIdentityMismatch(String),
    #[error("action denied: {0}")]
    ActionDenied(String),
}

#[derive(Debug, Clone)]
pub struct RuntimePolicyEngine {
    manifest: ProofManifest,
    signature_verified: bool,
}

impl RuntimePolicyEngine {
    pub fn new(manifest: ProofManifest) -> Self {
        Self {
            manifest,
            signature_verified: false,
        }
    }

    pub fn new_trusted(manifest: ProofManifest) -> Self {
        Self {
            manifest,
            signature_verified: true,
        }
    }

    pub fn manifest(&self) -> &ProofManifest {
        &self.manifest
    }

    pub fn authorize(
        &self,
        action: &str,
        ctx: &ExecutionContext,
    ) -> Result<(), RuntimePolicyError> {
        if !self.signature_verified {
            return Err(RuntimePolicyError::ActionDenied(
                "manifest signature has not been verified".into(),
            ));
        }
        self.validate_build_identity(ctx)?;
        let report = self.explain(action);
        if report.decision == PolicyDecision::Allow {
            Ok(())
        } else {
            Err(RuntimePolicyError::ActionDenied(report.reason))
        }
    }

    pub fn explain(&self, action: &str) -> ActionExplainReport {
        let policy = self.manifest.actions.iter().find(|p| p.action == action);
        let Some(policy) = policy else {
            return ActionExplainReport {
                action: action.to_string(),
                decision: PolicyDecision::Deny,
                reason: format!("no policy mapping for action {action}"),
                matched_policy: None,
                artifacts_checked: Vec::new(),
            };
        };

        let mut missing = Vec::new();
        let mut bad = Vec::new();
        let mut checked = Vec::new();
        let mut has_verus_artifact = false;

        for artifact_id in &policy.required_artifacts {
            match self.manifest.artifacts.iter().find(|a| &a.id == artifact_id) {
                Some(a) => {
                    checked.push((a.id.clone(), a.status.clone(), a.sorry_count));
                    if a.status != ArtifactStatus::Passed {
                        bad.push(format!("artifact {artifact_id} status {:?}", a.status));
                    }
                    if a.sorry_count > 0 {
                        bad.push(format!(
                            "artifact {artifact_id} has {} sorry placeholders",
                            a.sorry_count
                        ));
                    }
                    if a.verus.is_some() {
                        has_verus_artifact = true;
                    }
                }
                None => missing.push(artifact_id.clone()),
            }
        }

        if !missing.is_empty() {
            return ActionExplainReport {
                action: action.to_string(),
                decision: PolicyDecision::Deny,
                reason: format!("missing required artifacts: {}", missing.join(", ")),
                matched_policy: Some(policy.rationale.clone()),
                artifacts_checked: checked,
            };
        }

        if matches!(policy.risk_class, RiskClass::Actuation | RiskClass::Destructive)
            && !has_verus_artifact
        {
            bad.push("high-risk action requires at least one Verus-backed artifact".into());
        }

        if !bad.is_empty() {
            return ActionExplainReport {
                action: action.to_string(),
                decision: PolicyDecision::Deny,
                reason: bad.join("; "),
                matched_policy: Some(policy.rationale.clone()),
                artifacts_checked: checked,
            };
        }

        ActionExplainReport {
            action: action.to_string(),
            decision: PolicyDecision::Allow,
            reason: "all required proof artifacts are passed and sorry-free".to_string(),
            matched_policy: Some(policy.rationale.clone()),
            artifacts_checked: checked,
        }
    }

    fn validate_build_identity(&self, ctx: &ExecutionContext) -> Result<(), RuntimePolicyError> {
        if self.manifest.build.git_commit != ctx.git_commit {
            return Err(RuntimePolicyError::BuildIdentityMismatch(format!(
                "git commit mismatch: manifest={}, runtime={}",
                self.manifest.build.git_commit, ctx.git_commit
            )));
        }
        if self.manifest.build.binary_hash != ctx.binary_hash {
            return Err(RuntimePolicyError::BuildIdentityMismatch(format!(
                "binary hash mismatch: manifest={}, runtime={}",
                self.manifest.build.binary_hash, ctx.binary_hash
            )));
        }

        if let Some(expected) = &self.manifest.build.container_image_digest {
            if ctx.container_image_digest.as_deref() != Some(expected.as_str()) {
                return Err(RuntimePolicyError::BuildIdentityMismatch(format!(
                    "container image digest mismatch: manifest={}, runtime={:?}",
                    expected, ctx.container_image_digest
                )));
            }
        }

        if let Some(expected) = &self.manifest.build.device_id {
            if ctx.device_id.as_deref() != Some(expected.as_str()) {
                return Err(RuntimePolicyError::BuildIdentityMismatch(format!(
                    "device id mismatch: manifest={}, runtime={:?}",
                    expected, ctx.device_id
                )));
            }
        }

        if let Some(expected) = &self.manifest.build.firmware_version {
            if ctx.firmware_version.as_deref() != Some(expected.as_str()) {
                return Err(RuntimePolicyError::BuildIdentityMismatch(format!(
                    "firmware mismatch: manifest={}, runtime={:?}",
                    expected, ctx.firmware_version
                )));
            }
        }

        Ok(())
    }

    pub fn artifact_for(&self, id: &str) -> Option<&ProofArtifact> {
        self.manifest.artifacts.iter().find(|a| a.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        ActionPolicy, BuildIdentity, ProofArtifact, ProofManifest, RiskClass,
    };
    use std::collections::BTreeMap;

    fn manifest() -> ProofManifest {
        ProofManifest {
            schema_version: 1,
            generated_unix_secs: 0,
            build: BuildIdentity {
                git_commit: "g".into(),
                binary_hash: "b".into(),
                workspace_hash: "w".into(),
                container_image_digest: Some("img".into()),
                device_id: Some("dev".into()),
                firmware_version: Some("fw".into()),
            },
            artifacts: vec![ProofArtifact {
                id: "arm_safety".into(),
                source_path: "s".into(),
                source_hash: "h".into(),
                mir_path: None,
                mir_hash: None,
                lean: vec![],
                verus: Some(crate::manifest::VerusArtifact {
                    path: "verus_verified/arm_safety.rs".into(),
                    hash: "vh".into(),
                    status: ArtifactStatus::Passed,
                }),
                theorem_count: 1,
                sorry_count: 0,
                status: ArtifactStatus::Passed,
                metadata: BTreeMap::new(),
            }],
            actions: vec![ActionPolicy {
                action: "move_arm".into(),
                risk_class: RiskClass::Actuation,
                required_artifacts: vec!["arm_safety".into()],
                rationale: "hardware safety constraint".into(),
            }],
        }
    }

    #[test]
    fn authorizes_when_proof_chain_is_valid() {
        let e = RuntimePolicyEngine::new_trusted(manifest());
        let ctx = ExecutionContext {
            git_commit: "g".into(),
            binary_hash: "b".into(),
            container_image_digest: Some("img".into()),
            device_id: Some("dev".into()),
            firmware_version: Some("fw".into()),
        };
        assert!(e.authorize("move_arm", &ctx).is_ok());
    }
}
