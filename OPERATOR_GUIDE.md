# AxiomLab Operator Guide

Operational reference for running, testing, and understanding the safety architecture. This document describes what the system **actually does** — not aspirations.

## 1) System Architecture

AxiomLab is a Rust workspace (9 crates) + a Python SiLA 2 server + a React web dashboard. The core loop: an LLM proposes lab experiments → a 5-stage validation pipeline checks every proposed action → validated actions execute over SiLA 2 gRPC → results feed back to the LLM.

### 1.1 Crate Roles

**Production path (server + agent_runtime + proof_artifacts + vessel_physics):**

- **server** — Axum HTTP server with WebSocket event streaming, in-memory event buffer, and the continuous exploration loop that drives the agent.
  - [server/src/main.rs](server/src/main.rs) — HTTP endpoints (`/ws`, `/api/status`, `/api/history`), auto-starts exploration loop on launch
  - [server/src/simulator.rs](server/src/simulator.rs) — Connects SiLA 2 clients, loads proof manifest, creates orchestrator, runs LLM loop
  - [server/src/ws_sink.rs](server/src/ws_sink.rs) — WebSocket broadcast sink + in-memory EventBuffer (up to 2000 events per type, reset on restart)

- **agent_runtime** — The orchestrator, protocol executor, and all safety layers.
  - [agent_runtime/src/orchestrator.rs](agent_runtime/src/orchestrator.rs) — 5-stage validation pipeline (`try_tool_call`), LLM chat loop (`run_experiment`), protocol execution (`run_protocol`)
  - [agent_runtime/src/protocol.rs](agent_runtime/src/protocol.rs) — `Protocol`, `ProtocolPlan`, `ProtocolStep`, `ProtocolRunResult` types; JSON schema exposed to LLM
  - [agent_runtime/src/protocol_executor.rs](agent_runtime/src/protocol_executor.rs) — `ProtocolExecutor`: iterates protocol steps through the full 5-stage pipeline, feeds observations to LLM for adaptation, requests and signs conclusion
  - [agent_runtime/src/hardware.rs](agent_runtime/src/hardware.rs) — SiLA 2 gRPC client pool: 6 instruments, 12 methods
  - [agent_runtime/src/sandbox.rs](agent_runtime/src/sandbox.rs) — Path/command allowlist enforcement
  - [agent_runtime/src/capabilities.rs](agent_runtime/src/capabilities.rs) — Numeric parameter bounds (volume, position, temperature, etc.)
  - [agent_runtime/src/approvals.rs](agent_runtime/src/approvals.rs) — Two-person Ed25519 approval records
  - [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs) — Hash-chained JSONL audit log with per-event Ed25519 signatures
  - [agent_runtime/src/rekor.rs](agent_runtime/src/rekor.rs) — Sigstore Rekor transparency-log anchoring for protocol conclusions
  - [agent_runtime/src/tools.rs](agent_runtime/src/tools.rs) — ToolCall/ToolResult types, ToolRegistry for dispatch
  - [agent_runtime/src/llm.rs](agent_runtime/src/llm.rs) — LLM client (OpenAI-compatible API)
  - [agent_runtime/src/events.rs](agent_runtime/src/events.rs) — Event types and EventSink trait

- **proof_artifacts** — Proof manifest schema and runtime policy engine.
  - [proof_artifacts/src/manifest.rs](proof_artifacts/src/manifest.rs) — ProofManifest, ProofArtifact, VerusArtifact, RiskClass
  - [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs) — RuntimePolicyEngine: maps tool actions → risk classes → required artifacts
  - [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs) — Ed25519 manifest signing/verification
  - [proof_artifacts/src/ci.rs](proof_artifacts/src/ci.rs) — CI gate enforcement (sorry-free, build identity)

- **vessel_physics** — Formally verified vessel physics with PyO3 Python bindings.
  - [vessel_physics/src/lib.rs](vessel_physics/src/lib.rs) — `VesselRegistry` (u64 nanoliter volumes), `proved_add`/`proved_sub` arithmetic, PyO3 `#[pyclass]`
  - Proofs: [verus_verified/vessel_registry.rs](verus_verified/vessel_registry.rs) — 11 theorems verified by Verus, 0 errors
  - Protocol proofs: [verus_verified/protocol_safety.rs](verus_verified/protocol_safety.rs) — 13 theorems: step count ≤ 20, total volume ≤ 200 mL, dilution series safe
  - Manifest: [proof_artifacts/vessel_physics_manifest.json](proof_artifacts/vessel_physics_manifest.json) — real Verus compiler output, committed
  - Build: `maturin develop --manifest-path vessel_physics/Cargo.toml`

**Scientific compute (standalone, no I/O):**

- **scientific_compute** — Pure-Rust numerics: `nalgebra` linear algebra, `rustfft` FFT, OLS regression, Hill equation fitting, lab data parsing
- **physical_types** — Compile-time dimensional analysis via `uom`

**Formal verification tooling (requires external toolchains):**

- **verus_proofs** — Verus-compatible specs with a macro shim for dual `rustc`/Verus compilation.
- **proof_synthesizer** — LLM-driven iterative Verus proof repair (observe → reason → act). Requires Verus binary + LLM.

### 1.2 Runtime Authorization Path

When the LLM returns a tool call, `Orchestrator::try_tool_call()` executes these stages in order:

1. **Sandbox** — Is the tool name in the allowlist? If not, reject immediately.
2. **Approval** — Does this action's risk class require two-person approval? If so, check for valid Ed25519 approval records. Check revocation list.
3. **Capability** — Are all numeric parameters within hardware bounds? (e.g., dispense volume ∈ [0.5, 1000] µL, arm x ∈ [0, 300] mm)
4. **Fail-closed** — Is this a high-risk action (Actuation, Destructive) without a proof policy engine configured? If so, deny.
5. **Proof policy** — Does the RuntimePolicyEngine authorize this action based on Verus artifact status in `vessel_physics_manifest.json`? ReadOnly actions pass without artifacts. LiquidHandling and Actuation require `ArtifactStatus::Passed`.
6. **Dispatch** — Log Ed25519-signed audit event, call `tools.dispatch()` which sends the gRPC request to SiLA 2 hardware. For liquid operations, the Python SiLA 2 server calls into the `vessel_physics` Rust crate via PyO3, which uses `proved_add`/`proved_sub`.

Every stage emits audit events. A rejection at any stage stops the pipeline — later stages never run.

When the LLM calls `propose_protocol`, the orchestrator intercepts it before dispatch and runs `run_protocol()`, which iterates each step through the full 5-stage pipeline above, then signs and Rekor-anchors the conclusion.

### 1.3 Proof Chain Detail

```text
vessel_physics_manifest.json
  ← generated by: python3 vessel_physics/generate_manifest.py
  ← which runs:   ~/verus/verus verus_verified/vessel_registry.rs
  ← sets status:  "Passed" iff Verus exits 0 (11 theorems, 0 errors)
  ← also runs:    ~/verus/verus verus_verified/protocol_safety.rs
  ← sets status:  "Passed" iff Verus exits 0 (13 theorems, 0 errors)

At runtime:
  RuntimePolicyEngine.load(manifest)
  → checks ArtifactStatus::Passed for LiquidHandling actions
  → if Passed: dispatch over SiLA 2 gRPC
      → Python server calls vessel_physics.VesselRegistry.dispense()
          → PyO3 → Rust VesselRegistry::dispense()
              → runtime overflow guard (matches Verus precondition)
              → proved_add(volume_nl, delta_nl)  ← Z3-verified

For protocol runs:
  ProtocolExecutor runs each step through the pipeline above
  → ProtocolStepRecord written (Ed25519-signed, hash-chained)
  → After all steps: LLM generates conclusion
  → ProtocolConclusionRecord written (Ed25519-signed, hash-chained)
  → Rekor anchor submitted (hash + sig → UUID + integrated_time)
```

## 2) Security Findings and Risk Priorities

Honest assessment of security posture. Ordered by severity.

### 2.1 High: Policy construction trust boundary

- Code: [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs)
- Issue: The `RuntimePolicyEngine` can be constructed with `trusted` flag without prior signature verification. In production, `simulator.rs` calls `mark_signature_verified()` only when the manifest hash matches a known constant — but this is a compile-time constant, not a runtime cryptographic check.
- Impact: A modified binary could bypass signature verification intent.
- Mitigation: Use `proofctl verify` with actual Ed25519 keys before deployment. Restrict trusted constructors to test paths.

### 2.2 Medium: Audit log signing key is ephemeral

- Code: [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs)
- Issue: Each event is individually Ed25519-signed and hash-chained. Protocol conclusions are anchored to Sigstore Rekor for external timestamp witnessing. However, the Ed25519 signing key is generated fresh on each run — a complete chain rewrite with a fresh key would pass local checks.
- Impact: An attacker with disk access and code-execution could forge a new chain.
- Mitigation: Persist the signing key across runs (HSM or sealed storage). Use Rekor anchoring (already implemented) as the external witness. Add periodic signed checkpoints.

### 2.3 Medium: Hardware is entirely simulated

- Code: [sila_mock/](sila_mock/) and [agent_runtime/src/hardware.rs](agent_runtime/src/hardware.rs)
- Issue: The SiLA 2 server returns simulated values. The validation pipeline is tested against this server, not real instruments.
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

### 3.1 Local Development

```bash
# Build all crates
cargo build

# Build PyO3 vessel_physics module (required for Python SiLA 2 server)
pip install maturin
VIRTUAL_ENV=$VIRTUAL_ENV PATH="$VIRTUAL_ENV/bin:$PATH" \
    maturin develop --manifest-path vessel_physics/Cargo.toml

# Run pure-Rust tests (no external dependencies)
cargo test -p agent_runtime -- capability sandbox proof_policy

# Start SiLA 2 server for integration tests
cd sila_mock && python3 -m axiomlab_mock --insecure -p 50052 &

# Run integration tests
cargo test -p agent_runtime --test vessel_simulation_e2e -- --ignored --test-threads=1
cargo test -p agent_runtime --test sila2_e2e -- --ignored --test-threads=1
cargo test -p agent_runtime --test orchestrator_sila2 -- --ignored --test-threads=1
```

### 3.2 Verus Proof Workflow

```bash
# Install Verus (native on macOS ARM64, Linux x86-64, Linux ARM64)
# Download: https://github.com/verus-lang/verus/releases
# Extract to ~/verus/, run: chmod +x ~/verus/verus
# Also install Rust toolchain: rustup toolchain install 1.94.0-aarch64-apple-darwin

# Verify vessel physics proofs
~/verus/verus verus_verified/vessel_registry.rs
# Expected: verification results:: 11 verified, 0 errors

# Verify hardware safety bounds
~/verus/verus verus_verified/lab_safety.rs
# Expected: verification results:: 6 verified, 0 errors

# Verify protocol safety proofs
~/verus/verus verus_verified/protocol_safety.rs
# Expected: verification results:: 13 verified, 0 errors

# Regenerate proof manifest from real Verus run
python3 vessel_physics/generate_manifest.py
# Writes: proof_artifacts/vessel_physics_manifest.json

# Check manifest status without writing
python3 vessel_physics/generate_manifest.py --status-only
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

### 3.6 Verify Rekor Anchor

After a protocol run, the Rekor UUID is logged at INFO level. To verify independently:

```bash
# Via rekor-cli
rekor-cli verify --uuid <uuid> --artifact-hash <sha256_hex>

# Via REST API
curl https://rekor.sigstore.dev/api/v1/log/entries/<uuid>
```

### 3.7 Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `SILA2_ENDPOINT` | `http://localhost:50052` | SiLA 2 gRPC server address |
| `AXIOMLAB_LLM_ENDPOINT` | — | Ollama or OpenAI-compatible API endpoint |
| `AXIOMLAB_LLM_MODEL` | `qwen2.5-coder:7b` | LLM model name |
| `PORT` | `3000` | HTTP server port |
| `AXIOMLAB_AUDIT_LOG` | — | Path for audit log output |
| `AXIOMLAB_GIT_COMMIT` | `dev` | Git commit SHA embedded in proof manifest build identity |

### 3.8 Platform Notes

- **macOS ARM64 (Apple Silicon):** Full Verus support via native ARM64 binary (`~/verus/verus`). Requires Rust toolchain `1.94.0-aarch64-apple-darwin`. All integration tests work.
- **Linux x86-64:** Full Verus support via native x86-64 binary. All features work.
- **Linux ARM64:** Full Verus support via native ARM64 binary. All features work.

## 4) Operator Checklist

Before running high-risk actions:

1. Verify signed manifest (`proofctl verify`).
2. Verify CI gate pass for required artifacts (sorry_count == 0, ArtifactStatus::Passed).
3. Verify runtime build identity inputs (git commit, binary hash, optional device/firmware fields).
4. Verify approval bundle for Actuation or Destructive actions.
5. Verify audit chain integrity after execution.
6. Confirm Rekor anchor UUID was logged for any protocol conclusion.

## 5) Suggested Next Hardening Tasks

1. Persist the Ed25519 audit signing key across restarts (HSM or sealed storage) so chain continuity can be cryptographically proved.
2. Restrict trusted policy-engine constructor usage to test-only contexts.
3. Replace hardware simulation with injected production driver traits.
4. Extend integration tests to enforce signed-manifest-only authorization path.
5. Add multi-agent coordination layer for parallel instrument utilization.
