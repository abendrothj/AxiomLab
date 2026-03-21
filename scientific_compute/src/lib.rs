//! # scientific_compute
//!
//! Pure-Rust scientific primitives for AxiomLab.
//! Built on `nalgebra` – no C/Fortran FFI – so every operation
//! stays within Rust's memory-safety guarantees.

pub mod doe;
pub mod fft;
pub mod fitting;
pub mod lab_data;
pub mod linalg;
pub mod rsm;
pub mod stats;

/// Re-export core numeric types used across the workspace.
pub use nalgebra;
