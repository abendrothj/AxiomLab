//! Hardware-bound safety enforcement.
//!
//! Constants are AUTO-GENERATED from `verus_verified/lab_safety.rs` at
//! build time.  That file is the single source of truth — formally
//! verified by the real Verus compiler + Z3 SMT solver.
//!
//! The runtime functions here mirror the verified Verus functions
//! exactly: same bounds, same logic, same semantics.  The difference
//! is that Verus proves safety at *compile time* (preconditions are
//! statically discharged), while these functions enforce safety at
//! *runtime* (preconditions are checked dynamically).
//!
//! This duality guarantees:
//! - The constants used at runtime are IDENTICAL to those Verus verified.
//! - Any constant change requires editing the Verus source and re-verifying.

// ── Constants: generated from verus_verified/lab_safety.rs ───────

include!(concat!(env!("OUT_DIR"), "/verus_constants.rs"));

// ── Runtime predicate functions (mirrors of Verus spec fns) ──────

/// Arm extension within safe range.
/// Mirrors: `pub open spec fn arm_in_range` in lab_safety.rs
#[inline]
pub fn arm_in_range(mm: u64) -> bool {
    MIN_ARM_EXTENSION_MM <= mm && mm <= MAX_ARM_EXTENSION_MM
}

/// Temperature within safe operating envelope.
/// Mirrors: `pub open spec fn temp_in_range` in lab_safety.rs
#[inline]
pub fn temp_in_range(mk: u64) -> bool {
    MIN_TEMPERATURE_MILLI_K <= mk && mk <= MAX_TEMPERATURE_MILLI_K
}

/// Pressure within vessel rating.
/// Mirrors: `pub open spec fn pressure_in_range` in lab_safety.rs
#[inline]
pub fn pressure_in_range(pa: u64) -> bool {
    pa <= MAX_PRESSURE_PA
}

/// Dispense volume within syringe capacity.
/// Mirrors: `pub open spec fn volume_in_range` in lab_safety.rs
#[inline]
pub fn volume_in_range(ul: u64) -> bool {
    ul <= MAX_VOLUME_UL
}

// ── Runtime-checked safety functions ─────────────────────────────
//
// These mirror the Verus `safe_*` functions in lab_safety.rs.
// Same logic: validate → dispatch or reject.

/// Command the robotic arm to extend to `mm` millimetres.
/// Returns `Err` if `mm` is outside the verified safety range.
pub fn move_arm_verified(mm: u64) -> Result<u64, &'static str> {
    if arm_in_range(mm) { Ok(mm) } else { Err("arm position out of verified range") }
}

/// Set reactor temperature to `mk` milli-kelvins.
pub fn set_temperature_verified(mk: u64) -> Result<u64, &'static str> {
    if temp_in_range(mk) { Ok(mk) } else { Err("temperature out of verified range") }
}

/// Set chamber pressure to `pa` pascals.
pub fn set_pressure_verified(pa: u64) -> Result<u64, &'static str> {
    if pressure_in_range(pa) { Ok(pa) } else { Err("pressure out of verified range") }
}

/// Dispense `ul` microlitres from the syringe pump.
pub fn dispense_verified(ul: u64) -> Result<u64, &'static str> {
    if volume_in_range(ul) { Ok(ul) } else { Err("volume out of verified range") }
}

/// Execute a composite lab command — all four actuators.
/// Mirrors: `pub fn execute_lab_command` in lab_safety.rs.
pub fn execute_lab_command(
    arm_mm: u64,
    temp_mk: u64,
    pressure_pa: u64,
    volume_ul: u64,
) -> Result<(u64, u64, u64, u64), &'static str> {
    if !arm_in_range(arm_mm) { return Err("arm out of range"); }
    if !temp_in_range(temp_mk) { return Err("temperature out of range"); }
    if !pressure_in_range(pressure_pa) { return Err("pressure out of range"); }
    if !volume_in_range(volume_ul) { return Err("volume out of range"); }
    Ok((arm_mm, temp_mk, pressure_pa, volume_ul))
}

/// Clamp arm value to the safe range.
/// Mirrors: `pub fn clamp_arm` in lab_safety.rs.
pub fn clamp_arm(mm: u64) -> u64 {
    if mm < MIN_ARM_EXTENSION_MM {
        MIN_ARM_EXTENSION_MM
    } else if mm > MAX_ARM_EXTENSION_MM {
        MAX_ARM_EXTENSION_MM
    } else {
        mm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── arm bounds ──
    #[test]
    fn arm_within_bounds() {
        assert!(arm_in_range(500));
        assert_eq!(move_arm_verified(500), Ok(500));
    }

    #[test]
    fn arm_at_limit() {
        assert!(arm_in_range(MAX_ARM_EXTENSION_MM));
    }

    #[test]
    fn arm_over_limit() {
        assert!(!arm_in_range(MAX_ARM_EXTENSION_MM + 1));
        assert!(move_arm_verified(MAX_ARM_EXTENSION_MM + 1).is_err());
    }

    // ── temperature bounds ──
    #[test]
    fn temp_valid() {
        assert!(temp_in_range(300_000)); // 300 K
    }

    #[test]
    fn temp_too_cold() {
        assert!(!temp_in_range(100_000)); // 100 K – below range
    }

    #[test]
    fn temp_too_hot() {
        assert!(!temp_in_range(600_000)); // 600 K – above range
    }

    // ── pressure bounds ──
    #[test]
    fn pressure_ok() {
        assert!(pressure_in_range(101_325)); // ~1 atm
    }

    #[test]
    fn pressure_over() {
        assert!(!pressure_in_range(300_000));
        assert!(set_pressure_verified(300_000).is_err());
    }

    // ── volume bounds ──
    #[test]
    fn volume_ok() {
        assert!(volume_in_range(1_000)); // 1 mL
    }

    #[test]
    fn volume_over() {
        assert!(!volume_in_range(60_000)); // 60 mL – over capacity
    }

    // ── composite command ──
    #[test]
    fn composite_command_safe() {
        let result = execute_lab_command(600, 300_000, 101_325, 5_000);
        assert!(result.is_ok());
    }

    #[test]
    fn composite_command_rejects_bad_arm() {
        assert!(execute_lab_command(5000, 300_000, 101_325, 5_000).is_err());
    }

    // ── clamp ──
    #[test]
    fn clamp_identity_in_range() {
        assert_eq!(clamp_arm(600), 600);
    }

    #[test]
    fn clamp_to_max() {
        assert_eq!(clamp_arm(9999), MAX_ARM_EXTENSION_MM);
    }

    // ── constants come from Verus source ──
    #[test]
    fn constants_match_verus_source() {
        // These values must match verus_verified/lab_safety.rs exactly.
        // If this test fails, the build.rs extraction is broken.
        assert_eq!(MAX_ARM_EXTENSION_MM, 1200);
        assert_eq!(MIN_ARM_EXTENSION_MM, 0);
        assert_eq!(MAX_TEMPERATURE_MILLI_K, 500_000);
        assert_eq!(MIN_TEMPERATURE_MILLI_K, 200_000);
        assert_eq!(MAX_PRESSURE_PA, 200_000);
        assert_eq!(MAX_VOLUME_UL, 50_000);
    }
}
