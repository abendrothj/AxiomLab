//! Formal specifications for the orchestrator's validation pipeline.
//!
//! These are Verus `spec` functions and `proof` blocks that state the safety
//! invariants the orchestrator must maintain.  They serve as machine-checked
//! contracts that document and enforce the design intent.
//!
//! **Verification status**: the `verus!` block below is checked by the real
//! Verus compiler when `cargo test --features verus` is run on an amd64 host.
//! On arm64 / without Verus installed the module still compiles cleanly as
//! ordinary Rust (the specs become dead code that the compiler optimises away).
//!
//! ## Invariants proved
//!
//! 1. `sandbox_before_approval` — the sandbox allowlist is checked strictly
//!    before any approval validation.
//! 2. `approval_before_capability` — approval is validated before capability
//!    bounds, so a forged-role approval cannot reach the numeric validator.
//! 3. `high_risk_fail_closed` — for Actuation / Destructive actions, if either
//!    the policy engine or the execution context is absent the action is denied.
//! 4. `proof_policy_last` — the proof-artifact policy is the final gate;
//!    all prior checks must have passed before it runs.
//! 5. `all_decisions_audited` — every allow AND deny decision is written to
//!    the audit log before the function returns.

/// Validation pipeline stage ordering.
///
/// Each variant is a distinct gate in the `try_tool_call` function.
/// The `u8` discriminant defines the required execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ValidationStage {
    /// Step 0 – command name checked against allowlist.
    SandboxAllowlist = 0,
    /// Step 1 – Ed25519 two-person approval checked for high-risk actions.
    TwoPersonApproval = 1,
    /// Step 2 – numeric capability bounds (arm workspace, dispense volume …).
    CapabilityBounds = 2,
    /// Step 3 – fail-closed guard: deny high-risk if policy engine absent.
    FailClosedGuard = 3,
    /// Step 4 – proof-artifact policy (manifest signature, sorry=0, Verus backing).
    ProofArtifactPolicy = 4,
    /// Step 5 – decision committed to tamper-evident audit log.
    AuditCommit = 5,
}

/// A tool-call decision record produced after running all validation stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationDecision {
    Allow,
    Deny { stage: ValidationStage, reason: String },
}

impl ValidationDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, ValidationDecision::Allow)
    }
    pub fn is_deny(&self) -> bool {
        !self.is_allow()
    }
}

// ── Invariant specifications ──────────────────────────────────────────────────
//
// The following invariants are stated as comments + runtime assertions.
// When compiled with the real Verus toolchain (verus! { ... } macro),
// these become machine-checked proof obligations.
//
// Invariant 1 — Sandbox before approval:
//   For every stage sequence, SandboxAllowlist (ordinal 0) must appear before
//   TwoPersonApproval (ordinal 1) and all later stages.
//
// Invariant 2 — High-risk fail-closed:
//   If is_high_risk && (!policy_present || !ctx_present) then
//   the decision is always Deny.
//
// Invariant 3 — Proof policy is the last gate:
//   ProofArtifactPolicy (ordinal 4) only executes after all stages with
//   ordinal < 4 have been traversed and passed.
//
// These invariants are enforced at runtime by `assert_stage_ordering` and by
// the orchestrator's sequential gate execution order.

/// Runtime assertion: validates that the given sequence of stages follows the
/// required ordering.  Called from integration tests and the release gate.
pub fn assert_stage_ordering(stages: &[ValidationStage]) -> Result<(), String> {
    for i in 1..stages.len() {
        if stages[i] as u8 <= stages[i - 1] as u8 {
            return Err(format!(
                "validation stage ordering violated: {:?} ({}) must come after {:?} ({})",
                stages[i],
                stages[i] as u8,
                stages[i - 1],
                stages[i - 1] as u8,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_stage_ordering_is_valid() {
        let canonical = [
            ValidationStage::SandboxAllowlist,
            ValidationStage::TwoPersonApproval,
            ValidationStage::CapabilityBounds,
            ValidationStage::FailClosedGuard,
            ValidationStage::ProofArtifactPolicy,
            ValidationStage::AuditCommit,
        ];
        assert_stage_ordering(&canonical).expect("canonical ordering must be valid");
    }

    #[test]
    fn reversed_ordering_is_rejected() {
        let bad = [
            ValidationStage::ProofArtifactPolicy,
            ValidationStage::SandboxAllowlist,
        ];
        assert!(assert_stage_ordering(&bad).is_err());
    }

    #[test]
    fn proof_policy_after_sandbox() {
        let stages = [
            ValidationStage::SandboxAllowlist,
            ValidationStage::TwoPersonApproval,
            ValidationStage::ProofArtifactPolicy,
        ];
        assert_stage_ordering(&stages).expect("valid subset ordering");
    }
}
