//! ╔══════════════════════════════════════════════════════════════════╗
//! ║  SIMULATION 1 — Hardware Violation: LLM Hallucination Blocked  ║
//! ╚══════════════════════════════════════════════════════════════════╝
//!
//! Scenario: An AI agent attempts to command a robotic arm to extend
//! to 500 mm, but the verified specification restricts the arm to a
//! strict maximum of 100 mm (simulating a confined glovebox).
//!
//! The safety check fires at the specification boundary, proving
//! that even if the LLM hallucinates a physically impossible command,
//! the runtime (backed by Verus-verified bounds) blocks it before
//! any hardware moves.

// ── Glovebox-specific specification (100 mm max) ─────────────────

const GLOVEBOX_MAX_MM: u64 = 100;

fn glovebox_arm_in_range(mm: u64) -> bool {
    mm <= GLOVEBOX_MAX_MM
}

fn glovebox_move_arm(mm: u64) -> Result<u64, &'static str> {
    if !glovebox_arm_in_range(mm) {
        return Err("arm position out of glovebox range");
    }
    Ok(mm)
}

// ── Standard lab arm (full 1200 mm range — bounds from Verus source) ──

use verus_proofs::hardware_bounds::{
    move_arm_verified,
    set_temperature_verified,
    set_pressure_verified,
    dispense_verified,
};

// ─────────────────────────────────────────────────────────────────
//  Test: agent hallucinates 500 mm in a 100 mm glovebox → BLOCKED
// ─────────────────────────────────────────────────────────────────

#[test]
fn sim1_hallucinated_500mm_in_glovebox_is_blocked() {
    let agent_requested_mm = 500; // LLM hallucination

    let result = glovebox_move_arm(agent_requested_mm);

    assert!(
        result.is_err(),
        "CRITICAL: the verified runtime MUST reject 500 mm in a 100 mm glovebox"
    );
    println!(
        "✓ Glovebox spec rejected agent command: {} mm (max {})",
        agent_requested_mm, GLOVEBOX_MAX_MM
    );
}

#[test]
fn sim1_valid_80mm_in_glovebox_is_accepted() {
    let result = glovebox_move_arm(80);
    assert_eq!(result, Ok(80));
    println!("✓ Glovebox spec accepted 80 mm (within 100 mm limit)");
}

#[test]
fn sim1_exactly_at_glovebox_limit() {
    assert_eq!(glovebox_move_arm(GLOVEBOX_MAX_MM), Ok(GLOVEBOX_MAX_MM));
    assert!(glovebox_move_arm(GLOVEBOX_MAX_MM + 1).is_err());
    println!("✓ Boundary: {} mm OK, {} mm rejected", GLOVEBOX_MAX_MM, GLOVEBOX_MAX_MM + 1);
}

// ─────────────────────────────────────────────────────────────────
//  Test: cascade of hallucinated physical commands → ALL BLOCKED
// ─────────────────────────────────────────────────────────────────

#[test]
fn sim1_cascade_of_hallucinated_commands() {
    // Simulate an LLM generating a batch of dangerous lab commands:
    struct HallucinatedCommand {
        description: &'static str,
        result: Result<u64, &'static str>,
    }

    let commands = vec![
        HallucinatedCommand {
            description: "arm to 5000 mm (4x beyond max)",
            result: move_arm_verified(5000),
        },
        HallucinatedCommand {
            description: "temperature to 800K (beyond 500K limit)",
            result: set_temperature_verified(800_000),
        },
        HallucinatedCommand {
            description: "temperature to 50K (below 200K minimum)",
            result: set_temperature_verified(50_000),
        },
        HallucinatedCommand {
            description: "pressure to 500 kPa (beyond 200 kPa limit)",
            result: set_pressure_verified(500_000),
        },
        HallucinatedCommand {
            description: "dispense 100 mL (beyond 50 mL syringe capacity)",
            result: dispense_verified(100_000),
        },
    ];

    for cmd in &commands {
        assert!(
            cmd.result.is_err(),
            "SAFETY VIOLATION: '{}' should have been blocked!",
            cmd.description
        );
        println!("✓ Blocked hallucinated command: {}", cmd.description);
    }

    println!(
        "\n═══ All {} hallucinated commands were rejected by the verified runtime ═══",
        commands.len()
    );
}

// ─────────────────────────────────────────────────────────────────
//  Test: valid commands pass through the verified boundary
// ─────────────────────────────────────────────────────────────────

#[test]
fn sim1_valid_commands_accepted() {
    assert_eq!(move_arm_verified(600), Ok(600));
    assert_eq!(set_temperature_verified(300_000), Ok(300_000)); // 300 K
    assert_eq!(set_pressure_verified(101_325), Ok(101_325));    // 1 atm
    assert_eq!(dispense_verified(5_000), Ok(5_000));            // 5 mL
    println!("✓ All safe commands accepted by the verified runtime");
}
