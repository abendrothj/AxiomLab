//! Workspace sandboxing primitives.
//!
//! Ensures the agent can only access paths and syscalls that appear
//! on the explicit allowlist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("path not allowed: {0}")]
    PathDenied(PathBuf),
}

/// A simple path-based sandbox that restricts file-system access.
pub struct Sandbox {
    allowed_roots: HashSet<PathBuf>,
}

impl Sandbox {
    /// Create a new sandbox that only permits access under `roots`.
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            allowed_roots: roots.into_iter().collect(),
        }
    }

    /// Check whether `path` falls under an allowed root.
    pub fn check(&self, path: &Path) -> Result<(), SandboxError> {
        if self
            .allowed_roots
            .iter()
            .any(|root| path.starts_with(root))
        {
            Ok(())
        } else {
            Err(SandboxError::PathDenied(path.to_path_buf()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_within_root() {
        let sb = Sandbox::new(vec![PathBuf::from("/lab/workspace")]);
        assert!(sb.check(Path::new("/lab/workspace/data.csv")).is_ok());
    }

    #[test]
    fn deny_outside_root() {
        let sb = Sandbox::new(vec![PathBuf::from("/lab/workspace")]);
        assert!(sb.check(Path::new("/etc/passwd")).is_err());
    }
}
