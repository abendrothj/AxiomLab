//! Hardware-bound invariant specifications.
//!
//! Defines physical constraints on laboratory hardware as formal
//! preconditions.  Under Verus these become SMT-verified `requires`
//! clauses; under standard `rustc` they are runtime assertions.

use crate::verus_shim::*;

// ── Physical constants ───────────────────────────────────────────

pub const MAX_ARM_EXTENSION_MM: u64 = 1200;
pub const MIN_ARM_EXTENSION_MM: u64 = 0;
pub const MAX_TEMPERATURE_MILLI_K: u64 = 500_000; // 500 K in mK
pub const MIN_TEMPERATURE_MILLI_K: u64 = 200_000; // 200 K in mK
pub const MAX_PRESSURE_PA: u64 = 200_000;         // 200 kPa
pub const MAX_VOLUME_UL: u64 = 50_000;            // 50 mL in µL

// ── Specification functions ──────────────────────────────────────

// Spec: arm extension must be within [MIN, MAX].
spec_fn!(arm_in_range, (mm: u64) -> bool, {
    mm >= MIN_ARM_EXTENSION_MM && mm <= MAX_ARM_EXTENSION_MM
});

// Spec: temperature must be within the safe operating envelope.
spec_fn!(temp_in_range, (mk: u64) -> bool, {
    mk >= MIN_TEMPERATURE_MILLI_K && mk <= MAX_TEMPERATURE_MILLI_K
});

// Spec: pressure within safe limits.
spec_fn!(pressure_in_range, (pa: u64) -> bool, {
    pa <= MAX_PRESSURE_PA
});

// Spec: dispense volume within syringe capacity.
spec_fn!(volume_in_range, (ul: u64) -> bool, {
    ul <= MAX_VOLUME_UL
});

// ── Verified executive functions ─────────────────────────────────

/// Command the robotic arm to extend to `mm` millimetres.
/// Verified precondition: `arm_in_range(mm)` must hold.
pub fn move_arm_verified(mm: u64) -> Result<u64, &'static str> {
    requires!(arm_in_range(mm));
    ensures!(|result: &Result<u64, &'static str>| result.is_ok());
    Ok(mm)
}

/// Set reactor temperature to `mk` milli-kelvins.
pub fn set_temperature_verified(mk: u64) -> Result<u64, &'static str> {
    requires!(temp_in_range(mk));
    ensures!(|result: &Result<u64, &'static str>| result.is_ok());
    Ok(mk)
}

/// Set chamber pressure to `pa` pascals.
pub fn set_pressure_verified(pa: u64) -> Result<u64, &'static str> {
    requires!(pressure_in_range(pa));
    ensures!(|result: &Result<u64, &'static str>| result.is_ok());
    Ok(pa)
}

/// Dispense `ul` microlitres from the syringe pump.
pub fn dispense_verified(ul: u64) -> Result<u64, &'static str> {
    requires!(volume_in_range(ul));
    ensures!(|result: &Result<u64, &'static str>| result.is_ok());
    Ok(ul)
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
}
