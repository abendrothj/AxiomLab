//! # agent_runtime
//!
//! Sandboxed agent orchestrator for AxiomLab.
//!
//! Provides a restricted control-plane that mediates every interaction
//! between the LLM agent and laboratory hardware, enforcing allowlists
//! and resource limits.

pub mod sandbox;
pub mod llm;
pub mod tools;
pub mod experiment;
pub mod orchestrator;
pub mod protocol;
pub mod reasoning;
pub mod audit;
pub mod rekor;
pub mod capabilities;
pub mod approvals;
pub mod approval_queue;
pub mod revocation;
pub mod attestation;
pub mod events;
pub mod hardware;
