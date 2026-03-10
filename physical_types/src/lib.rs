//! # physical_types
//!
//! Compile-time dimensional analysis for AxiomLab.
//!
//! Wraps the `uom` (Units of Measurement) crate so that AI-generated code
//! cannot mix incompatible physical quantities.  An attempt to add a `Mass`
//! to a `Velocity`, for example, is a **compile-time error**.

pub mod quantities;
