//! # verus_proofs
//!
//! Hardware safety enforcement for AxiomLab, backed by formal verification.
//!
//! ## Architecture
//!
//! The single source of truth for all safety constants and predicates is
//! `verus_verified/lab_safety.rs`, which is formally verified by the real
//! Verus compiler + Z3 SMT solver.
//!
//! At build time, `build.rs` extracts the constants from that file and
//! generates a Rust source file that `hardware_bounds.rs` includes.
//! This guarantees the runtime uses EXACTLY the same bounds that were
//! formally proven safe.
//!
//! ## Modules
//!
//! - [`hardware_bounds`] — Safety constants (from Verus source) and
//!   runtime-checked actuator functions.
//! - [`concurrency`] — Token-based channel ownership for safe hardware
//!   sharing across concurrent tasks.
//! - [`resource_allocator`] — Fixed-capacity resource pools and well plates.
//! - [`verify`] — Driver to invoke the real Verus compiler on the source
//!   of truth and confirm proofs still hold.
//! - [`verus_shim`] — Legacy compatibility macros (deprecated).

pub mod verus_shim;
pub mod hardware_bounds;
pub mod concurrency;
pub mod resource_allocator;
pub mod verify;
