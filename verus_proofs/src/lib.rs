//! # verus_proofs
//!
//! Houses Verus proof specifications for AxiomLab.
//!
//! Proofs verify:
//! - Concurrent hardware-control code is free of data races.
//! - Array accesses remain within physical hardware bounds.
//! - Resource allocators satisfy invariants.
//!
//! When compiled under standard `rustc` the proof blocks are inert stubs;
//! the Verus compiler driver activates them for full SMT verification.

pub mod hardware_bounds;
