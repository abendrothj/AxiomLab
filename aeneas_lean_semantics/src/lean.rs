//! Lean 4 type-checker invocation.
//!
//! After Aeneas generates `.lean` files, this module runs `lean` to
//! verify that the generated code type-checks (and that any attached
//! theorem statements are provable).

use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::process::Command;
use tracing::info;

#[derive(Debug, Error)]
pub enum LeanError {
    #[error("lean binary not found; install Lean 4 via elan")]
    BinaryNotFound,
    #[error("lean check failed: {0}")]
    CheckFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result of a Lean type-check.
#[derive(Debug)]
pub struct LeanResult {
    pub success: bool,
    pub output: String,
}

/// Locate the `lean` binary (typically installed via `elan`).
pub fn find_lean() -> Result<PathBuf, LeanError> {
    if let Ok(p) = std::env::var("LEAN_PATH_BIN") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Ok(path);
        }
    }
    Ok(PathBuf::from("lean"))
}

/// Type-check a single `.lean` file.
pub async fn check_file(lean_file: &Path) -> Result<LeanResult, LeanError> {
    info!(file = %lean_file.display(), "type-checking Lean file");

    let lean = find_lean()?;

    let output = Command::new(&lean)
        .arg(lean_file)
        .output()
        .await?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(LeanResult {
        success: output.status.success(),
        output: combined,
    })
}

/// Type-check all `.lean` files in a directory.
pub async fn check_all(lean_dir: &Path) -> Result<Vec<(PathBuf, LeanResult)>, LeanError> {
    let mut results = Vec::new();
    let mut entries = tokio::fs::read_dir(lean_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("lean") {
            let result = check_file(&path).await?;
            results.push((path, result));
        }
    }

    let passed = results.iter().filter(|(_, r)| r.success).count();
    let total = results.len();
    info!(passed, total, "Lean type-check complete");

    Ok(results)
}

/// End-to-end pipeline: MIR export → Aeneas translation → Lean check.
pub async fn verify_crate(crate_dir: &Path) -> Result<Vec<(PathBuf, LeanResult)>, LeanError> {
    use crate::aeneas;
    use crate::mir_export;

    let mir = mir_export::export_mir(crate_dir)
        .await
        .map_err(|e| LeanError::CheckFailed(format!("MIR export: {e}")))?;

    let lean_out_dir = crate_dir.join("target/lean");
    let aeneas_out = aeneas::translate(&mir.mir_path, &lean_out_dir)
        .await
        .map_err(|e| LeanError::CheckFailed(format!("Aeneas: {e}")))?;

    check_all(&aeneas_out.lean_dir).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_lean_returns_path() {
        // SAFETY: test-only; no concurrent env access.
        unsafe { std::env::remove_var("LEAN_PATH_BIN") };
        let p = find_lean().unwrap();
        assert_eq!(p, PathBuf::from("lean"));
    }

    #[test]
    fn lean_error_display() {
        let e = LeanError::BinaryNotFound;
        assert!(e.to_string().contains("lean binary not found"));
    }
}
