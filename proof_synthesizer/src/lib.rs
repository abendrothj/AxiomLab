//! # proof_synthesizer
//!
//! VeruSAGE-inspired agentic proof generator for AxiomLab.
//!
//! Implements an observation → reasoning → action loop:
//! 1. Receive a candidate Rust snippet from the scientific agent.
//! 2. Generate preliminary Verus proof annotations.
//! 3. Invoke the Verus compiler; if verification fails, parse the
//!    error diagnostics and refine the proof.
//! 4. Repeat until the proof is accepted or a retry limit is hit.

pub mod agent;
pub mod compiler;
pub mod diagnostics;
