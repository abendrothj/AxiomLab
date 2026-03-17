# AxiomLab Operator Guide

Operational reference for running, testing, and understanding the safety architecture. This document describes what the system **actually does** — not aspirations.

## 1) System Architecture

AxiomLab is a Rust workspace (8 crates) + a Python SiLA 2 mock server + a React web dashboard. The core loop: an LLM proposes lab experiments → a 5-stage validation pipeline checks every proposed action → validated actions execute over SiLA 2 gRPC → results feed back to the LLM.

### 1.1 Crate Roles

**Production path (server + agent_runtime + proof_artifacts):**

- **server** — Axum HTTP server with WebSocket event streaming, SQLite event log, and the continuous exploration loop that drives the agent.
  - [server/src/main.rs](server/src/main.rs) — HTTP endpoints (`/ws`, `/api/status`, `/api/history`), auto-starts exploration loop on launch
  - [server/src/simulator.rs](server/src/simulator.rs) — Connects SiLA 2 clients, builds proof manifest, creates orchestrator, runs LLM loop
  - [server/src/db.rs](server/src/db.rs) — Append-only SQLite with WAL mode
  - [server/src/ws_sink.rs](server/src/ws_sink.rs) — WebSocket broadcast sink

- **agent_runtime** — The orchestrator and all safety layers.
  - [agent_runtime/src/orchestrator.rs](agent_runtime/src/orchestrator.rs) — 5-stage validation pipeline (`try_tool_call`), LLM chat loop (`run_experiment`)
  - [agent_runtime/src/hardware.rs](agent_runtime/src/hardware.rs) — SiLA 2 gRPC client pool: 6 instruments, 12 methods
  - [agent_runtime/src/sandbox.rs](agent_runtime/src/sandbox.rs) — Path/command allowlist enforcement
  - [agent_runtime/src/capabilities.rs](agent_runtime/src/capabilities.rs) — Numeric parameter bounds (volume, position, temperature, etc.)
  - [agent_runtime/src/approvals.rs](agent_runtime/src/approvals.rs) — Two-person Ed25519 approval records
  - [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs) — Hash-chained JSONL audit log
  - [agent_runtime/src/tools.rs](agent_runtime/src/tools.rs) — ToolCall/ToolResult types, ToolRegistry for dispatch
  - [agent_runtime/src/llm.rs](agent_runtime/src/llm.rs) — LLM client (OpenAI-compatible API)
  - [agent_runtime/src/events.rs](agent_runtime/src/events.rs) — Event types and EventSink trait

- **proof_artifacts** — Proof manifest schema and runtime policy engine.
  - [proof_artifacts/src/manifest.rs](proof_artifacts/src/manifest.rs) — ProofManifest, ProofArtifact, VerusArtifact, RiskClass
  - [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs) — RuntimePolicyEngine: maps tool actions → risk classes → required artifacts
  - [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs) — Ed25519 manifest signing/verification
  - [proof_artifacts/src/ci.rs](proof_artifacts/src/ci.rs) — CI gate enforcement (sorry-free, build identity)

**Scientific compute (standalone, no I/O):**

- **scientific_compute** — Pure-Rust numerics: `nalgebra` linear algebra, `rustfft` FFT, OLS regression, lab data parsing
- **physical_types** — Compile-time dimensional analysis via `uom`

**Formal verification tooling (requires external toolchains):**

- **verus_proofs** — Verus-compatible specs with a macro shim for dual `rustc`/Verus compilation. Verified source in `verus_verified/lab_safety.rs`.
- **proof_synthesizer** — LLM-driven iterative Verus proof repair (observe → reason → act). Requires Verus binary + LLM.
- **aeneas_lean_semantics** — MIR export → Aeneas translation → Lean 4 type-checking. Requires Aeneas + Lean.

### 1.2 Runtime Authorization Path (what actually runs)

When the LLM returns a tool call, `Orchestrator::try_tool_call()` executes these stages in order:

1. **Sandbox** — Is the tool name in the allowlist? If not, reject immediately.
2. **Approval** — Does this action's risk class require two-person approval? If so, check for valid Ed25519 approval records. Check revocation list.
3. **Capability** — Are all numeric parameters within hardware bounds? (e.g., dispense volume ∈ [0.5, 1000] µL, arm x ∈ [0, 300] mm)
4. **Fail-closed** — Is this a high-risk action (Actuation, Destructive) without a proof policy engine configured? If so, deny.
5. **Proof policy** — Does the RuntimePolicyEngine authorize this action based on Verus artifact status? ReadOnly actions pass without artifacts. Actuation requires passed, signed, sorry-free proofs.
6. **Dispatch** — Log audit event, call `tools.dispatch()` which sends the gRPC request to SiLA 2 hardware.

Every stage emits audit events. A rejection at any stage stops the pipeline — later stages never run.

## 2) Security Findings and Risk Priorities

Honest assessment of security posture. Ordered by severity.

### 2.1 High: Policy construction trust boundary

- Code: [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs)
- Issue: The `RuntimePolicyEngine` can be constructed with `trusted` flag without prior signature verification. In production, `simulator.rs` calls `mark_signature_verified()` only when the manifest hash matches a known constant — but this is a compile-time constant, not a runtime cryptographic check.
- Impact: A modified binary could bypass signature verification intent.
- Mitigation: Use `proofctl verify` with actual Ed25519 keys before deployment. Restrict trusted constructors to test paths.

### 2.2 High: Audit log is tamper-evident but not independently anchored

- Code: [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs)
- Issue: SHA256 hash chaining detects insertion/deletion after the fact, but a complete chain rewrite would pass local checks. No per-event signatures. No external anchor (blockchain, remote witness, etc.).
- Impact: An attacker with disk access could rewrite the entire chain.
- Mitigation: Add periodic signed checkpoints. Mirror audit events to external immutable storage.

### 2.3 Medium: Hardware is entirely simulated

- Code: [sila_mock/](sila_mock/) and [agent_runtime/src/hardware.rs](agent_runtime/src/hardware.rs)
- Issue: The SiLA 2 mock returns plausible fake data (e.g., dispense returns requested_volume ± noise). The validation pipeline is tested against this mock, not real instruments.
- Impact: We know the software pipeline works correctly. We don't know if real hardware would behave within the expected response formats.
- Mitigation: When connecting to real SiLA 2 instruments, add hardware-in-the-loop tests behind a feature flag.

### 2.4 Medium: Lean `sorry` placeholders in non-critical files

- File: [lean4/AxiomLabVerified.lean](lean4/AxiomLabVerified.lean)
- Issue: Some Lean theorem files use `sorry` (unproven placeholder). These are not in the release-critical path.
- Impact: If these files were accidentally included in proof policy requirements, confidence would be overstated.
- Mitigation: CI gate enforces zero-sorry policy on required artifacts. Keep scope precise.

### 2.5 Low: Key lifecycle is external

- Code: [agent_runtime/src/approvals.rs](agent_runtime/src/approvals.rs), [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs)
- Issue: Ed25519 signing and verification are implemented. Key generation, rotation, revocation, and storage are left to the operator.
- Impact: Key compromise undermines the entire approval and signing chain.
- Mitigation: Use HSM/KMS for production. Define rotation and break-glass procedures.

## 3) Runbook

### 3.1 Docker Compose (recommended)

```bash
# Start everything (Ollama + SiLA 2 mock + AxiomLab server)
docker compose up --build

# Web dashboard
open http://localhost:3000

# View logs
docker compose logs -f axiomlab
```

### 3.2 Local Development

```bash
# Build all crates
cargo build

# Run pure-Rust tests (no external dependencies)
cargo test -p agent_runtime -- capability sandbox proof_policy

# Start SiLA 2 mock for integration tests
cd sila_mock && python -m axiomlab_mock --insecure &

# Run all integration tests (19 tests)
cargo test -p agent_runtime --test sila2_e2e --test orchestrator_sila2
```

### 3.3 Release Gate

```bash
./scripts/proof_release_gate.sh
```

Runs 10 steps: build, manifest generation, signing, policy enforcement, sandbox/capability tests, audit chain verification, compliance bundle export.

### 3.4 Verify Audit Chain

```bash
cargo run -p agent_runtime --bin auditctl -- verify --path .artifacts/proof/runtime_audit.jsonl
```

### 3.5 Verify Signed Manifest

```bash
cargo run -p proof_artifacts --bin proofctl -- verify \
  --signed-manifest .artifacts/proof/manifest.signed.json \
  --public-key .artifacts/proof/manifest_signing_key.public.b64
```

### 3.6 Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `SILA2_ENDPOINT` | `http://localhost:50052` | SiLA 2 gRPC server address |
| `AXIOMLAB_LLM_ENDPOINT` | — | Ollama or OpenAI-compatible API endpoint |
| `AXIOMLAB_LLM_MODEL` | `qwen2.5-coder:7b` | LLM model name |
| `PORT` | `3000` | HTTP server port |
| `VERUS_VERIFIED` | — | Set to `1` if Verus verification passed at build time |
| `AXIOMLAB_DOCKER` | — | Set to `1` inside Docker containers |
| `AXIOMLAB_AUDIT_LOG` | — | Path for audit log output |

### 3.7 Architecture Notes

- **x86-linux (amd64):** Full Verus verification available. All features work.
- **ARM (aarch64):** Verus is unavailable. A graceful stub is installed. Lean, Aeneas, agent reasoning, and all SiLA 2 integration work normally. Verus-dependent tests detect the stub and skip.
- **macOS:** Local development works. SiLA 2 mock and integration tests work. Docker Compose works. Verus requires x86-linux (or Docker on an amd64 host).

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
