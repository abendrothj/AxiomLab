// AxiomLab — Formally Verified Vessel Physics
//
// Proves that VesselRegistry.dispense and VesselRegistry.aspirate preserve
// the volume invariant:
//
//   vessel_inv(volume_nl, max_nl)  ≡  volume_nl ≤ max_nl
//
// In Verus proof/spec contexts, arithmetic on u64 values produces `int`
// (unbounded mathematical integer).  Calls to spec functions that take u64
// arguments use `(expr) as u64` casts, which are unchecked — correctness
// follows from the overflow-prevention preconditions.
//
// Run: ~/verus/verus verus_verified/vessel_registry.rs

use vstd::prelude::*;

verus! {

// ═══════════════════════════════════════════════════════════════════
//  Invariant definition
// ═══════════════════════════════════════════════════════════════════

/// The vessel volume invariant: fill level never exceeds capacity.
pub open spec fn vessel_inv(volume_nl: u64, max_nl: u64) -> bool {
    volume_nl <= max_nl
}

/// An empty vessel trivially satisfies the invariant.
proof fn empty_satisfies_inv(max_nl: u64)
    ensures vessel_inv(0u64, max_nl),
{ }

// ═══════════════════════════════════════════════════════════════════
//  Core verified arithmetic
// ═══════════════════════════════════════════════════════════════════

/// Dispense `delta_nl` nanoliters into a vessel.
///
/// Preconditions (checked at runtime in VesselRegistry::dispense):
///   1. Invariant holds before the operation.
///   2. The addition will not overflow u64.
///   3. The resulting volume will not exceed max capacity.
///
/// Postcondition (Verus proves this cannot be broken):
///   The invariant holds after the operation.
pub fn proved_add(volume_nl: u64, delta_nl: u64, max_nl: u64) -> (result: u64)
    requires
        vessel_inv(volume_nl, max_nl),
        volume_nl + delta_nl <= max_nl,
        volume_nl + delta_nl <= u64::MAX,
    ensures
        result == volume_nl + delta_nl,
        vessel_inv(result, max_nl),
{
    volume_nl + delta_nl
}

/// Aspirate `delta_nl` nanoliters from a vessel.
///
/// Preconditions (checked at runtime in VesselRegistry::aspirate):
///   1. delta_nl ≤ volume_nl  (no underflow).
///
/// Postcondition: volume strictly decreases and stays ≥ 0.
pub fn proved_sub(volume_nl: u64, delta_nl: u64) -> (result: u64)
    requires
        delta_nl <= volume_nl,
    ensures
        result == volume_nl - delta_nl,
        result <= volume_nl,
{
    volume_nl - delta_nl
}

// ═══════════════════════════════════════════════════════════════════
//  Invariant preservation proofs
// ═══════════════════════════════════════════════════════════════════

/// Proof: if the invariant holds and dispense is within capacity,
/// the invariant holds after dispense.
proof fn dispense_preserves_inv(volume_nl: u64, delta_nl: u64, max_nl: u64)
    requires
        vessel_inv(volume_nl, max_nl),
        volume_nl + delta_nl <= max_nl,
        volume_nl + delta_nl <= u64::MAX,
    ensures
        vessel_inv((volume_nl + delta_nl) as u64, max_nl),
{ }

/// Proof: aspirate always produces a non-negative result ≤ max.
proof fn aspirate_preserves_inv(volume_nl: u64, delta_nl: u64, max_nl: u64)
    requires
        vessel_inv(volume_nl, max_nl),
        delta_nl <= volume_nl,
    ensures
        vessel_inv((volume_nl - delta_nl) as u64, max_nl),
{ }

// ═══════════════════════════════════════════════════════════════════
//  Chain proofs — sequential operations stay safe
// ═══════════════════════════════════════════════════════════════════

/// Proof: two consecutive dispenses remain safe if each is within capacity.
proof fn dispense_chain_safe(v0: u64, d1: u64, d2: u64, max: u64)
    requires
        vessel_inv(v0, max),
        v0 + d1 <= max,
        v0 + d1 + d2 <= max,
        v0 + d1 <= u64::MAX,
        v0 + d1 + d2 <= u64::MAX,
    ensures
        vessel_inv((v0 + d1) as u64, max),
        vessel_inv((v0 + d1 + d2) as u64, max),
{ }

/// Proof: aspirating exactly what was dispensed returns to the original volume.
proof fn aspirate_inverts_dispense(vol: u64, delta: u64, max: u64)
    requires
        vessel_inv(vol, max),
        vol + delta <= max,
        vol + delta <= u64::MAX,
    ensures
        ((vol + delta) - delta) as u64 == vol,
{ }

/// Proof: partial aspirate of a partially filled vessel stays within [0, max].
proof fn partial_aspirate_safe(vol: u64, delta: u64, max: u64)
    requires
        vessel_inv(vol, max),
        delta <= vol,
    ensures
        vessel_inv((vol - delta) as u64, max),
{ }

// ═══════════════════════════════════════════════════════════════════
//  Boundary condition proofs
// ═══════════════════════════════════════════════════════════════════

/// Proof: filling to exactly max capacity is valid.
proof fn fill_to_capacity_is_valid(max: u64)
    ensures vessel_inv(max, max),
{ }

/// Proof: draining to zero is valid.
proof fn drain_to_zero_is_valid(vol: u64, max: u64)
    requires vessel_inv(vol, max),
    ensures vessel_inv(0u64, max),
{ }

// ═══════════════════════════════════════════════════════════════════
//  Main — exercised by `~/verus/verus verus_verified/vessel_registry.rs`
// ═══════════════════════════════════════════════════════════════════

fn main() {
    // Dispense 10 000 nl into a 50 000 nl vessel
    let v1 = proved_add(0, 10_000, 50_000);
    assert(v1 == 10_000);
    assert(vessel_inv(v1, 50_000));

    // Dispense another 10 000 nl
    let v2 = proved_add(v1, 10_000, 50_000);
    assert(v2 == 20_000);
    assert(vessel_inv(v2, 50_000));

    // Aspirate 5 000 nl back
    let v3 = proved_sub(v2, 5_000);
    assert(v3 == 15_000);
    assert(vessel_inv(v3, 50_000));

    // Drain completely
    let v4 = proved_sub(v3, v3);
    assert(v4 == 0);
    assert(vessel_inv(v4, 50_000));
}

} // verus!
