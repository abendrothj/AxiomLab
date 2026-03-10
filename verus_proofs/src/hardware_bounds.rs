//! Specification stubs for hardware-bound invariants.
//!
//! These will carry `#[verus::spec]` and `#[verus::proof]` attributes
//! once the Verus toolchain is integrated.

/// Maximum extension of the robotic arm in millimetres.
pub const MAX_ARM_EXTENSION_MM: u64 = 1200;

/// Assert (at proof time) that a requested extension is within safe limits.
///
/// Under standard `rustc` this is a plain runtime check;
/// under Verus it will be an SMT-verified precondition.
pub fn check_arm_extension(mm: u64) -> bool {
    mm <= MAX_ARM_EXTENSION_MM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_bounds() {
        assert!(check_arm_extension(500));
    }

    #[test]
    fn at_limit() {
        assert!(check_arm_extension(MAX_ARM_EXTENSION_MM));
    }

    #[test]
    fn over_limit() {
        assert!(!check_arm_extension(MAX_ARM_EXTENSION_MM + 1));
    }
}
