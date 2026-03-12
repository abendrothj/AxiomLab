use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LeanStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildIdentity {
    pub git_commit: String,
    pub binary_hash: String,
    pub workspace_hash: String,
    pub container_image_digest: Option<String>,
    pub device_id: Option<String>,
    pub firmware_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeanArtifact {
    pub path: String,
    pub hash: String,
    pub theorem_count: u32,
    pub sorry_count: u32,
    pub status: LeanStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerusArtifact {
    pub path: String,
    pub hash: String,
    pub status: ArtifactStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofArtifact {
    pub id: String,
    pub source_path: String,
    pub source_hash: String,
    pub mir_path: Option<String>,
    pub mir_hash: Option<String>,
    pub lean: Vec<LeanArtifact>,
    pub verus: Option<VerusArtifact>,
    pub theorem_count: u32,
    pub sorry_count: u32,
    pub status: ArtifactStatus,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RiskClass {
    ReadOnly,
    LiquidHandling,
    Actuation,
    Destructive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionPolicy {
    pub action: String,
    pub risk_class: RiskClass,
    pub required_artifacts: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofManifest {
    pub schema_version: u32,
    pub generated_unix_secs: u64,
    pub build: BuildIdentity,
    pub artifacts: Vec<ProofArtifact>,
    pub actions: Vec<ActionPolicy>,
}
