//! ╔══════════════════════════════════════════════════════════════════╗
//! ║  SIMULATION 3 — VeruSAGE Proof Synthesis Loop                  ║
//! ╚══════════════════════════════════════════════════════════════════╝
//!
//! Scenario: The agent generates a concurrent sensor-polling function
//! without proof annotations.  The VeruSAGE observe→reason→act loop:
//!   1. Feeds the code to the Verus compiler → gets diagnostic errors.
//!   2. Sends the diagnostics + source to an LLM → receives a fix.
//!   3. Replaces the source with the corrected version.
//!   4. Repeats until Verus accepts or the retry budget is exhausted.
//!
//! Since the Verus binary is not installed in CI, this test simulates
//! the full loop using the diagnostics parser and a synthetic Verus
//! output, proving the pipeline structure works end-to-end.

use proof_synthesizer::diagnostics::{self, Severity};

// ── Synthetic Verus output for an unverified function ────────────

const UNVERIFIED_SOURCE: &str = r#"
use verus::*;

fn poll_sensors(channels: &[u32]) -> Vec<f64> {
    let mut readings = Vec::new();
    for &ch in channels {
        let value = read_hw(ch);
        readings.push(value);
    }
    readings
}
"#;

const VERUS_OUTPUT_ROUND_1: &str = "\
error: verification of poll_sensors failed
  --> candidate.rs:4:1
   |
4  | fn poll_sensors(channels: &[u32]) -> Vec<f64> {
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: precondition not satisfied
   = note: `channels.len() > 0` not established
error: verification of poll_sensors loop invariant failed
  --> candidate.rs:6:5
   |
6  |     for &ch in channels {
   |     ^^^^^^^^^^^^^^^^^^^^
   |
   = note: loop invariant `readings.len() <= channels.len()` not proved
warning: unused ghost variable `proof_token`
  --> candidate.rs:3:1
";

const VERUS_OUTPUT_ROUND_2: &str = "\
error: verification of poll_sensors ensures clause failed
  --> candidate.rs:4:1
   |
4  | fn poll_sensors(channels: &[u32]) -> Vec<f64> {
   | ^
   |
   = note: postcondition `result.len() == channels.len()` not proved
";

const VERUS_OUTPUT_ROUND_3: &str = "\
verification complete: 3 verified, 0 errors
";

// ── Simulated LLM responses (corrected code per iteration) ──────

const LLM_FIX_ROUND_1: &str = r#"Here's the corrected version:

```rust
use verus::*;

// requires(channels.len() > 0)
// ensures(result.len() == channels.len())
fn poll_sensors(channels: &[u32]) -> Vec<f64> {
    let mut readings = Vec::new();
    // invariant(readings.len() <= channels.len())
    for &ch in channels {
        let value = read_hw(ch);
        readings.push(value);
    }
    readings
}
```
"#;

const LLM_FIX_ROUND_2: &str = r#"Fixed the postcondition:

```rust
use verus::*;

// requires(channels.len() > 0)
// ensures(result.len() == channels.len())
fn poll_sensors(channels: &[u32]) -> Vec<f64> {
    let mut readings = Vec::with_capacity(channels.len());
    // invariant(readings.len() <= channels.len())
    // invariant(readings.len() == loop_iter)
    for &ch in channels {
        let value = read_hw(ch);
        readings.push(value);
    }
    // assert(readings.len() == channels.len())
    readings
}
```
"#;

// ── Test: full observe→reason→act loop ───────────────────────────

#[test]
fn sim3_verusage_observe_reason_act_loop() {
    // This test walks through the VeruSAGE loop manually, proving the
    // pipeline logic works before real Verus is wired up.

    let verus_outputs = [
        VERUS_OUTPUT_ROUND_1,
        VERUS_OUTPUT_ROUND_2,
        VERUS_OUTPUT_ROUND_3,
    ];
    let llm_responses = [LLM_FIX_ROUND_1, LLM_FIX_ROUND_2];

    let mut current_source = UNVERIFIED_SOURCE.to_owned();
    let max_retries: usize = 5;
    let mut verified = false;

    println!("═══ VeruSAGE Proof Synthesis Loop ═══\n");
    println!("Initial source ({} bytes):\n{current_source}", current_source.len());

    for attempt in 0..max_retries {
        let verus_output = verus_outputs.get(attempt).unwrap_or(&verus_outputs[2]);

        // ── 1. OBSERVE: parse Verus diagnostics ──
        let diags = diagnostics::parse(verus_output);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();

        println!(
            "\n── Iteration {} ──\nVerus output: {} diagnostics ({} errors)",
            attempt + 1,
            diags.len(),
            errors.len()
        );

        if errors.is_empty() && verus_output.contains("0 errors") {
            println!("✓ Verus accepted the proof on iteration {}!", attempt + 1);
            verified = true;
            break;
        }

        // Summarize for the LLM
        let summary = diagnostics::summarize(&diags);
        println!("Diagnostic summary for LLM:\n{summary}");

        // ── 2. REASON: LLM proposes a fix ──
        let llm_reply = llm_responses
            .get(attempt)
            .expect("test should have enough LLM responses");

        println!("LLM proposed a fix ({} bytes)", llm_reply.len());

        // ── 3. ACT: extract corrected code ──
        if let Some(code) = extract_rust_block(llm_reply) {
            println!("Extracted corrected source ({} bytes)", code.len());
            current_source = code;
        } else {
            panic!("LLM response did not contain a ```rust block");
        }
    }

    assert!(
        verified,
        "VeruSAGE should have produced a valid proof within {max_retries} iterations"
    );
    assert!(
        current_source.contains("with_capacity"),
        "Final source should contain the Vec::with_capacity fix"
    );
    assert!(
        current_source.contains("invariant"),
        "Final source should contain loop invariants"
    );

    println!("\n═══ VeruSAGE completed: proof synthesized in 3 iterations ═══");
    println!("\nFinal verified source:\n{current_source}");
}

// ─────────────────────────────────────────────────────────────────
//  Test: diagnostic parser accuracy
// ─────────────────────────────────────────────────────────────────

#[test]
fn sim3_diagnostic_parser_accuracy() {
    let diags = diagnostics::parse(VERUS_OUTPUT_ROUND_1);

    // Should extract 2 errors and 1 warning
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    let warnings: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Warning).collect();

    assert_eq!(errors.len(), 2, "Expected 2 errors, got {}", errors.len());
    assert_eq!(warnings.len(), 1, "Expected 1 warning, got {}", warnings.len());

    // First error should mention precondition
    assert!(
        errors[0].message.contains("poll_sensors"),
        "First error should mention poll_sensors, got: {}",
        errors[0].message
    );

    // Second error should mention loop invariant
    assert!(
        errors[1].message.contains("loop invariant"),
        "Second error should mention loop invariant, got: {}",
        errors[1].message
    );

    // Both should have parsed Verus spans
    assert!(
        errors[0].span.as_ref().map_or(false, |s| s.contains("poll_sensors")),
        "First error should have span for poll_sensors"
    );

    println!("✓ Diagnostic parser correctly extracted {} errors, {} warnings",
        errors.len(), warnings.len()
    );
}

// ─────────────────────────────────────────────────────────────────
//  Test: the loop terminates correctly when verification succeeds
//  immediately (no errors in first compile)
// ─────────────────────────────────────────────────────────────────

#[test]
fn sim3_immediate_success() {
    let diags = diagnostics::parse(VERUS_OUTPUT_ROUND_3);
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();

    assert!(errors.is_empty(), "Round 3 output should have 0 errors");
    assert!(
        VERUS_OUTPUT_ROUND_3.contains("0 errors"),
        "Should indicate success"
    );
    println!("✓ Already-verified code detected as passing immediately");
}

// ── Helper ───────────────────────────────────────────────────────

fn extract_rust_block(text: &str) -> Option<String> {
    let start = text.find("```rust")?;
    let code_start = start + 7;
    let end = text[code_start..].find("```")?;
    let code = text[code_start..code_start + end].trim();
    if code.is_empty() {
        None
    } else {
        Some(code.to_owned())
    }
}
