use crate::manifest::{ArtifactStatus, ProofManifest};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct CiGatePolicy {
    pub required_artifacts: Vec<String>,
    pub require_zero_sorry: bool,
    pub expected_git_commit: Option<String>,
    pub expected_binary_hash: Option<String>,
}

impl Default for CiGatePolicy {
    fn default() -> Self {
        Self {
            required_artifacts: Vec::new(),
            require_zero_sorry: true,
            expected_git_commit: None,
            expected_binary_hash: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CiGateReport {
    pub passed: bool,
    pub violations: Vec<String>,
}

#[derive(Debug, Error)]
#[error("CI gate failed: {0:?}")]
pub struct CiGateError(pub Vec<String>);

pub fn evaluate_ci_gate(manifest: &ProofManifest, policy: &CiGatePolicy) -> CiGateReport {
    let mut violations = Vec::new();

    for required in &policy.required_artifacts {
        let maybe = manifest.artifacts.iter().find(|a| &a.id == required);
        let Some(artifact) = maybe else {
            violations.push(format!("missing required artifact: {required}"));
            continue;
        };
        if artifact.status != ArtifactStatus::Passed {
            violations.push(format!("artifact {required} is not passed"));
        }
        if policy.require_zero_sorry && artifact.sorry_count > 0 {
            violations.push(format!(
                "artifact {required} contains {} sorry placeholders",
                artifact.sorry_count
            ));
        }
    }

    if let Some(expected) = &policy.expected_git_commit {
        if &manifest.build.git_commit != expected {
            violations.push(format!(
                "git commit mismatch: expected {expected}, got {}",
                manifest.build.git_commit
            ));
        }
    }

    if let Some(expected) = &policy.expected_binary_hash {
        if &manifest.build.binary_hash != expected {
            violations.push(format!(
                "binary hash mismatch: expected {expected}, got {}",
                manifest.build.binary_hash
            ));
        }
    }

    CiGateReport {
        passed: violations.is_empty(),
        violations,
    }
}

pub fn enforce_ci_gate(manifest: &ProofManifest, policy: &CiGatePolicy) -> Result<(), CiGateError> {
    let report = evaluate_ci_gate(manifest, policy);
    if report.passed {
        Ok(())
    } else {
        Err(CiGateError(report.violations))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest,
    };
    use std::collections::BTreeMap;

    fn sample_manifest() -> ProofManifest {
        ProofManifest {
            schema_version: 1,
            generated_unix_secs: 0,
            build: BuildIdentity {
                git_commit: "abc".into(),
                binary_hash: "bin".into(),
                workspace_hash: "ws".into(),
            },
            artifacts: vec![ProofArtifact {
                id: "a".into(),
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
                required_artifacts: vec!["a".into()],
                rationale: "safety".into(),
            }],
        }
    }

    #[test]
    fn gate_passes() {
        let m = sample_manifest();
        let p = CiGatePolicy {
            required_artifacts: vec!["a".into()],
            require_zero_sorry: true,
            expected_git_commit: Some("abc".into()),
            expected_binary_hash: Some("bin".into()),
        };
        assert!(evaluate_ci_gate(&m, &p).passed);
    }
}
