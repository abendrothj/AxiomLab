//! ╔══════════════════════════════════════════════════════════════════╗
//! ║  REAL VERUS INTEGRATION — Proof Synthesizer End-to-End          ║
//! ╚══════════════════════════════════════════════════════════════════╝
//!
//! This test invokes the ACTUAL Verus compiler through the
//! proof_synthesizer crate, proving the full pipeline works:
//!   1. invoke_verus() with safe code → verification succeeds
//!   2. invoke_verus() with unsafe code → verification fails
//!   3. diagnostics::parse() extracts real Verus errors
//!
//! Run inside Docker: cargo test --package proof_synthesizer --test real_verus_integration
//! Requires: VERUS_PATH environment variable pointing to the verus binary.

use proof_synthesizer::compiler::{find_verus, invoke_verus};
use proof_synthesizer::diagnostics::{self, Severity};

/// Safe lab control code that Verus should accept.
const SAFE_CODE: &str = r#"
use vstd::prelude::*;

verus! {

pub const MAX_ARM_MM: u64 = 1200;

pub open spec fn arm_safe(mm: u64) -> bool {
    mm <= MAX_ARM_MM
}

pub fn move_arm(mm: u64) -> (result: u64)
    requires arm_safe(mm),
    ensures result == mm,
{
    mm
}

pub fn safe_move_arm(mm: u64) -> (result: Result<u64, u64>)
    ensures
        result.is_ok() <==> arm_safe(mm),
{
    if mm <= MAX_ARM_MM {
        Ok(move_arm(mm))
    } else {
        Err(mm)
    }
}

fn main() {
    let ok = safe_move_arm(600);
    assert(ok.is_ok());
    let bad = safe_move_arm(9999);
    assert(bad.is_err());
}

} // verus!
"#;

/// Deliberately unsafe code that should FAIL verification.
const UNSAFE_CODE: &str = r#"
use vstd::prelude::*;

verus! {

pub const MAX_ARM_MM: u64 = 1200;

pub open spec fn arm_safe(mm: u64) -> bool {
    mm <= MAX_ARM_MM
}

pub fn move_arm(mm: u64) -> (result: u64)
    requires arm_safe(mm),
    ensures result == mm,
{
    mm
}

fn main() {
    let _ = move_arm(5000); // UNSAFE: violates precondition
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

#[tokio::test]
#[ignore = "requires Verus binary (VERUS_PATH or verus on PATH); run inside Docker"]
async fn real_verus_safe_code_accepted() {
    if !verus_available() {
        eprintln!("SKIP: Verus not available (set VERUS_PATH or install verus)");
        return;
    }
    let tmp = tempfile::tempdir().expect("create tempdir");
    let result = invoke_verus(SAFE_CODE, tmp.path()).await.expect("invoke verus");

    println!("=== Verus output (safe code) ===\n{}", result.output);
    assert!(
        result.success,
        "Verus should accept safe lab code.\nOutput:\n{}",
        result.output
    );
    assert!(
        result.output.contains("verified") && result.output.contains("0 errors"),
        "Output should confirm verification.\nOutput:\n{}",
        result.output
    );
}

#[tokio::test]
#[ignore = "requires Verus binary (VERUS_PATH or verus on PATH); run inside Docker"]
async fn real_verus_unsafe_code_rejected() {
    if !verus_available() {
        eprintln!("SKIP: Verus not available (set VERUS_PATH or install verus)");
        return;
    }
    let tmp = tempfile::tempdir().expect("create tempdir");
    let result = invoke_verus(UNSAFE_CODE, tmp.path()).await.expect("invoke verus");

    println!("=== Verus output (unsafe code) ===\n{}", result.output);
    assert!(
        !result.success,
        "Verus should reject unsafe lab code.\nOutput:\n{}",
        result.output
    );

    // Parse the diagnostics
    let diags = diagnostics::parse(&result.output);
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(
        !errors.is_empty(),
        "Should have extracted error diagnostics.\nOutput:\n{}",
        result.output
    );

    // Should mention precondition failure
    let has_precondition_error = errors.iter().any(|d| d.message.contains("precondition"));
    assert!(
        has_precondition_error,
        "Error should mention precondition violation.\nDiagnostics: {:?}",
        errors
    );

    let summary = diagnostics::summarize(&diags);
    println!("=== Diagnostic summary ===\n{summary}");
    assert!(summary.contains("precondition"));
}

#[tokio::test]
#[ignore = "requires Verus binary (VERUS_PATH or verus on PATH); run inside Docker"]
async fn real_verus_diagnostics_pipeline() {
    if !verus_available() {
        eprintln!("SKIP: Verus not available (set VERUS_PATH or install verus)");
        return;
    }
    let tmp = tempfile::tempdir().expect("create tempdir");
    let result = invoke_verus(UNSAFE_CODE, tmp.path()).await.expect("invoke verus");

    // Full pipeline: compile → parse → summarize
    let diags = diagnostics::parse(&result.output);
    let summary = diagnostics::summarize(&diags);

    // Summary should be suitable for feeding back to an LLM
    assert!(!summary.is_empty());
    assert!(summary != "No errors.");
    println!("=== LLM-ready diagnostic summary ===\n{summary}");

    // Verify we can extract line numbers from real Verus output
    let with_lines: Vec<_> = diags.iter().filter(|d| d.line.is_some()).collect();
    println!(
        "Extracted {} diagnostics with line numbers from real Verus output",
        with_lines.len()
    );
}
