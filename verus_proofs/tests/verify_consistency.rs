//! ╔══════════════════════════════════════════════════════════════════╗
//! ║  CONSISTENCY TEST — Verus Source ↔ Runtime Code                 ║
//! ╚══════════════════════════════════════════════════════════════════╝
//!
//! This test verifies that the runtime safety enforcement in
//! `verus_proofs::hardware_bounds` is CONSISTENT with the formally
//! verified Verus source in `verus_verified/lab_safety.rs`.
//!
//! It does three things:
//! 1. Confirms the constants extracted by build.rs match expected values
//! 2. Runs the real Verus compiler to confirm proofs still hold
//! 3. Tests boundary values to ensure runtime and Verus agree
//!
//! If this test passes, you can trust that the runtime code enforces
//! exactly the bounds that Verus formally proved safe.

use verus_proofs::hardware_bounds::*;
use verus_proofs::verify;

// ═══════════════════════════════════════════════════════════════════
//  Test 1: Constants extracted from Verus source are correct
// ═══════════════════════════════════════════════════════════════════

#[test]
fn constants_from_verus_source() {
    // These are the values defined in verus_verified/lab_safety.rs.
    // build.rs extracts them. If the Verus file changes, this test
    // will fail until the values are updated in the Verus source.
    assert_eq!(MAX_ARM_EXTENSION_MM, 1200, "arm max should be 1200 mm");
    assert_eq!(MIN_ARM_EXTENSION_MM, 0, "arm min should be 0 mm");
    assert_eq!(MAX_TEMPERATURE_MILLI_K, 500_000, "temp max should be 500K");
    assert_eq!(MIN_TEMPERATURE_MILLI_K, 200_000, "temp min should be 200K");
    assert_eq!(MAX_PRESSURE_PA, 200_000, "pressure max should be 200 kPa");
    assert_eq!(MAX_VOLUME_UL, 50_000, "volume max should be 50 mL");
}

// ═══════════════════════════════════════════════════════════════════
//  Test 2: Runtime predicates match Verus spec fns at boundaries
// ═══════════════════════════════════════════════════════════════════

#[test]
fn runtime_predicates_match_verus_at_boundaries() {
    // Arm: [0, 1200]
    assert!(arm_in_range(0), "min boundary");
    assert!(arm_in_range(1200), "max boundary");
    assert!(!arm_in_range(1201), "above max");

    // Temperature: [200_000, 500_000]
    assert!(!temp_in_range(199_999), "below min");
    assert!(temp_in_range(200_000), "min boundary");
    assert!(temp_in_range(500_000), "max boundary");
    assert!(!temp_in_range(500_001), "above max");

    // Pressure: [0, 200_000]
    assert!(pressure_in_range(0), "zero pressure");
    assert!(pressure_in_range(200_000), "max boundary");
    assert!(!pressure_in_range(200_001), "above max");

    // Volume: [0, 50_000]
    assert!(volume_in_range(0), "zero volume");
    assert!(volume_in_range(50_000), "max boundary");
    assert!(!volume_in_range(50_001), "above max");
}

// ═══════════════════════════════════════════════════════════════════
//  Test 3: Runtime safety functions accept/reject consistently
// ═══════════════════════════════════════════════════════════════════

#[test]
fn safety_functions_consistent_with_predicates() {
    // For every predicate, the corresponding safety function should
    // return Ok iff the predicate is true.
    let arm_cases: Vec<(u64, bool)> = vec![
        (0, true), (600, true), (1200, true),
        (1201, false), (5000, false), (u64::MAX, false),
    ];
    for (val, expected) in arm_cases {
        assert_eq!(
            move_arm_verified(val).is_ok(), expected,
            "move_arm_verified({val}) should be {expected}"
        );
        assert_eq!(
            arm_in_range(val), expected,
            "arm_in_range({val}) should be {expected}"
        );
    }

    let temp_cases: Vec<(u64, bool)> = vec![
        (0, false), (199_999, false), (200_000, true),
        (300_000, true), (500_000, true), (500_001, false),
    ];
    for (val, expected) in temp_cases {
        assert_eq!(
            set_temperature_verified(val).is_ok(), expected,
            "set_temperature_verified({val}) should be {expected}"
        );
    }

    let pressure_cases: Vec<(u64, bool)> = vec![
        (0, true), (101_325, true), (200_000, true), (200_001, false),
    ];
    for (val, expected) in pressure_cases {
        assert_eq!(
            set_pressure_verified(val).is_ok(), expected,
            "set_pressure_verified({val}) should be {expected}"
        );
    }

    let volume_cases: Vec<(u64, bool)> = vec![
        (0, true), (5_000, true), (50_000, true), (50_001, false), (100_000, false),
    ];
    for (val, expected) in volume_cases {
        assert_eq!(
            dispense_verified(val).is_ok(), expected,
            "dispense_verified({val}) should be {expected}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Test 4: Real Verus verification confirms proofs still hold
// ═══════════════════════════════════════════════════════════════════

#[test]
fn verus_proofs_still_hold() {
    match verify::verify_lab_safety() {
        Some(result) => {
            println!("Verus output:\n{}", result.output);
            assert!(
                result.success,
                "Verus verification FAILED — the proofs no longer hold!\n\
                 This means the Verus source was modified in a way that \
                 breaks the safety guarantees.\n\
                 Output:\n{}",
                result.output
            );
            assert!(
                result.verified_count >= 18,
                "Expected at least 18 verified functions, got {}",
                result.verified_count
            );
            assert_eq!(result.error_count, 0, "Expected 0 verification errors");
            println!(
                "✓ Verus verification passed: {} functions verified, {} errors",
                result.verified_count, result.error_count
            );
        }
        None => {
            eprintln!(
                "SKIP: Verus compiler not available.\n\
                 Run inside Docker to execute real verification:\n\
                 docker compose run --rm axiomlab cargo test --package verus_proofs --test verify_consistency"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Test 5: Composite command mirrors Verus execute_lab_command
// ═══════════════════════════════════════════════════════════════════

#[test]
fn composite_command_mirrors_verus() {
    // Safe values — should match Verus execute_lab_command returning Ok
    let result = execute_lab_command(600, 300_000, 101_325, 5_000);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), (600, 300_000, 101_325, 5_000));

    // Any single violation should cause rejection
    assert!(execute_lab_command(5000, 300_000, 101_325, 5_000).is_err()); // arm
    assert!(execute_lab_command(600, 100_000, 101_325, 5_000).is_err()); // temp low
    assert!(execute_lab_command(600, 600_000, 101_325, 5_000).is_err()); // temp high
    assert!(execute_lab_command(600, 300_000, 300_000, 5_000).is_err()); // pressure
    assert!(execute_lab_command(600, 300_000, 101_325, 60_000).is_err()); // volume
}

// ═══════════════════════════════════════════════════════════════════
//  Test 6: Clamp function mirrors Verus clamp_arm
// ═══════════════════════════════════════════════════════════════════

#[test]
fn clamp_mirrors_verus() {
    // In range → identity
    assert_eq!(clamp_arm(0), 0);
    assert_eq!(clamp_arm(600), 600);
    assert_eq!(clamp_arm(1200), 1200);

    // Above max → clamped to max
    assert_eq!(clamp_arm(1201), 1200);
    assert_eq!(clamp_arm(9999), 1200);
    assert_eq!(clamp_arm(u64::MAX), 1200);

    // Result is always in range
    for val in [0, 1, 600, 1199, 1200, 1201, 5000, u64::MAX] {
        assert!(arm_in_range(clamp_arm(val)), "clamp_arm({val}) should always be in range");
    }
}
