//! Verus verification driver.
//!
//! Provides [`verify_lab_safety`] which invokes the real Verus compiler
//! on `verus_verified/lab_safety.rs` — the source of truth for all
//! hardware safety constants.  This function is called by CI and
//! integration tests to confirm the proofs still hold.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of running the Verus compiler.
#[derive(Debug)]
pub struct VerificationResult {
    /// True if Verus exited with code 0 (all proofs discharged).
    pub success: bool,
    /// Number of functions verified (parsed from Verus output).
    pub verified_count: u32,
    /// Number of errors (parsed from Verus output).
    pub error_count: u32,
    /// Raw Verus output.
    pub output: String,
}

/// Locate the Verus binary from `VERUS_PATH` env var or `$PATH`.
pub fn find_verus() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VERUS_PATH") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Some(path);
        }
    }
    // Check if `verus` is on PATH
    Command::new("which")
        .arg("verus")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

/// Locate the `verus_verified/` directory relative to the workspace root.
fn find_verus_source_dir() -> Option<PathBuf> {
    // Try CARGO_MANIFEST_DIR (available in tests and build scripts)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let workspace_root = Path::new(&manifest_dir).parent()?;
        let dir = workspace_root.join("verus_verified");
        if dir.exists() {
            return Some(dir);
        }
    }
    // Fallback: current working directory
    let cwd = std::env::current_dir().ok()?;
    let dir = cwd.join("verus_verified");
    if dir.exists() {
        return Some(dir);
    }
    None
}

/// Parse Verus output line like `verification results:: 18 verified, 0 errors`
fn parse_verus_summary(output: &str) -> (u32, u32) {
    for line in output.lines() {
        if line.contains("verification results::") {
            let verified = line
                .split_whitespace()
                .zip(line.split_whitespace().skip(1))
                .find(|(_, w)| *w == "verified," || *w == "verified")
                .and_then(|(n, _)| n.parse::<u32>().ok())
                .unwrap_or(0);
            let errors = line
                .split_whitespace()
                .zip(line.split_whitespace().skip(1))
                .find(|(_, w)| *w == "errors" || *w == "error")
                .and_then(|(n, _)| n.parse::<u32>().ok())
                .unwrap_or(0);
            return (verified, errors);
        }
    }
    (0, 0)
}

/// Run the Verus compiler on `verus_verified/lab_safety.rs`.
///
/// Returns `None` if Verus is not installed.
pub fn verify_lab_safety() -> Option<VerificationResult> {
    let verus = find_verus()?;
    let source_dir = find_verus_source_dir()?;
    let source_file = source_dir.join("lab_safety.rs");

    if !source_file.exists() {
        return None;
    }

    let output = Command::new(&verus)
        .arg(&source_file)
        .output()
        .ok()?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let (verified, errors) = parse_verus_summary(&combined);

    Some(VerificationResult {
        success: output.status.success(),
        verified_count: verified,
        error_count: errors,
        output: combined,
    })
}

/// Run the Verus compiler on an arbitrary `.rs` file.
///
/// Returns `None` if Verus is not installed.
pub fn verify_file(path: &Path) -> Option<VerificationResult> {
    let verus = find_verus()?;

    let output = Command::new(&verus)
        .arg(path)
        .output()
        .ok()?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let (verified, errors) = parse_verus_summary(&combined);

    Some(VerificationResult {
        success: output.status.success(),
        verified_count: verified,
        error_count: errors,
        output: combined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_verus_output() {
        let output = "verification results:: 18 verified, 0 errors\n";
        let (v, e) = parse_verus_summary(output);
        assert_eq!(v, 18);
        assert_eq!(e, 0);
    }

    #[test]
    fn parse_verus_output_with_errors() {
        let output = "verification results:: 9 verified, 4 errors\nerror: aborting\n";
        let (v, e) = parse_verus_summary(output);
        assert_eq!(v, 9);
        assert_eq!(e, 4);
    }
}
