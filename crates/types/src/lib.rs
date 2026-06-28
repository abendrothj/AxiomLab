//! Shared domain types for AxiomLab.
//!
//! This crate holds the vocabulary the rest of the system speaks: the [`Action`]
//! an LLM proposes, the [`Rejection`] a gate returns, the [`RiskClass`] taxonomy,
//! physical-quantity newtypes, and laboratory state. It contains **zero business
//! logic** — no gates, no signing, no I/O beyond `LabState` load/save helpers.

mod action;
mod lab_state;
mod quantities;

pub use action::{Action, RejectedAction, Rejection, RiskClass};
pub use lab_state::{LabState, Reagent, VesselContribution};
pub use quantities::{Ph, TempC, VolumeUl};
