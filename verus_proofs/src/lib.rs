//! # verus_proofs
//!
//! The formally-verified hardware safety envelope, and the bridge that makes the
//! runtime use exactly the bounds Verus proved.
//!
//! The single source of truth is `verus_verified/lab_safety.rs`, verified by the
//! real Verus compiler + Z3 (see `.github/workflows/verus.yml`). At build time,
//! `build.rs` extracts that file's constants into a generated Rust file that
//! [`hardware_bounds`] includes — so "what runs" is mechanically derived from
//! "what was proven," not hand-copied.
//!
//! ## Modules
//! - [`hardware_bounds`] — verified safety constants + runtime-checked bound predicates.
//! - [`verify`] — driver that re-invokes the Verus compiler on the source of truth.

pub mod hardware_bounds;
pub mod verify;
