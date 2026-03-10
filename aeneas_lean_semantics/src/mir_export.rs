//! Emit Rust MIR for a target crate via `rustc`.
//!
//! Invokes `cargo rustc` with `--emit=mir` to produce the `.mir` file
//! that Aeneas consumes.

use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::process::Command;
use tracing::info;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("cargo/rustc invocation failed: {0}")]
    Invocation(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("MIR file not found after compilation")]
    MirNotFound,
}

/// Result of a MIR export.
#[derive(Debug)]
pub struct MirArtifact {
    /// Path to the emitted `.mir` file.
    pub mir_path: PathBuf,
    /// Raw `rustc` output (for debugging).
    pub compiler_output: String,
}

/// Export MIR for the crate at `crate_dir`.
///
/// Runs `cargo rustc` in the given directory with `-- --emit=mir`
/// and then locates the generated `.mir` file under `target/`.
pub async fn export_mir(crate_dir: &Path) -> Result<MirArtifact, ExportError> {
    info!(crate_dir = %crate_dir.display(), "exporting MIR");

    let output = Command::new("cargo")
        .arg("rustc")
        .arg("--")
        .arg("--emit=mir")
        .current_dir(crate_dir)
        .output()
        .await?;

    let compiler_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if !output.status.success() {
        return Err(ExportError::Invocation(compiler_output));
    }

    // Search `target/debug/deps/` for the `.mir` file.
    let deps_dir = crate_dir.join("target/debug/deps");
    let mir_path = find_mir_file(&deps_dir).await?;

    info!(path = %mir_path.display(), "MIR exported");
    Ok(MirArtifact {
        mir_path,
        compiler_output,
    })
}

async fn find_mir_file(dir: &Path) -> Result<PathBuf, ExportError> {
    let mut entries = tokio::fs::read_dir(dir).await.map_err(|_| ExportError::MirNotFound)?;
    while let Some(entry) = entries.next_entry().await.map_err(|_| ExportError::MirNotFound)? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("mir") {
            return Ok(path);
        }
    }
    Err(ExportError::MirNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_error_display() {
        let e = ExportError::MirNotFound;
        assert_eq!(e.to_string(), "MIR file not found after compilation");
    }
}
