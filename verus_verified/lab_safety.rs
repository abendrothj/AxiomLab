// AxiomLab — Formally Verified Hardware Safety Bounds
//
// This file is verified by the REAL Verus compiler, proving at compile time
// that robotic arm movements, temperature setpoints, pressure limits, and
// liquid dispensing volumes can NEVER violate physical safety constraints —
// even if an LLM agent hallucinates dangerous commands.
//
// This is run via: verus verus_verified/lab_safety.rs

use vstd::prelude::*;

verus! {

// ═══════════════════════════════════════════════════════════════════
//  Physical constants — hardware safety envelope
// ═══════════════════════════════════════════════════════════════════

pub const MAX_ARM_EXTENSION_MM: u64 = 1200;
pub const MIN_ARM_EXTENSION_MM: u64 = 0;

pub const MAX_TEMPERATURE_MILLI_K: u64 = 500_000;  // 500 K
pub const MIN_TEMPERATURE_MILLI_K: u64 = 200_000;  // 200 K

pub const MAX_PRESSURE_PA: u64 = 200_000;           // 200 kPa

pub const MAX_VOLUME_UL: u64 = 50_000;              // 50 mL syringe

// ═══════════════════════════════════════════════════════════════════
//  Spec functions — define the safety predicates
// ═══════════════════════════════════════════════════════════════════

pub open spec fn arm_in_range(mm: u64) -> bool {
    MIN_ARM_EXTENSION_MM <= mm && mm <= MAX_ARM_EXTENSION_MM
}

pub open spec fn temp_in_range(milli_k: u64) -> bool {
    MIN_TEMPERATURE_MILLI_K <= milli_k && milli_k <= MAX_TEMPERATURE_MILLI_K
}

pub open spec fn pressure_in_range(pa: u64) -> bool {
    pa <= MAX_PRESSURE_PA
}

pub open spec fn volume_in_range(ul: u64) -> bool {
    ul <= MAX_VOLUME_UL
}

// ═══════════════════════════════════════════════════════════════════
//  Verified exec functions — the ONLY way hardware gets commanded
// ═══════════════════════════════════════════════════════════════════

/// Move the robotic arm to position `mm`.
/// Verus proves: this function can ONLY be called with a safe value.
pub fn move_arm(mm: u64) -> (result: u64)
    requires
        arm_in_range(mm),
    ensures
        result == mm,
        arm_in_range(result),
{
    mm
}

/// Set the thermal controller to `milli_k` millikelvin.
pub fn set_temperature(milli_k: u64) -> (result: u64)
    requires
        temp_in_range(milli_k),
    ensures
        result == milli_k,
        temp_in_range(result),
{
    milli_k
}

/// Set the pressure regulator to `pa` Pascals.
pub fn set_pressure(pa: u64) -> (result: u64)
    requires
        pressure_in_range(pa),
    ensures
        result == pa,
        pressure_in_range(result),
{
    pa
}

/// Dispense `ul` microlitres from the syringe pump.
pub fn dispense(ul: u64) -> (result: u64)
    requires
        volume_in_range(ul),
    ensures
        result == ul,
        volume_in_range(result),
{
    ul
}

// ═══════════════════════════════════════════════════════════════════
//  Safe wrappers — runtime check + verified dispatch
// ═══════════════════════════════════════════════════════════════════

/// Safely command the arm: returns Ok(mm) if in range, Err otherwise.
/// Verus proves: the Ok path ALWAYS satisfies the safety invariant.
pub fn safe_move_arm(mm: u64) -> (result: Result<u64, u64>)
    ensures
        result.is_ok() <==> arm_in_range(mm),
        result.is_ok() ==> arm_in_range(result.unwrap()),
        result.is_ok() ==> result.unwrap() == mm,
{
    if MIN_ARM_EXTENSION_MM <= mm && mm <= MAX_ARM_EXTENSION_MM {
        Ok(move_arm(mm))
    } else {
        Err(mm)
    }
}

/// Safely set temperature: returns Ok(milli_k) if in range, Err otherwise.
pub fn safe_set_temperature(milli_k: u64) -> (result: Result<u64, u64>)
    ensures
        result.is_ok() <==> temp_in_range(milli_k),
        result.is_ok() ==> temp_in_range(result.unwrap()),
        result.is_ok() ==> result.unwrap() == milli_k,
{
    if MIN_TEMPERATURE_MILLI_K <= milli_k && milli_k <= MAX_TEMPERATURE_MILLI_K {
        Ok(set_temperature(milli_k))
    } else {
        Err(milli_k)
    }
}

/// Safely set pressure: returns Ok(pa) if in range, Err otherwise.
pub fn safe_set_pressure(pa: u64) -> (result: Result<u64, u64>)
    ensures
        result.is_ok() <==> pressure_in_range(pa),
        result.is_ok() ==> pressure_in_range(result.unwrap()),
        result.is_ok() ==> result.unwrap() == pa,
{
    if pa <= MAX_PRESSURE_PA {
        Ok(set_pressure(pa))
    } else {
        Err(pa)
    }
}

/// Safely dispense: returns Ok(ul) if in range, Err otherwise.
pub fn safe_dispense(ul: u64) -> (result: Result<u64, u64>)
    ensures
        result.is_ok() <==> volume_in_range(ul),
        result.is_ok() ==> volume_in_range(result.unwrap()),
        result.is_ok() ==> result.unwrap() == ul,
{
    if ul <= MAX_VOLUME_UL {
        Ok(dispense(ul))
    } else {
        Err(ul)
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Composite safety — multi-axis command validation
// ═══════════════════════════════════════════════════════════════════

/// A complete lab command: move arm + set temperature + dispense.
/// Verus proves the composition is safe if each component is safe.
pub open spec fn lab_command_safe(
    arm_mm: u64,
    temp_mk: u64,
    pressure_pa: u64,
    volume_ul: u64,
) -> bool {
    arm_in_range(arm_mm)
    && temp_in_range(temp_mk)
    && pressure_in_range(pressure_pa)
    && volume_in_range(volume_ul)
}

/// Execute a full lab command — all four actuators at once.
/// Verus proves: if this returns Ok, ALL actuator values are safe.
pub fn execute_lab_command(
    arm_mm: u64,
    temp_mk: u64,
    pressure_pa: u64,
    volume_ul: u64,
) -> (result: Result<(u64, u64, u64, u64), &'static str>)
    ensures
        result.is_ok() <==> lab_command_safe(arm_mm, temp_mk, pressure_pa, volume_ul),
        result.is_ok() ==> lab_command_safe(
            result.unwrap().0,
            result.unwrap().1,
            result.unwrap().2,
            result.unwrap().3,
        ),
{
    // Validate each axis
    if !(MIN_ARM_EXTENSION_MM <= arm_mm && arm_mm <= MAX_ARM_EXTENSION_MM) {
        return Err("arm out of range");
    }
    if !(MIN_TEMPERATURE_MILLI_K <= temp_mk && temp_mk <= MAX_TEMPERATURE_MILLI_K) {
        return Err("temperature out of range");
    }
    if !(pressure_pa <= MAX_PRESSURE_PA) {
        return Err("pressure out of range");
    }
    if !(volume_ul <= MAX_VOLUME_UL) {
        return Err("volume out of range");
    }

    // All validated — dispatch to verified actuators
    let a = move_arm(arm_mm);
    let t = set_temperature(temp_mk);
    let p = set_pressure(pressure_pa);
    let v = dispense(volume_ul);

    Ok((a, t, p, v))
}

// ═══════════════════════════════════════════════════════════════════
//  Proof: monotonic safety — clamped values stay safe
// ═══════════════════════════════════════════════════════════════════

/// Spec version of arm clamping for use in proofs.
pub open spec fn spec_clamp_arm(mm: u64) -> u64 {
    if mm < MIN_ARM_EXTENSION_MM {
        MIN_ARM_EXTENSION_MM
    } else if mm > MAX_ARM_EXTENSION_MM {
        MAX_ARM_EXTENSION_MM
    } else {
        mm
    }
}

/// Clamp a value to the arm range. Verus proves the output is always safe.
pub fn clamp_arm(mm: u64) -> (result: u64)
    ensures
        arm_in_range(result),
        result == spec_clamp_arm(mm),
        mm <= MAX_ARM_EXTENSION_MM && mm >= MIN_ARM_EXTENSION_MM ==> result == mm,
{
    if mm < MIN_ARM_EXTENSION_MM {
        MIN_ARM_EXTENSION_MM
    } else if mm > MAX_ARM_EXTENSION_MM {
        MAX_ARM_EXTENSION_MM
    } else {
        mm
    }
}

/// Proof: clamping is idempotent — clamping an already-safe value is identity.
proof fn clamp_idempotent(mm: u64)
    requires arm_in_range(mm),
    ensures spec_clamp_arm(mm) == mm,
{
}

// ═══════════════════════════════════════════════════════════════════
//  Main — exercised by test runner
// ═══════════════════════════════════════════════════════════════════

fn main() {
    // These calls are verified at compile time — preconditions met.
    let a = move_arm(600);
    let t = set_temperature(300_000);
    let p = set_pressure(101_325);
    let v = dispense(5_000);

    // Safe wrappers handle runtime validation.
    let ok = safe_move_arm(500);
    assert(ok.is_ok());

    let bad = safe_move_arm(9999);
    assert(bad.is_err());

    // Composite command.
    let cmd = execute_lab_command(600, 300_000, 101_325, 5_000);
    assert(cmd.is_ok());
}

} // verus!
