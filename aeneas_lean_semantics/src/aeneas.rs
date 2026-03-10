//! Aeneas toolchain integration.
//!
//! Invokes the `aeneas` binary to translate Rust MIR into Lean 4 source
//! files.  The generated Lean code uses purely functional representations
//! (leveraging Rust's aliasing rules to avoid separation-logic overhead).

use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::process::Command;
use tracing::info;

#[derive(Debug, Error)]
pub enum AeneasError {
    #[error("aeneas binary not found; set AENEAS_PATH or install aeneas")]
    BinaryNotFound,
    #[error("aeneas invocation failed: {0}")]
    Invocation(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no Lean files generated")]
    NoOutput,
}

/// Output of a successful Aeneas translation.
#[derive(Debug)]
pub struct AeneasOutput {
    /// Directory containing the generated `.lean` files.
    pub lean_dir: PathBuf,
    /// List of generated `.lean` files.
    pub lean_files: Vec<PathBuf>,
    /// Raw aeneas stdout+stderr.
    pub tool_output: String,
}

/// Locate the Aeneas binary.
pub fn find_aeneas() -> Result<PathBuf, AeneasError> {
    if let Ok(p) = std::env::var("AENEAS_PATH") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Ok(path);
        }
    }
    Ok(PathBuf::from("aeneas"))
}

/// Translate a `.mir` file into Lean 4 source.
///
/// `mir_path` — the `.mir` file produced by [`crate::mir_export::export_mir`].  
/// `output_dir` — directory where Lean files will be written.
pub async fn translate(
    mir_path: &Path,
    output_dir: &Path,
) -> Result<AeneasOutput, AeneasError> {
    info!(
        mir = %mir_path.display(),
        out = %output_dir.display(),
        "invoking aeneas"
    );

    tokio::fs::create_dir_all(output_dir).await?;

    let aeneas = find_aeneas()?;

    let output = Command::new(&aeneas)
        .arg(mir_path)
        .arg("--dest")
        .arg(output_dir)
        .arg("--backend")
        .arg("lean")
        .output()
        .await?;

    let tool_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if !output.status.success() {
        return Err(AeneasError::Invocation(tool_output));
    }

    let lean_files = collect_lean_files(output_dir).await?;
    if lean_files.is_empty() {
        return Err(AeneasError::NoOutput);
    }

    info!(count = lean_files.len(), "Lean files generated");
    Ok(AeneasOutput {
        lean_dir: output_dir.to_path_buf(),
        lean_files,
        tool_output,
    })
}

async fn collect_lean_files(dir: &Path) -> Result<Vec<PathBuf>, AeneasError> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("lean") {
            files.push(path);
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_aeneas_returns_path() {
        // SAFETY: test-only; no concurrent env access.
        unsafe { std::env::remove_var("AENEAS_PATH") };
        let p = find_aeneas().unwrap();
        assert_eq!(p, PathBuf::from("aeneas"));
    }
}
