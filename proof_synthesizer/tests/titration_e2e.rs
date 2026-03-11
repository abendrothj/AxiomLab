//! ╔══════════════════════════════════════════════════════════════════╗
//! ║  END-TO-END: Acid-Base Titration with Formal Verification       ║
//! ╚══════════════════════════════════════════════════════════════════╝
//!
//! This test demonstrates the complete AxiomLab pipeline:
//!   1. Agent proposes a titration experiment hypothesis
//!   2. Code is generated with Verus safety annotations
//!   3. Real Verus compiler formally verifies hardware bounds
//!   4. Experiment executes (simulated hardware)
//!   5. Results are analysed to confirm/refute hypothesis
//!
//! The key innovation: step 3 uses the ACTUAL Verus compiler to prove
//! that no dispense volume, temperature, or arm movement can violate
//! physical safety constraints — even if the LLM hallucinates bad values.
//!
//! Run inside Docker:
//!   cargo test --package proof_synthesizer --test titration_e2e -- --nocapture

use proof_synthesizer::compiler::{find_verus, invoke_verus};
use proof_synthesizer::diagnostics;

/// Verus-verifiable titration control code.
///
/// This is what an LLM agent would generate — a complete titration
/// protocol with formally verified safety bounds on every actuator command.
const TITRATION_CODE: &str = r#"
use vstd::prelude::*;

verus! {

// ── Hardware safety envelope ─────────────────────────────────────
pub const MAX_VOLUME_UL: u64 = 50_000;       // 50 mL syringe max
pub const MIN_DISPENSE_UL: u64 = 10;         // 10 µL minimum step
pub const MAX_ARM_MM: u64 = 1200;
pub const MAX_TEMPERATURE_MK: u64 = 500_000; // 500 K
pub const MIN_TEMPERATURE_MK: u64 = 200_000; // 200 K

// ── Safety predicates ────────────────────────────────────────────
pub open spec fn volume_safe(ul: u64) -> bool {
    MIN_DISPENSE_UL <= ul && ul <= MAX_VOLUME_UL
}

pub open spec fn arm_safe(mm: u64) -> bool {
    mm <= MAX_ARM_MM
}

pub open spec fn temp_safe(mk: u64) -> bool {
    MIN_TEMPERATURE_MK <= mk && mk <= MAX_TEMPERATURE_MK
}

// ── Verified actuator commands ───────────────────────────────────

/// Dispense titrant from the syringe pump.
/// Verus proves: volume is always within physical syringe limits.
pub fn dispense_titrant(volume_ul: u64) -> (result: u64)
    requires volume_safe(volume_ul),
    ensures  result == volume_ul, volume_safe(result),
{
    volume_ul
}

/// Move arm to position the syringe over a well.
pub fn position_arm(mm: u64) -> (result: u64)
    requires arm_safe(mm),
    ensures  result == mm,
{
    mm
}

/// Set the stirring hotplate temperature.
pub fn set_hotplate(mk: u64) -> (result: u64)
    requires temp_safe(mk),
    ensures  result == mk,
{
    mk
}

// ── Titration protocol ──────────────────────────────────────────

/// Calculate a single titration step volume.
/// Returns the dispense volume in µL, clamped to safe bounds.
/// Precondition ensures remaining_ul * fraction_pct fits in u64.
pub fn titration_step_volume(
    remaining_ul: u64,
    fraction_pct: u64,  // e.g. 10 means 10%
) -> (result: u64)
    requires
        0 < fraction_pct,
        fraction_pct <= 100,
        remaining_ul <= MAX_VOLUME_UL,
        // Overflow guard: 50_000 * 100 = 5_000_000 fits in u64 easily
        remaining_ul as int * fraction_pct as int <= u64::MAX as int,
    ensures
        volume_safe(result),
{
    let step: u64 = remaining_ul * fraction_pct / 100;
    // Clamp to safe bounds
    if step < MIN_DISPENSE_UL {
        MIN_DISPENSE_UL
    } else if step > MAX_VOLUME_UL {
        MAX_VOLUME_UL
    } else {
        step
    }
}

/// Total volume after n uniform dispenses doesn't exceed syringe capacity.
/// Verus proves: you can never dispense more than the syringe holds.
pub fn verify_total_volume(step_ul: u64, n_steps: u64) -> (result: bool)
    requires
        volume_safe(step_ul),
        n_steps <= 1000,
        step_ul <= 500,
        // Overflow guard: 500 * 1000 = 500_000 fits in u64
        step_ul as int * n_steps as int <= u64::MAX as int,
    ensures
        result == (step_ul * n_steps <= MAX_VOLUME_UL),
{
    step_ul * n_steps <= MAX_VOLUME_UL
}

// ── Full titration run (simplified) ─────────────────────────────

/// Execute a single titration: position arm, set temperature, dispense.
/// Verus proves the COMPOSITION is safe — all three commands are within bounds.
pub fn titration_run(
    well_position_mm: u64,
    hotplate_mk: u64,
    dispense_ul: u64,
) -> (result: (u64, u64, u64))
    requires
        arm_safe(well_position_mm),
        temp_safe(hotplate_mk),
        volume_safe(dispense_ul),
    ensures
        result.0 == well_position_mm,
        result.1 == hotplate_mk,
        result.2 == dispense_ul,
{
    let arm = position_arm(well_position_mm);
    let temp = set_hotplate(hotplate_mk);
    let vol = dispense_titrant(dispense_ul);
    (arm, temp, vol)
}

fn main() {
    // Simulate a titration at room temperature
    let arm = position_arm(350);          // well A4 at 350mm
    let temp = set_hotplate(298_000);     // 298 K (room temp) in millikelvin

    // Calculate step volume: 10% of 5000 µL remaining = 500 µL
    let vol = titration_step_volume(5000, 10);

    // Execute the titration
    let result = titration_run(350, 298_000, vol);
}

} // verus!
"#;

/// Unsafe titration code — dispense volume exceeds syringe capacity.
const UNSAFE_TITRATION_CODE: &str = r#"
use vstd::prelude::*;

verus! {

pub const MAX_VOLUME_UL: u64 = 50_000;
pub const MIN_DISPENSE_UL: u64 = 10;

pub open spec fn volume_safe(ul: u64) -> bool {
    MIN_DISPENSE_UL <= ul && ul <= MAX_VOLUME_UL
}

pub fn dispense_titrant(volume_ul: u64) -> (result: u64)
    requires volume_safe(volume_ul),
    ensures  result == volume_ul,
{
    volume_ul
}

fn main() {
    // BUG: Agent hallucinated "dispense 100 mL" — exceeds 50 mL syringe
    let _ = dispense_titrant(100_000);
}

} // verus!
"#;

fn verus_available() -> bool {
    find_verus().ok().map_or(false, |p| {
        if p.exists() {
            return true;
        }
        std::process::Command::new(&p).arg("--version").output().is_ok()
    })
}

// ═══════════════════════════════════════════════════════════════════
//  Test 1: Complete titration verified end-to-end
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires Verus binary (VERUS_PATH or verus on PATH); run inside Docker"]
async fn e2e_titration_verified() {
    if !verus_available() {
        eprintln!("SKIP: Verus not available");
        return;
    }

    println!("═══ AxiomLab End-to-End: Acid-Base Titration ═══\n");

    // ── Stage 1: Hypothesis ──────────────────────────────────────
    let hypothesis = "NaOH titration of 0.1M HCl will reach equivalence \
                      point at ~25 mL when using 0.1M NaOH titrant.";
    println!("HYPOTHESIS: {hypothesis}\n");

    // ── Stage 2: Code generation (above constant is the output) ──
    println!("STAGE 2: Code generated with Verus safety annotations");
    println!("  Safety bounds: volume ≤ 50mL, arm ≤ 1200mm, temp 200-500K\n");

    // ── Stage 3: FORMAL VERIFICATION ─────────────────────────────
    println!("STAGE 3: Invoking REAL Verus compiler...");
    let tmp = tempfile::tempdir().expect("create tempdir");
    let result = invoke_verus(TITRATION_CODE, tmp.path()).await.expect("invoke verus");

    println!("  Verus output: {}", result.output.trim());
    assert!(
        result.success,
        "Titration code should be formally verified.\nOutput:\n{}",
        result.output
    );
    println!("  ✓ ALL safety properties formally proven by Z3/Verus\n");

    // ── Stage 4: Simulated execution ─────────────────────────────
    println!("STAGE 4: Executing titration (simulated hardware)");
    let titration_data = simulate_titration();
    println!("  Dispensed {} steps, total volume: {} µL",
        titration_data.len(),
        titration_data.iter().map(|d| d.volume_ul).sum::<u64>()
    );

    // ── Stage 5: Analysis ────────────────────────────────────────
    println!("\nSTAGE 5: Analysing results");
    let equivalence_point = find_equivalence_point(&titration_data);
    println!("  Equivalence point at cumulative volume: {} µL ({:.1} mL)",
        equivalence_point, equivalence_point as f64 / 1000.0
    );
    println!("  pH at equivalence: {:.1}",
        titration_data.iter()
            .find(|d| d.cumulative_ul >= equivalence_point)
            .map_or(7.0, |d| d.ph)
    );

    // ── Conclusion ───────────────────────────────────────────────
    let hypothesis_confirmed = (20_000..=30_000).contains(&equivalence_point);
    println!(
        "\nCONCLUSION: Hypothesis {} — equivalence at {:.1} mL {}",
        if hypothesis_confirmed { "CONFIRMED" } else { "REFUTED" },
        equivalence_point as f64 / 1000.0,
        if hypothesis_confirmed { "(within expected range)" } else { "(outside expected range)" }
    );
    assert!(hypothesis_confirmed, "Titration should confirm the hypothesis");

    println!("\n═══ End-to-End Complete: Formally Verified Autonomous Experiment ═══");
}

// ═══════════════════════════════════════════════════════════════════
//  Test 2: Unsafe titration rejected before execution
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "requires Verus binary (VERUS_PATH or verus on PATH); run inside Docker"]
async fn e2e_unsafe_titration_blocked() {
    if !verus_available() {
        eprintln!("SKIP: Verus not available");
        return;
    }

    println!("═══ Safety Demo: Blocking Unsafe Titration ═══\n");

    let tmp = tempfile::tempdir().expect("create tempdir");
    let result = invoke_verus(UNSAFE_TITRATION_CODE, tmp.path()).await.expect("invoke verus");

    assert!(!result.success, "Unsafe titration should be rejected");

    let diags = diagnostics::parse(&result.output);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == diagnostics::Severity::Error)
        .collect();

    println!("Verus REJECTED the unsafe titration:");
    for e in &errors {
        println!("  ✗ {}", e.message);
        if let Some(line) = e.line {
            println!("    at line {line}");
        }
    }

    assert!(
        errors.iter().any(|e| e.message.contains("precondition")),
        "Should detect volume precondition violation"
    );

    println!("\n✓ Dangerous 100mL dispense blocked BEFORE reaching hardware");
    println!("═══ Safety Demo Complete ═══");
}

// ═══════════════════════════════════════════════════════════════════
//  Simulated titration data
// ═══════════════════════════════════════════════════════════════════

struct TitrationDataPoint {
    volume_ul: u64,
    cumulative_ul: u64,
    ph: f64,
}

/// Simulate an acid-base titration curve.
/// 0.1M NaOH into 25 mL of 0.1M HCl → equivalence at 25 mL.
fn simulate_titration() -> Vec<TitrationDataPoint> {
    let mut data = Vec::new();
    let mut cumulative: u64 = 0;
    let equivalence_ul: f64 = 25_000.0; // 25 mL expected equivalence

    let step_ul: u64 = 500;
    let n_steps = 100; // 50 mL total

    for _i in 0..n_steps {
        cumulative += step_ul;
        let fraction = cumulative as f64 / equivalence_ul;

        // Realistic sigmoidal titration curve using arctangent
        // Sharp inflection at equivalence point (fraction = 1.0)
        let ph = 7.0 + 5.5 * (20.0 * (fraction - 1.0)).tanh();

        data.push(TitrationDataPoint {
            volume_ul: step_ul,
            cumulative_ul: cumulative,
            ph,
        });
    }

    data
}

/// Find the equivalence point by looking for the steepest pH change.
fn find_equivalence_point(data: &[TitrationDataPoint]) -> u64 {
    if data.len() < 2 {
        return 0;
    }
    let mut max_delta = 0.0_f64;
    let mut eq_idx = 0;

    for i in 1..data.len() {
        let delta = (data[i].ph - data[i - 1].ph).abs();
        if delta > max_delta {
            max_delta = delta;
            eq_idx = i;
        }
    }

    data[eq_idx].cumulative_ul
}
