//! Binary self-attestation.
//!
//! Computes a SHA-256 hash of the running executable image from disk so that
//! the `binary_hash` in [`ExecutionContext`] can be measured rather than
//! self-reported by the process.
//!
//! In a full production deployment this would be replaced by a TPM PCR quote
//! or a remote attestation service.  This module provides the first step:
//! reading the binary off disk and hashing it, which defeats adversaries who
//! simply pass a forged hash string.
//!
//! # Usage
//! ```rust,ignore
//! let binary_hash = attest_self().unwrap_or_else(|e| {
//!     tracing::warn!("binary attestation failed: {e}");
//!     "unknown".to_string()
//! });
//! ```

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AttestationError {
    #[error("could not resolve current executable path: {0}")]
    ExePath(#[from] std::io::Error),
    #[error("current executable path is empty")]
    EmptyPath,
}

/// Return the SHA-256 hex digest of the running binary's on-disk image.
///
/// This calls [`std::env::current_exe`] to locate the binary and reads it
/// directly.  The hash is computed in streaming 64 KiB chunks to avoid large
/// heap allocations.
pub fn attest_self() -> Result<String, AttestationError> {
    let path = current_exe_path()?;
    hash_file(&path)
}

fn current_exe_path() -> Result<PathBuf, AttestationError> {
    let path = std::env::current_exe()?;
    if path.as_os_str().is_empty() {
        return Err(AttestationError::EmptyPath);
    }
    // On Linux, `current_exe` may return a path under /proc/self/exe; resolve
    // the symlink so we read the actual binary rather than the symlink target.
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

fn hash_file(path: &std::path::Path) -> Result<String, AttestationError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Build an [`ExecutionContext`] whose `binary_hash` is measured from disk
/// rather than self-reported.
///
/// Falls back to `"unknown"` with a warning if attestation fails (e.g., in
/// environments where the binary path is unavailable).
pub fn measured_execution_context(
    git_commit: &str,
    container_image_digest: Option<String>,
    device_id: Option<String>,
    firmware_version: Option<String>,
) -> proof_artifacts::policy::ExecutionContext {
    let binary_hash = attest_self().unwrap_or_else(|e| {
        tracing::warn!("binary attestation unavailable, using 'unknown': {e}");
        "unknown".to_string()
    });

    proof_artifacts::policy::ExecutionContext {
        git_commit: git_commit.to_owned(),
        binary_hash,
        container_image_digest,
        device_id,
        firmware_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attest_self_returns_hex_sha256() {
        let hash = attest_self().expect("should be able to hash own binary in tests");
        assert_eq!(hash.len(), 64, "SHA-256 hex digest must be 64 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be lowercase hex"
        );
    }

    #[test]
    fn attest_self_is_deterministic() {
        let h1 = attest_self().unwrap();
        let h2 = attest_self().unwrap();
        assert_eq!(h1, h2, "same binary must produce same hash across calls");
    }
}
