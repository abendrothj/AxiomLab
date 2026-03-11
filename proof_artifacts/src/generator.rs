use crate::cache::{ProofCache, ProofCacheEntry};
use crate::manifest::{
    ActionPolicy, ArtifactStatus, BuildIdentity, LeanArtifact, LeanStatus, ProofArtifact,
    ProofManifest, VerusArtifact,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeneratorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid artifact {id}: {reason}")]
    InvalidArtifact { id: String, reason: String },
}

#[derive(Debug, Clone)]
pub struct ArtifactInput {
    pub id: String,
    pub source_path: PathBuf,
    pub mir_path: Option<PathBuf>,
    pub lean_paths: Vec<PathBuf>,
    pub verus_proof_path: Option<PathBuf>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub build: BuildIdentity,
    pub artifacts: Vec<ArtifactInput>,
    pub actions: Vec<ActionPolicy>,
}

pub struct ManifestGenerator;

impl ManifestGenerator {
    pub fn generate(
        req: &GenerateRequest,
        mut cache: Option<&mut ProofCache>,
    ) -> Result<ProofManifest, GeneratorError> {
        let mut artifacts_out = Vec::with_capacity(req.artifacts.len());

        for artifact in &req.artifacts {
            if !artifact.source_path.exists() {
                return Err(GeneratorError::InvalidArtifact {
                    id: artifact.id.clone(),
                    reason: format!("missing source path {}", artifact.source_path.display()),
                });
            }

            let source_hash = hash_file(&artifact.source_path)?;
            let (mir_path, mir_hash) = match &artifact.mir_path {
                Some(p) => {
                    if !p.exists() {
                        return Err(GeneratorError::InvalidArtifact {
                            id: artifact.id.clone(),
                            reason: format!("missing MIR path {}", p.display()),
                        });
                    }
                    (Some(path_to_string(p)), Some(hash_file(p)?))
                }
                None => (None, None),
            };

            let mut lean = Vec::with_capacity(artifact.lean_paths.len());
            let mut theorem_count = 0u32;
            let mut sorry_count = 0u32;
            let mut lean_failed = false;

            for lean_path in &artifact.lean_paths {
                if !lean_path.exists() {
                    return Err(GeneratorError::InvalidArtifact {
                        id: artifact.id.clone(),
                        reason: format!("missing Lean path {}", lean_path.display()),
                    });
                }

                let lean_hash = hash_file(lean_path)?;
                let cache_key = format!("lean:{}", path_to_string(lean_path));

                let cached = cache
                    .as_deref()
                    .and_then(|c| c.get(&cache_key))
                    .filter(|entry| entry.value_hash == lean_hash)
                    .and_then(|entry| {
                        Some((
                            entry.theorem_count?,
                            entry.sorry_count?,
                            entry.status.clone()?,
                        ))
                    });

                let (this_theorems, this_sorry, status) = if let Some((t, s, status_s)) = cached {
                    let status = if status_s == "passed" {
                        LeanStatus::Passed
                    } else {
                        LeanStatus::Failed
                    };
                    (t, s, status)
                } else {
                    let content = fs::read_to_string(lean_path)?;
                    let this_theorems = count_theorems(&content);
                    let this_sorry = count_sorry(&content);
                    let status = if this_sorry == 0 {
                        LeanStatus::Passed
                    } else {
                        LeanStatus::Failed
                    };

                    if let Some(c) = cache.as_deref_mut() {
                        c.upsert(ProofCacheEntry {
                            key: cache_key,
                            value_hash: lean_hash.clone(),
                            unix_secs: now_unix_secs(),
                            theorem_count: Some(this_theorems),
                            sorry_count: Some(this_sorry),
                            status: Some(match status {
                                LeanStatus::Passed => "passed".into(),
                                LeanStatus::Failed => "failed".into(),
                            }),
                        });
                    }

                    (this_theorems, this_sorry, status)
                };

                theorem_count += this_theorems;
                sorry_count += this_sorry;

                if status == LeanStatus::Failed {
                    lean_failed = true;
                }

                lean.push(LeanArtifact {
                    path: path_to_string(lean_path),
                    hash: lean_hash,
                    theorem_count: this_theorems,
                    sorry_count: this_sorry,
                    status,
                });
            }

            let verus = match &artifact.verus_proof_path {
                Some(p) => {
                    if !p.exists() {
                        return Err(GeneratorError::InvalidArtifact {
                            id: artifact.id.clone(),
                            reason: format!("missing Verus proof path {}", p.display()),
                        });
                    }
                    let h = hash_file(p)?;
                    if let Some(c) = cache.as_deref_mut() {
                        let key = format!("verus:{}", path_to_string(p));
                        c.upsert(ProofCacheEntry {
                            key,
                            value_hash: h.clone(),
                            unix_secs: now_unix_secs(),
                            theorem_count: None,
                            sorry_count: None,
                            status: Some("passed".into()),
                        });
                    }
                    Some(VerusArtifact {
                        path: path_to_string(p),
                        hash: h,
                        status: ArtifactStatus::Passed,
                    })
                }
                None => None,
            };

            let status = if lean_failed {
                ArtifactStatus::Failed
            } else {
                ArtifactStatus::Passed
            };

            artifacts_out.push(ProofArtifact {
                id: artifact.id.clone(),
                source_path: path_to_string(&artifact.source_path),
                source_hash,
                mir_path,
                mir_hash,
                lean,
                verus,
                theorem_count,
                sorry_count,
                status,
                metadata: artifact.metadata.clone(),
            });
        }

        Ok(ProofManifest {
            schema_version: 1,
            generated_unix_secs: now_unix_secs(),
            build: req.build.clone(),
            artifacts: artifacts_out,
            actions: req.actions.clone(),
        })
    }
}

fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}

fn count_theorems(content: &str) -> u32 {
    content
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("theorem ") || line.starts_with("example "))
        .count() as u32
}

fn count_sorry(content: &str) -> u32 {
    let mut count = 0u32;
    let mut in_block_comment = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if in_block_comment {
            if line.contains("-/") {
                in_block_comment = false;
            }
            continue;
        }

        if line.starts_with("/-") {
            if !line.contains("-/") {
                in_block_comment = true;
            }
            continue;
        }

        if line.starts_with("--") {
            continue;
        }

        // Count token-like occurrences in code, not identifiers.
        let words = line
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty());
        for w in words {
            if w == "sorry" {
                count += 1;
            }
        }
    }

    count
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_manifest_counts_theorems_and_sorry() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("m.rs");
        let lean = tmp.path().join("m.lean");
        fs::write(&src, "fn x(){}\n").unwrap();
        fs::write(&lean, "theorem t : True := by\n  trivial\n").unwrap();

        let req = GenerateRequest {
            build: BuildIdentity {
                git_commit: "g".into(),
                binary_hash: "b".into(),
                workspace_hash: "w".into(),
            },
            artifacts: vec![ArtifactInput {
                id: "id".into(),
                source_path: src,
                mir_path: None,
                lean_paths: vec![lean],
                verus_proof_path: None,
                metadata: BTreeMap::new(),
            }],
            actions: vec![],
        };

        let m = ManifestGenerator::generate(&req, None).unwrap();
        assert_eq!(m.artifacts.len(), 1);
        assert_eq!(m.artifacts[0].theorem_count, 1);
        assert_eq!(m.artifacts[0].sorry_count, 0);
        assert_eq!(m.artifacts[0].status, ArtifactStatus::Passed);
    }

        #[test]
        fn sorry_count_ignores_comments() {
                let content = r#"
-- this says sorry but is a comment
/-
    multi-line sorry comment
-/
theorem t : True := by
    sorry
"#;
                assert_eq!(count_sorry(content), 1);
        }
}
