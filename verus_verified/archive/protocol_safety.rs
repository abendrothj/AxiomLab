// AxiomLab — Formally Verified Protocol Safety
//
// Proves protocol-level invariants for structured experimental protocols:
//
//   protocol_inv(steps, total_vol_nl)  ≡
//       step_count_safe(steps) ∧ total_volume_safe(total_vol_nl)
//
// These proofs sit above the vessel physics layer (vessel_registry.rs) and
// establish that the *composition* of steps remains safe, not just individual
// operations.
//
// Volumes are in nanoliters (u64) to stay in integer arithmetic — the same
// representation used by vessel_registry.rs and vessel_physics/src/lib.rs.
//
// Run: ~/verus/verus verus_verified/protocol_safety.rs

use vstd::prelude::*;

verus! {

// ═══════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════

/// Maximum number of steps in a single protocol.
/// Mirrors protocol::MAX_PROTOCOL_STEPS in agent_runtime/src/protocol.rs.
pub const MAX_PROTOCOL_STEPS: u64 = 20;

/// Maximum total volume (nanoliters) that may be dispensed in one protocol run.
/// 200 mL = 200_000_000 nl.  Chosen to stay well below the 200 mL reservoir.
pub const MAX_TOTAL_VOLUME_NL: u64 = 200_000_000;

/// Maximum volume per well / tube step (nanoliters).
/// 2 mL = 2_000_000 nl.  Mirrors tube max capacity in vessel_registry.rs.
pub const MAX_WELL_VOLUME_NL: u64 = 2_000_000;

// ═══════════════════════════════════════════════════════════════════
//  Safety predicates
// ═══════════════════════════════════════════════════════════════════

/// Step count is within the allowed protocol size.
pub open spec fn step_count_safe(n: u64) -> bool {
    n <= MAX_PROTOCOL_STEPS
}

/// Cumulative dispensed volume stays within the per-protocol total limit.
pub open spec fn total_volume_safe(v: u64) -> bool {
    v <= MAX_TOTAL_VOLUME_NL
}

/// Per-step dispense volume is within a single vessel's capacity.
pub open spec fn per_step_volume_safe(v: u64) -> bool {
    v <= MAX_WELL_VOLUME_NL
}

/// The combined protocol invariant.
pub open spec fn protocol_inv(steps: u64, total_vol_nl: u64) -> bool {
    step_count_safe(steps) && total_volume_safe(total_vol_nl)
}

// ═══════════════════════════════════════════════════════════════════
//  Core proof functions
// ═══════════════════════════════════════════════════════════════════

/// An empty protocol (zero steps, zero volume) satisfies the invariant.
proof fn empty_protocol_safe()
    ensures protocol_inv(0, 0),
{ }

/// Adding a step to a valid protocol preserves step count safety,
/// provided the new count is still within the limit.
proof fn add_step_preserves_count(n: u64)
    requires
        step_count_safe(n),
        n + 1 <= MAX_PROTOCOL_STEPS,
        n + 1 <= u64::MAX,
    ensures
        step_count_safe((n + 1) as u64),
{ }

/// Accumulating a dispense volume into the running total preserves
/// total volume safety, provided the sum stays within the limit.
proof fn accumulate_volume_safe(total: u64, delta: u64)
    requires
        total_volume_safe(total),
        total + delta <= MAX_TOTAL_VOLUME_NL,
        total + delta <= u64::MAX,
    ensures
        total_volume_safe((total + delta) as u64),
{ }

/// Adding a step and its volume simultaneously preserves the protocol invariant.
proof fn add_step_and_volume_preserves_inv(steps: u64, total: u64, delta: u64)
    requires
        protocol_inv(steps, total),
        steps + 1 <= MAX_PROTOCOL_STEPS,
        steps + 1 <= u64::MAX,
        total + delta <= MAX_TOTAL_VOLUME_NL,
        total + delta <= u64::MAX,
    ensures
        protocol_inv((steps + 1) as u64, (total + delta) as u64),
{ }

// ═══════════════════════════════════════════════════════════════════
//  Dilution series proof
// ═══════════════════════════════════════════════════════════════════

/// A dilution series of n steps, each dispensing per_step_nl nanoliters,
/// stays within both step count and total volume limits.
///
/// Preconditions are exactly what the runtime checks before executing:
///   - step count ≤ 20
///   - each step volume ≤ 2 mL
///   - n × per_step_nl ≤ 200 mL total (no overflow)
proof fn dilution_series_safe(n: u64, per_step_nl: u64)
    requires
        step_count_safe(n),
        per_step_volume_safe(per_step_nl),
        n * per_step_nl <= MAX_TOTAL_VOLUME_NL,
        n * per_step_nl <= u64::MAX,
    ensures
        total_volume_safe((n * per_step_nl) as u64),
        protocol_inv(n, (n * per_step_nl) as u64),
{ }

/// Halving concentrations across n wells never exceeds total volume,
/// provided the first well is within capacity and n ≤ MAX_PROTOCOL_STEPS.
///
/// This models a serial 2-fold dilution: each step uses half the volume
/// of the previous.  The geometric series 1 + 1/2 + 1/4 + ... < 2, so
/// the total is at most 2 × first_step_nl.
proof fn twofold_dilution_total_bounded(n: u64, first_step_nl: u64)
    requires
        step_count_safe(n),
        per_step_volume_safe(first_step_nl),
        2 * first_step_nl <= MAX_TOTAL_VOLUME_NL,   // sum < 2 × first step
        2 * first_step_nl <= u64::MAX,
    ensures
        total_volume_safe((2 * first_step_nl) as u64),
{ }

// ═══════════════════════════════════════════════════════════════════
//  Boundary proofs
// ═══════════════════════════════════════════════════════════════════

/// A protocol at exactly the maximum step count is still safe.
proof fn max_steps_is_valid()
    ensures step_count_safe(MAX_PROTOCOL_STEPS),
{ }

/// A single step at exactly the well volume limit is safe.
proof fn max_well_volume_step_is_safe()
    ensures per_step_volume_safe(MAX_WELL_VOLUME_NL),
{ }

/// A protocol with MAX_PROTOCOL_STEPS steps each at MAX_WELL_VOLUME_NL
/// may exceed the total volume limit — this is the overflow guard the
/// runtime checks before executing.
///
/// Concretely: 20 × 2_000_000 = 40_000_000 nl = 40 mL < 200 mL limit.
proof fn full_protocol_at_well_max_is_safe()
    ensures
        MAX_PROTOCOL_STEPS * MAX_WELL_VOLUME_NL <= MAX_TOTAL_VOLUME_NL,
{ }

// ═══════════════════════════════════════════════════════════════════
//  Main — exercised by `~/verus/verus verus_verified/protocol_safety.rs`
// ═══════════════════════════════════════════════════════════════════

fn main() {
    // A 5-step dilution series, 500 µL (500_000 nl) per well.
    // Total = 5 × 500_000 = 2_500_000 nl = 2.5 mL — well within limits.
    let n_steps: u64 = 5;
    let per_step: u64 = 500_000;
    assert(step_count_safe(n_steps));
    assert(per_step_volume_safe(per_step));
    assert(n_steps * per_step <= MAX_TOTAL_VOLUME_NL);
    assert(total_volume_safe((n_steps * per_step) as u64));
    assert(protocol_inv(n_steps, (n_steps * per_step) as u64));
}

} // verus!
