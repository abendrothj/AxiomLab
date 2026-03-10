//! # aeneas_lean_semantics
//!
//! Bridges AxiomLab's Rust compute kernels to Lean 4 via the Aeneas
//! toolchain, enabling theorem-prover-level guarantees of algorithmic
//! correctness (e.g., verifying that a chemistry simulation accurately
//! implements its governing equations).
//!
//! ## Pipeline
//!
//! 1. [`mir_export`] — invoke `rustc` to emit MIR for a target crate.
//! 2. [`aeneas`] — run the Aeneas binary to translate MIR → Lean 4.
//! 3. [`lean`] — invoke `lean` to type-check the generated `.lean` files.

pub mod mir_export;
pub mod aeneas;
pub mod lean;
