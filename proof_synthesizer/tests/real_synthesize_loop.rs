//! End-to-end crucible tests for the ORA proof-synthesis loop.
//!
//! Each test follows the same 3-step assertion chain:
//! 1) Initial Verus compile fails on the unsafe snippet.
//! 2) synthesize_proof() returns Ok(repaired_source).
//! 3) Verus accepts repaired_source.

use proof_synthesizer::agent::{synthesize_proof, SynthConfig};
use proof_synthesizer::compiler::{find_verus, invoke_verus};
use tracing_subscriber::EnvFilter;

/// Intentionally minimal unsafe snippet with one clear precondition issue:
/// `extend_arm` calls `move_arm` but does not require `arm_safe(mm)`.
const MICRO_UNSAFE_SNIPPET: &str = r#"
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

pub fn extend_arm(mm: u64) -> (result: u64)
    ensures result == mm,
{
    move_arm(mm)
}

fn main() {
    let _ = extend_arm(500);
}

} // verus!
"#;

fn verus_available() -> bool {
    find_verus().ok().is_some_and(|p| {
        match std::process::Command::new(&p).arg("--version").output() {
            Ok(out) => {
                let txt = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                out.status.success()
                    && !txt.contains("x86-linux only")
                    && !txt.contains("not available")
            }
            Err(_) => false,
        }
    })
}

fn llm_endpoint_configured() -> bool {
    std::env::var("AXIOMLAB_LLM_ENDPOINT")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

#[tokio::test]
#[ignore = "requires live LLM endpoint + Verus binary; run inside Docker"]
async fn real_synthesize_proof_fixes_micro_precondition_violation() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("proof_synthesizer=debug".parse().unwrap()))
        .with_test_writer()
        .try_init();

    if !verus_available() {
        eprintln!("SKIP: Verus not available (set VERUS_PATH or install verus)");
        return;
    }
    if !llm_endpoint_configured() {
        eprintln!("SKIP: AXIOMLAB_LLM_ENDPOINT not set for live synthesis test");
        return;
    }

    // 1) Initial compile should fail.
    let tmp_before = tempfile::tempdir().expect("create tempdir");
    let initial = invoke_verus(MICRO_UNSAFE_SNIPPET, tmp_before.path())
        .await
        .expect("invoke verus on unsafe snippet");
    assert!(
        !initial.success,
        "Initial unsafe snippet must fail Verus.\nOutput:\n{}",
        initial.output
    );

    // 2) Run ORA synthesis loop.
    let config = SynthConfig {
        // Small local models (e.g., phi3) often need a few extra ORA turns
        // to converge on Verus-specific precondition edits.
        max_retries: 8,
        temperature: 0.0,
        ..Default::default()
    };
    let repaired = synthesize_proof(MICRO_UNSAFE_SNIPPET, &config)
        .await
        .expect("synthesize_proof should return corrected source");

    // 3) Re-compile repaired source; it should verify.
    let tmp_after = tempfile::tempdir().expect("create tempdir");
    let verified = invoke_verus(&repaired, tmp_after.path())
        .await
        .expect("invoke verus on repaired snippet");

    assert!(
        verified.success,
        "Synthesized source should pass Verus.\n=== Synthesized Source ===\n{}\n=== Verus Output ===\n{}",
        repaired,
        verified.output
    );
}

// ── Loop invariant crucible ────────────────────────────────────────────────

/// A counter loop that already has a `decreases` clause but is missing its
/// loop invariant.  Without `invariant i <= n`, Verus cannot establish that
/// `i == n` after the loop exits, so the postcondition fails.
///
/// Correct fix: add `invariant i <= n,` to the while header.
const LOOP_INVARIANT_SNIPPET: &str = r#"
use vstd::prelude::*;

verus! {

/// Count from 0 to n by incrementing i; result must equal n.
pub fn count_up(n: u64) -> (result: u64)
    requires n <= 1000,
    ensures result == n,
{
    let mut i: u64 = 0;
    while i < n
        decreases n - i,
    {
        i = i + 1;
    }
    i
}

fn main() { let _ = count_up(5); }

} // verus!
"#;

#[tokio::test]
#[ignore = "requires live LLM endpoint + Verus binary; run inside Docker"]
async fn real_synthesize_proof_adds_loop_invariant() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("proof_synthesizer=debug".parse().unwrap()),
        )
        .with_test_writer()
        .try_init();

    if !verus_available() {
        eprintln!("SKIP: Verus not available (set VERUS_PATH or install verus)");
        return;
    }
    if !llm_endpoint_configured() {
        eprintln!("SKIP: AXIOMLAB_LLM_ENDPOINT not set for live synthesis test");
        return;
    }

    // 1) Initial compile must fail — invariant is missing.
    let tmp_before = tempfile::tempdir().expect("create tempdir");
    let initial = invoke_verus(LOOP_INVARIANT_SNIPPET, tmp_before.path())
        .await
        .expect("invoke_verus on loop snippet");
    assert!(
        !initial.success,
        "Loop snippet without invariant must fail Verus.\nOutput:\n{}",
        initial.output
    );

    // 2) ORA synthesis loop should add the invariant.
    let config = SynthConfig {
        max_retries: 10,
        temperature: 0.0,
        ..Default::default()
    };
    let repaired = synthesize_proof(LOOP_INVARIANT_SNIPPET, &config)
        .await
        .expect("synthesize_proof should return source with loop invariant");

    // Sanity-check: repaired source must mention `invariant` at all.
    assert!(
        repaired.contains("invariant"),
        "Repaired source should contain a loop invariant.\n=== Repaired Source ===\n{repaired}"
    );

    // 3) Verus must accept the repaired source.
    let tmp_after = tempfile::tempdir().expect("create tempdir");
    let verified = invoke_verus(&repaired, tmp_after.path())
        .await
        .expect("invoke_verus on repaired loop snippet");
    assert!(
        verified.success,
        "Repaired loop source should pass Verus.\n=== Repaired Source ===\n{}\n=== Verus Output ===\n{}",
        repaired,
        verified.output
    );
}
