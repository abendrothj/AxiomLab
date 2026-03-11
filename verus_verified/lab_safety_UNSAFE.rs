// AxiomLab — Deliberately UNSAFE lab code
//
// This file demonstrates that Verus REJECTS code that violates safety.
// An LLM agent might generate commands like these — Verus catches them
// at compile time, BEFORE any hardware is actuated.
//
// Run: verus verus_verified/lab_safety_UNSAFE.rs
// Expected: VERIFICATION FAILURE

use vstd::prelude::*;

verus! {

pub const MAX_ARM_EXTENSION_MM: u64 = 1200;
pub const MIN_ARM_EXTENSION_MM: u64 = 0;
pub const MAX_TEMPERATURE_MILLI_K: u64 = 500_000;
pub const MIN_TEMPERATURE_MILLI_K: u64 = 200_000;
pub const MAX_PRESSURE_PA: u64 = 200_000;

pub open spec fn arm_in_range(mm: u64) -> bool {
    MIN_ARM_EXTENSION_MM <= mm && mm <= MAX_ARM_EXTENSION_MM
}

pub open spec fn temp_in_range(milli_k: u64) -> bool {
    MIN_TEMPERATURE_MILLI_K <= milli_k && milli_k <= MAX_TEMPERATURE_MILLI_K
}

pub open spec fn pressure_in_range(pa: u64) -> bool {
    pa <= MAX_PRESSURE_PA
}

pub fn move_arm(mm: u64) -> (result: u64)
    requires arm_in_range(mm),
    ensures result == mm,
{
    mm
}

pub fn set_temperature(milli_k: u64) -> (result: u64)
    requires temp_in_range(milli_k),
    ensures result == milli_k,
{
    milli_k
}

pub fn set_pressure(pa: u64) -> (result: u64)
    requires pressure_in_range(pa),
    ensures result == pa,
{
    pa
}

// ═══════════════════════════════════════════════════════════════════
//  BUG 1: Arm extension beyond physical limit (2000 mm > 1200 mm)
//  An LLM might hallucinate "extend arm to 2 meters" — REJECTED
// ═══════════════════════════════════════════════════════════════════
fn bug_arm_overextend() {
    let _ = move_arm(2000); // UNSAFE: 2000 > 1200
}

// ═══════════════════════════════════════════════════════════════════
//  BUG 2: Temperature below safe minimum (100K < 200K)
//  Cryogenic temps could shatter glass vessels — REJECTED
// ═══════════════════════════════════════════════════════════════════
fn bug_cryo_temperature() {
    let _ = set_temperature(100_000); // UNSAFE: 100K < 200K minimum
}

// ═══════════════════════════════════════════════════════════════════
//  BUG 3: Temperature above safe maximum (1000K > 500K)
//  Could ignite solvents or destroy samples — REJECTED
// ═══════════════════════════════════════════════════════════════════
fn bug_overheat() {
    let _ = set_temperature(1_000_000); // UNSAFE: 1000K > 500K maximum
}

// ═══════════════════════════════════════════════════════════════════
//  BUG 4: Pressure exceeding vessel rating (500 kPa > 200 kPa)
//  Could cause explosion — REJECTED
// ═══════════════════════════════════════════════════════════════════
fn bug_overpressure() {
    let _ = set_pressure(500_000); // UNSAFE: 500kPa > 200kPa
}

fn main() {}

} // verus!
