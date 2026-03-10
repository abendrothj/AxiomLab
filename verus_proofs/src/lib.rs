//! # verus_proofs
//!
//! Houses Verus proof specifications for AxiomLab.
//!
//! Proofs verify:
//! - Concurrent hardware-control code is free of data races.
//! - Array accesses remain within physical hardware bounds.
//! - Resource allocators satisfy invariants.
//!
//! ## Dual-compile strategy
//!
//! Under standard `rustc` the `verus!{}` macro expands to plain Rust
//! (runtime checks, no proofs).  Under the Verus compiler (feature
//! `verus`) the macro is replaced by the real `verus!` from the
//! `builtin_macros` crate, enabling full SMT verification via Z3.

pub mod verus_shim;
pub mod hardware_bounds;
pub mod concurrency;
pub mod resource_allocator;
