pub mod manifest;
pub mod generator;
pub mod cache;
pub mod ci;
pub mod policy;
pub mod signature;

pub use cache::{ProofCache, ProofCacheEntry};
pub use ci::{CiGateError, CiGatePolicy, CiGateReport, evaluate_ci_gate};
pub use generator::{ArtifactInput, GenerateRequest, GeneratorError, ManifestGenerator};
pub use manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, LeanArtifact, LeanStatus, ProofArtifact,
    ProofManifest, RiskClass, VerusArtifact,
};
pub use policy::{
    ActionExplainReport, ExecutionContext, PolicyDecision, RuntimePolicyEngine, RuntimePolicyError,
};
pub use signature::{
    ManifestSignature, SignatureError, SignedProofManifest, keygen, sign_manifest,
    verify_signed_manifest,
};
