//! Verus compiler driver invocation.
//!
//! Writes the candidate source to a temporary file, invokes the Verus
//! compiler, and captures its stdout/stderr for diagnostic parsing.

use std::path::{Path, PathBuf};
use std::process::Output;
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum CompilerError {
    #[error("verus binary not found at {0}")]
    BinaryNotFound(PathBuf),
    #[error("failed to spawn verus: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("failed to write source file: {0}")]
    WriteSource(std::io::Error),
}

/// Result of a single Verus compilation attempt.
#[derive(Debug)]
pub struct CompileResult {
    /// True if Verus exited with code 0 (all proofs discharged).
    pub success: bool,
    /// Combined stdout + stderr output.
    pub output: String,
    /// Exit code.
    pub exit_code: Option<i32>,
}

/// Locate the Verus binary.
///
/// Checks, in order:
/// 1. `VERUS_PATH` environment variable.
/// 2. `verus` on `$PATH`.
pub fn find_verus() -> Result<PathBuf, CompilerError> {
    if let Ok(p) = std::env::var("VERUS_PATH") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Ok(path);
        }
        return Err(CompilerError::BinaryNotFound(path));
    }
    // Fall back to $PATH lookup.
    Ok(PathBuf::from("verus"))
}

/// Compile `source` through Verus and return the result.
///
/// `source` is the full Rust+Verus file content.
/// `work_dir` is a temporary directory for the source file.
pub async fn invoke_verus(source: &str, work_dir: &Path) -> Result<CompileResult, CompilerError> {
    let src_path = work_dir.join("candidate.rs");
    tokio::fs::write(&src_path, source)
        .await
        .map_err(CompilerError::WriteSource)?;

    let verus = find_verus()?;

    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(&verus)
        .arg(&src_path)
        .output()
        .await?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );

    Ok(CompileResult {
        success: status.success(),
        output: combined,
        exit_code: status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_verus_returns_path() {
        // Without VERUS_PATH set, falls back to bare "verus" name.
        // SAFETY: test-only; no concurrent env access.
        unsafe { std::env::remove_var("VERUS_PATH") };
        let p = find_verus().unwrap();
        assert_eq!(p, PathBuf::from("verus"));
    }
}
