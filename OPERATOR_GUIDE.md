# AxiomLab Operator Guide

This document is the consolidated operator reference for architecture, security controls, and day-2 run procedures.

## 1) System Architecture

AxiomLab is a multi-crate Rust workspace that combines:
- Scientific compute primitives
- Runtime policy and control-plane hardening
- Formal verification tooling (Verus, Aeneas, Lean)
- Proof artifact generation and policy gating

### 1.1 Crate Roles

- scientific_compute: Pure Rust numerics and data processing.
  - Source: [scientific_compute/src/lib.rs](scientific_compute/src/lib.rs)
  - Key modules:
    - [scientific_compute/src/linalg.rs](scientific_compute/src/linalg.rs)
    - [scientific_compute/src/fft.rs](scientific_compute/src/fft.rs)
    - [scientific_compute/src/discovery.rs](scientific_compute/src/discovery.rs)
    - [scientific_compute/src/lab_data.rs](scientific_compute/src/lab_data.rs)
- physical_types: Compile-time dimensional correctness via uom.
  - Source: [physical_types/src/quantities.rs](physical_types/src/quantities.rs)
- proof_artifacts: Manifest schema, signing, CI gate checks, runtime explain/authorize policy.
  - Sources:
    - [proof_artifacts/src/manifest.rs](proof_artifacts/src/manifest.rs)
    - [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs)
    - [proof_artifacts/src/ci.rs](proof_artifacts/src/ci.rs)
    - [proof_artifacts/src/generator.rs](proof_artifacts/src/generator.rs)
    - [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs)
- agent_runtime: Agent orchestration and hardening path for runtime tool calls.
  - Sources:
    - [agent_runtime/src/orchestrator.rs](agent_runtime/src/orchestrator.rs)
    - [agent_runtime/src/sandbox.rs](agent_runtime/src/sandbox.rs)
    - [agent_runtime/src/capabilities.rs](agent_runtime/src/capabilities.rs)
    - [agent_runtime/src/approvals.rs](agent_runtime/src/approvals.rs)
    - [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs)
    - [agent_runtime/src/tools.rs](agent_runtime/src/tools.rs)
- verus_proofs: Runtime mirrors of verified bounds and Verus invocation helpers.
  - Sources:
    - [verus_proofs/src/hardware_bounds.rs](verus_proofs/src/hardware_bounds.rs)
    - [verus_proofs/src/concurrency.rs](verus_proofs/src/concurrency.rs)
    - [verus_proofs/src/resource_allocator.rs](verus_proofs/src/resource_allocator.rs)
    - [verus_proofs/src/verify.rs](verus_proofs/src/verify.rs)
  - Verified source of truth:
    - [verus_verified/lab_safety.rs](verus_verified/lab_safety.rs)
    - [verus_verified/dilution_protocol.rs](verus_verified/dilution_protocol.rs)
- proof_synthesizer: Observe-reason-act loop for iterative Verus proof repair.
  - Sources:
    - [proof_synthesizer/src/agent.rs](proof_synthesizer/src/agent.rs)
    - [proof_synthesizer/src/compiler.rs](proof_synthesizer/src/compiler.rs)
    - [proof_synthesizer/src/diagnostics.rs](proof_synthesizer/src/diagnostics.rs)
- aeneas_lean_semantics: MIR export, Aeneas translation, Lean checks.
  - Sources:
    - [aeneas_lean_semantics/src/mir_export.rs](aeneas_lean_semantics/src/mir_export.rs)
    - [aeneas_lean_semantics/src/aeneas.rs](aeneas_lean_semantics/src/aeneas.rs)
    - [aeneas_lean_semantics/src/lean.rs](aeneas_lean_semantics/src/lean.rs)

### 1.2 Runtime Authorization Path

High-level flow for any tool action:
1. Sandbox command allowlist check.
2. Capability bounds check on numeric parameters.
3. Two-person approval check for high-risk risk classes.
4. Proof-policy authorization against manifest artifacts and build identity.
5. Audit write (hash-chained JSONL) and optional remote mirror.

Primary enforcement entry point:
- [agent_runtime/src/orchestrator.rs](agent_runtime/src/orchestrator.rs)

Audit implementation:
- [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs)

## 2) Security Findings and Risk Priorities

Ordered by severity.

### 2.1 High: Trusted policy construction can bypass signature-verification intent

- Relevant code: [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs)
- Issue: The trusted constructor path allows policy use without prior signature verification if used incorrectly.
- Operational risk: Policy checks can look valid while provenance is not cryptographically established.
- Recommended action:
  - Restrict trusted constructor to tests, or
  - Add explicit provenance state and enforce it at constructor boundary.

### 2.2 High: Audit log is tamper-evident but not independently signed per event

- Relevant code: [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs)
- Issue: Hash chaining detects local mutation but does not provide event-level signatures.
- Operational risk: A complete rewritten chain could pass local hash checks if attacker controls storage.
- Recommended action:
  - Add periodic signed checkpoints and
  - Anchor checkpoint hashes in external immutable storage.

### 2.3 Medium: Hardware integration still contains simulation stubs

- Relevant code:
  - [agent_runtime/src/tools.rs](agent_runtime/src/tools.rs)
  - [verus_proofs/src/concurrency.rs](verus_proofs/src/concurrency.rs)
- Issue: Some sensor/driver paths return fixed values in simulation mode.
- Operational risk: False confidence from passing tests without real hardware behavior.
- Recommended action:
  - Replace stubs with trait-driven driver adapters and
  - Add hardware-in-the-loop tests behind explicit feature flags.

### 2.4 Medium: Lean sorry placeholders exist in non-runtime Lean files

- Relevant file: [lean4/AxiomLabVerified.lean](lean4/AxiomLabVerified.lean)
- Issue: Placeholder proofs reduce assurance for artifacts that include sorry.
- Operational risk: If such files are accidentally admitted into release policy, proof confidence is overstated.
- Recommended action:
  - Keep CI gate strict for required artifacts with zero-sorry policy,
  - Scope required artifacts precisely in release specs.

### 2.5 Low: Cryptographic key lifecycle is external to code

- Relevant code:
  - [agent_runtime/src/approvals.rs](agent_runtime/src/approvals.rs)
  - [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs)
- Issue: Signing and verification are implemented, but key custody/rotation is operationally defined.
- Operational risk: Key compromise undermines approval and manifest trust.
- Recommended action:
  - Use HSM/KMS for production signing,
  - Define rotation, revocation, and break-glass procedures.

## 3) Runbook

## 3.1 Quick Local Validation

```bash
cargo build
cargo test
```

## 3.2 Docker Toolchain Build

```bash
docker compose build
```

## 3.3 Full Docker Test Sweep

```bash
docker compose run --rm axiomlab cargo test -- --include-ignored
```

## 3.4 Release Gate

```bash
./scripts/proof_release_gate.sh
```

Release-gate implementation:
- [scripts/proof_release_gate.sh](scripts/proof_release_gate.sh)

## 3.5 Verify Audit Chain

```bash
cargo run -p agent_runtime --bin auditctl -- verify --path .artifacts/proof/runtime_audit.jsonl
```

CLI source:
- [agent_runtime/src/bin/auditctl.rs](agent_runtime/src/bin/auditctl.rs)

## 3.6 Verify Signed Manifest

```bash
cargo run -p proof_artifacts --bin proofctl -- verify \
  --signed-manifest .artifacts/proof/manifest.signed.json \
  --public-key .artifacts/proof/manifest_signing_key.public.b64
```

CLI source:
- [proof_artifacts/src/bin/proofctl.rs](proof_artifacts/src/bin/proofctl.rs)

## 3.7 Verify Approval Bundle

```bash
cargo run -p agent_runtime --bin approvalctl -- verify \
  --bundle .artifacts/proof/replay_bundle/approval_bundle.json \
  --action move_arm \
  --risk-class Actuation \
  --git-commit "$(git rev-parse HEAD)" \
  --binary-hash "$(shasum -a 256 Cargo.lock | awk '{print $1}')" \
  --out .artifacts/proof/replay_bundle/approval_verification.json
```

CLI source:
- [agent_runtime/src/bin/approvalctl.rs](agent_runtime/src/bin/approvalctl.rs)

## 3.8 Architecture-Specific Notes

- amd64: full formal path is expected to run, including Verus.
- arm64: Verus can be unavailable by design in some setups; repository degrades gracefully for non-Verus paths.

Reference:
- [Dockerfile](Dockerfile)
- [verus_proofs/src/verify.rs](verus_proofs/src/verify.rs)

## 4) Operator Checklist

Before running high-risk actions:
1. Verify signed manifest.
2. Verify CI gate pass for required artifacts.
3. Verify runtime build identity inputs (git commit, binary hash, optional container/device/firmware fields).
4. Verify approval bundle for Actuation or Destructive actions.
5. Verify audit chain integrity after execution.

## 5) Suggested Next Hardening Tasks

1. Add signed audit checkpointing and remote hash anchoring.
2. Restrict trusted policy-engine constructor usage to test-only contexts.
3. Replace hardware stubs with injected production driver traits.
4. Extend integration tests to enforce signed-manifest-only authorization path.
