//! Workspace sandboxing primitives.
//!
//! Ensures the agent can only access paths and execute commands that
//! appear on the explicit allowlist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("path not allowed: {0}")]
    PathDenied(PathBuf),
    #[error("command not allowed: {0}")]
    CommandDenied(String),
    #[error("resource limit exceeded: {kind} (limit {limit}, requested {requested})")]
    ResourceLimit {
        kind: String,
        limit: u64,
        requested: u64,
    },
}

/// Resource-limit configuration for the sandbox.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum wall-clock seconds a single tool call may run.
    pub max_execution_secs: u64,
    /// Maximum bytes the agent may write per invocation.
    pub max_write_bytes: u64,
    /// Maximum concurrent hardware channels the agent may hold.
    pub max_hw_channels: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_execution_secs: 30,
            max_write_bytes: 10 * 1024 * 1024, // 10 MiB
            max_hw_channels: 4,
        }
    }
}

/// A sandbox that restricts file-system access, command execution,
/// and resource consumption.
pub struct Sandbox {
    allowed_roots: HashSet<PathBuf>,
    allowed_commands: HashSet<String>,
    limits: ResourceLimits,
}

impl Sandbox {
    /// Create a new sandbox.
    pub fn new(
        roots: impl IntoIterator<Item = PathBuf>,
        commands: impl IntoIterator<Item = String>,
        limits: ResourceLimits,
    ) -> Self {
        Self {
            allowed_roots: roots.into_iter().collect(),
            allowed_commands: commands.into_iter().collect(),
            limits,
        }
    }

    /// Check whether `path` falls under an allowed root.
    pub fn check_path(&self, path: &Path) -> Result<(), SandboxError> {
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

    /// Check whether `cmd` is on the command allowlist.
    pub fn check_command(&self, cmd: &str) -> Result<(), SandboxError> {
        if self.allowed_commands.contains(cmd) {
            Ok(())
        } else {
            Err(SandboxError::CommandDenied(cmd.to_owned()))
        }
    }

    /// Enforce a resource limit by kind.
    pub fn check_resource(&self, kind: &str, requested: u64) -> Result<(), SandboxError> {
        let limit = match kind {
            "execution_secs" => self.limits.max_execution_secs,
            "write_bytes" => self.limits.max_write_bytes,
            "hw_channels" => self.limits.max_hw_channels,
            _ => return Ok(()),
        };
        if requested <= limit {
            Ok(())
        } else {
            Err(SandboxError::ResourceLimit {
                kind: kind.to_owned(),
                limit,
                requested,
            })
        }
    }

    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sandbox() -> Sandbox {
        Sandbox::new(
            vec![PathBuf::from("/lab/workspace")],
            vec!["move_arm".into(), "read_sensor".into()],
            ResourceLimits::default(),
        )
    }

    #[test]
    fn allow_within_root() {
        let sb = test_sandbox();
        assert!(sb.check_path(Path::new("/lab/workspace/data.csv")).is_ok());
    }

    #[test]
    fn deny_outside_root() {
        let sb = test_sandbox();
        assert!(sb.check_path(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn allow_permitted_command() {
        let sb = test_sandbox();
        assert!(sb.check_command("move_arm").is_ok());
    }

    #[test]
    fn deny_unpermitted_command() {
        let sb = test_sandbox();
        assert!(sb.check_command("rm_rf").is_err());
    }

    #[test]
    fn resource_within_limit() {
        let sb = test_sandbox();
        assert!(sb.check_resource("write_bytes", 1024).is_ok());
    }

    #[test]
    fn resource_over_limit() {
        let sb = test_sandbox();
        assert!(sb
            .check_resource("write_bytes", 100 * 1024 * 1024)
            .is_err());
    }
}
