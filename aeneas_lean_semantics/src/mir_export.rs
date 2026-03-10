//! Utilities for exporting Rust MIR to a format consumable by Aeneas.
//!
//! Skeleton – the real pipeline will invoke `rustc` to emit MIR JSON
//! and then call the `aeneas` binary to produce Lean 4 files.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("MIR export not yet implemented")]
    NotImplemented,
}

/// Export the MIR for a given crate, returning the path to the
/// generated Lean 4 file.
pub fn export_mir(_crate_name: &str) -> Result<std::path::PathBuf, ExportError> {
    Err(ExportError::NotImplemented)
}
