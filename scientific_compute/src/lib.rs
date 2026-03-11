//! # scientific_compute
//!
//! Pure-Rust scientific primitives for AxiomLab.
//! Built on `nalgebra` – no C/Fortran FFI – so every operation
//! stays within Rust's memory-safety guarantees.

pub mod linalg;
pub mod fft;
pub mod discovery;
pub mod lab_data;

/// Re-export core numeric types used across the workspace.
pub use nalgebra;
