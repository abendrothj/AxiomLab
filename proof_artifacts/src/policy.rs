use crate::manifest::{ArtifactStatus, ProofArtifact, ProofManifest};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub git_commit: String,
    pub binary_hash: String,
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
}

impl RuntimePolicyEngine {
    pub fn new(manifest: ProofManifest) -> Self {
        Self { manifest }
    }

    pub fn manifest(&self) -> &ProofManifest {
        &self.manifest
    }

    pub fn authorize(
        &self,
        action: &str,
        ctx: &ExecutionContext,
    ) -> Result<(), RuntimePolicyError> {
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
        ActionPolicy, BuildIdentity, ProofArtifact, ProofManifest,
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
            },
            artifacts: vec![ProofArtifact {
                id: "arm_safety".into(),
                source_path: "s".into(),
                source_hash: "h".into(),
                mir_path: None,
                mir_hash: None,
                lean: vec![],
                verus: None,
                theorem_count: 1,
                sorry_count: 0,
                status: ArtifactStatus::Passed,
                metadata: BTreeMap::new(),
            }],
            actions: vec![ActionPolicy {
                action: "move_arm".into(),
                required_artifacts: vec!["arm_safety".into()],
                rationale: "hardware safety constraint".into(),
            }],
        }
    }

    #[test]
    fn authorizes_when_proof_chain_is_valid() {
        let e = RuntimePolicyEngine::new(manifest());
        let ctx = ExecutionContext {
            git_commit: "g".into(),
            binary_hash: "b".into(),
        };
        assert!(e.authorize("move_arm", &ctx).is_ok());
    }
}
