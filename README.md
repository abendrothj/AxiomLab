# AxiomLab

> A memory-safe Rust runtime for autonomous AI-driven lab exploration, with formal verification and SiLA 2 hardware integration.

## What This Is

AxiomLab is a working prototype of an autonomous science agent. An LLM (local Ollama) continuously proposes lab experiments. A Rust orchestrator validates every proposed action through a 5-stage safety pipeline before dispatching it to lab hardware over SiLA 2 gRPC. Hardware physics — volumes, capacities, overflow prevention — are enforced by formally verified Rust code proved correct by the Verus SMT solver. Multi-step experiments are expressed as structured protocols with cryptographically signed audit records anchored to the Sigstore Rekor transparency log.

**What works today (tested, integrated):**

- **5-stage tool validation pipeline** — sandbox allowlist → two-person approval → capability bounds → proof-artifact policy → audit + dispatch
- **SiLA 2 gRPC hardware layer** — 6 instruments (liquid handler, robotic arm, spectrophotometer, incubator, centrifuge, pH meter) with 12 operations
- **Formally verified vessel physics** — `vessel_physics` Rust crate stores volumes as u64 nanoliters; `proved_add`/`proved_sub` operations are proved correct by Verus (11 theorems, 0 errors). The `ProofManifest` is generated from real Verus compiler output, not a hardcoded fixture.
- **Formally verified protocol safety** — `verus_verified/protocol_safety.rs` proves step count bounds, total volume bounds, and dilution series safety (13 theorems, 0 errors).
- **Closed proof chain** — LLM intent → Rust proof gate (reads `vessel_physics_manifest.json`) → PyO3 boundary → `proved_add`/`proved_sub` (Z3-verified integer arithmetic) → SiLA 2 gRPC
- **Proof-policy gating** — Verus verification artifacts gate high-risk actions (actuation, liquid handling); read-only actions pass without proofs
- **Structured experiment protocols** — LLM proposes `ProtocolPlan` (name, hypothesis, ordered steps); a `ProtocolExecutor` iterates steps through the full 5-stage pipeline, feeds observations back to the LLM for adaptation, then requests a signed conclusion
- **Protocol replication** — `replicate_count` (1–10) on `ProtocolPlan` reruns all steps N times; `ReplicateAggregate` computes mean ± SD of steps-succeeded across replicates and is included in the signed conclusion
- **Per-event Ed25519 audit signatures** — every audit record (including protocol step records and conclusion records) is individually signed and hash-chained into an append-only JSONL log
- **Vessel state in audit chain** — dispense and aspirate embed a pre-operation vessel snapshot in the audit record (stripped before the LLM sees the output), providing a physical chain of custody for liquid volumes
- **Sigstore Rekor anchoring** — protocol conclusions are submitted to the public Rekor transparency log; the UUID and integrated timestamp provide an external, independently verifiable timestamp
- **Scientific compute in the loop** — `analyze_series` tool lets the LLM submit raw (x, y) data points and receive structured fit results: OLS slope/R², Hill EC50/E_max, Michaelis-Menten Vmax/Km, AIC-based model recommendation
- **Auto-findings from curve fits** — when `analyze_series` produces a linear R² ≥ 0.80 or a valid Hill fit, the runtime auto-records a `source: "system"` finding in the discovery journal with typed `Measurement` structs (value, unit, uncertainty) rather than prose strings, and emits a signed audit entry
- **Structured measurements in findings** — every `Finding` carries a `Vec<Measurement> { parameter, value, unit, uncertainty }` so numeric results are queryable, not just readable
- **Calibration log** — `calibrate_ph` records a `CalibrationRecord` in the discovery journal and emits a signed `calibration` audit event; calibration age and offset are injected into every LLM mandate
- **Parameter-space coverage tracking** — numeric tool inputs (e.g., absorbance wavelengths) are logged as `ParameterProbe` records (capped at 500); `coverage_summary_for_llm()` injects `[min, max] · N values` per parameter into the mandate
- **Unit metadata on tool schemas** — `ToolSpec.parameter_units` annotates every numeric parameter with its physical unit (e.g., `volume_ul → µL`, `x → mm`); injected into the LLM system prompt as `param [unit]` notes
- **Protocol template registry** — `server/src/simulator/protocol_library.rs` registers canonical protocol templates (beer-lambert-scan-v1, ph-titration-v1) that can be referenced by `template_id` in `ProtocolPlan`; template ID is recorded in the audit chain for reproducibility
- **Approval sidecar persistence** — pending approvals are written to `.artifacts/approvals/{id}.json` on enqueue and deleted on resolution; stale sidecars from a crashed run are detected and warned on startup
- **Audit query API** — `GET /api/audit?action=&decision=&since=&limit=` streams the JSONL audit log with server-side filtering; `GET /api/audit/verify` verifies the full hash chain without spawning a subprocess
- **Hypothesis lifecycle** — discovery journal tracks proposed → testing → confirmed / rejected; outer loop detects convergence (all hypotheses settled) and slows down, avoiding repeated experiments
- **Continuous autonomous loop** — LLM proposes → orchestrator validates → hardware executes → results feed back → LLM analyzes → journal records → LLM proposes next
- **Web visualizer** — real-time WebSocket dashboard with activity feed, state graph, and discovery journal

**What is simulated / not yet production:**

- Hardware is a **Python SiLA 2 server** returning simulated values — physics enforced by verified Rust, but no real physical instruments are connected
- LLM is **qwen2.5-coder:7b** via local Ollama — capable for structured tool calls, not a frontier reasoning model
- Two-person approval uses Ed25519 signatures but key management is manual / external

## Proof Chain

The complete authorization path for a liquid handling operation:

```text
LLM intent
  → Rust proof gate
      reads vessel_physics_manifest.json (real Verus compiler output)
      ArtifactStatus::Passed iff Verus exited 0
  → PyO3 boundary
      Python SiLA 2 server calls into Rust VesselRegistry
  → proved_add / proved_sub
      Z3-verified integer arithmetic on u64 nanoliters
      preconditions enforced before calling
  → SiLA 2 gRPC → instrument response
```

For structured protocol runs, an additional layer wraps the above:

```text
LLM emits ProtocolPlan JSON
  → parsed and validated (step count ≤ 20, tool names non-empty)
  → Verus proves: step count ≤ MAX_STEPS, total volume ≤ capacity
  → ProtocolExecutor iterates steps, each gated by the full 5-stage pipeline
  → ProtocolStepRecord (tool, params, result, proof_artifact_hash, chain hash, Ed25519 sig)
  → ProtocolConclusionRecord (LLM conclusion, Ed25519 signed)
  → Sigstore Rekor submission (UUID + integrated timestamp)
```

The Verus proofs live in `verus_verified/`. Run them with:

```bash
~/verus/verus verus_verified/vessel_registry.rs    # 11 verified, 0 errors
~/verus/verus verus_verified/lab_safety.rs          # 6 verified, 0 errors
~/verus/verus verus_verified/protocol_safety.rs     # 13 verified, 0 errors
```

## Crate Map

| Crate | Purpose | Status |
| --- | --- | --- |
| `server` | Axum HTTP + WebSocket server, in-memory event buffer, continuous exploration loop | Working |
| `agent_runtime` | Orchestrator (5-stage validation), protocol executor, SiLA 2 gRPC clients (6 instruments), sandbox, capabilities, approvals, audit, Rekor anchoring | Working, integration-tested |
| `proof_artifacts` | Manifest schema, RuntimePolicyEngine, RiskClass/ActionPolicy, Ed25519 signing, CI gate | Working, used in production pipeline |
| `vessel_physics` | Formally verified vessel physics — u64 nanoliter VesselRegistry with PyO3 Python bindings | Working; build with `maturin develop` |
| `scientific_compute` | Pure-Rust linear algebra (`nalgebra`), FFT (`rustfft`), OLS regression, Hill equation fitting, Michaelis-Menten, Welch t-test, AIC model selection — exposed to the LLM via `analyze_series` tool | Working |
| `physical_types` | Compile-time dimensional analysis via `uom` | Working |
| `verus_proofs` | Verus-compatible specs (dual `rustc`/Verus compilation shim), hardware-bound invariants | Working |
| `proof_synthesizer` | VeruSAGE-inspired observe→reason→act loop for iterative Verus proof repair | Compiles; requires Verus + LLM |

## Deployment Hardening: Five Validation Stages

Every tool call from the LLM passes through all five stages in `agent_runtime/src/orchestrator.rs` before reaching hardware.

| Stage | Component | What It Does |
| --- | --- | --- |
| **0. Sandbox** | Command allowlist | Blocks tools not in the allowlist |
| **1. Approval** | Two-person control | Ed25519 signatures required for high-risk actions |
| **2. Capability** | Numeric bounds | Rejects parameters outside hardware limits (e.g., volume > 1000 µL, x > 300 mm) |
| **3. Fail-Closed** | High-risk gate | If no proof policy engine is configured, all actuation/destructive actions are denied |
| **4. Proof Policy** | Artifact authorization | Checks Verus verification status from manifest; blocks actuation if proofs are missing/failed |
| **5. Dispatch** | Audit + execute | Logs Ed25519-signed hash-chained audit event, dispatches tool call over SiLA 2 gRPC |

## SiLA 2 Hardware Integration

AxiomLab talks to lab hardware using the [SiLA 2](https://sila-standard.com/) standard over gRPC. Six instruments are implemented:

| Instrument | Operations | Risk Class |
| --- | --- | --- |
| Liquid Handler | `dispense`, `aspirate` | LiquidHandling |
| Robotic Arm | `move_arm` | Actuation |
| Spectrophotometer | `read_absorbance` | ReadOnly |
| Incubator | `set_temperature`, `read_temperature`, `incubate` | Actuation / ReadOnly |
| Centrifuge | `spin_centrifuge`, `read_centrifuge_temperature` | Actuation / ReadOnly |
| pH Meter | `read_ph`, `calibrate_ph` | ReadOnly |

**Physics layer:** The Python SiLA 2 server delegates all volume arithmetic to the `vessel_physics` Rust crate via PyO3. Dispense and aspirate call `proved_add`/`proved_sub` — operations proved correct by Verus. The Beer-Lambert absorbance model reads vessel state from the same Rust registry.

**To use real hardware:** Replace the Python SiLA 2 server with actual SiLA 2-compliant instrument drivers. The Rust client code in `agent_runtime/src/hardware.rs` doesn't change — SiLA 2 is the interface contract.

## Quick Start

```bash
# Build the Rust workspace
cargo build

# Build the PyO3 vessel_physics extension (requires maturin)
pip install maturin
VIRTUAL_ENV=$VIRTUAL_ENV PATH="$VIRTUAL_ENV/bin:$PATH" \
    maturin develop --manifest-path vessel_physics/Cargo.toml

# Run pure-Rust tests (no external dependencies)
cargo test -p agent_runtime

# Start the SiLA 2 server and run integration tests
cd sila_mock && python3 -m axiomlab_mock --insecure -p 50052 &
cargo test -p agent_runtime --test vessel_simulation_e2e -- --ignored --test-threads=1
cargo test -p agent_runtime --test sila2_e2e -- --ignored --test-threads=1
```

### Run Verus proofs

```bash
# Install Verus (macOS ARM64, Linux x86-64, Linux ARM64)
# Download from https://github.com/verus-lang/verus/releases
# Extract to ~/verus/, chmod +x ~/verus/verus

~/verus/verus verus_verified/vessel_registry.rs
# Expected: verification results:: 11 verified, 0 errors

~/verus/verus verus_verified/lab_safety.rs
# Expected: verification results:: 6 verified, 0 errors

~/verus/verus verus_verified/protocol_safety.rs
# Expected: verification results:: 13 verified, 0 errors

# Regenerate the proof manifest from real Verus output
python3 vessel_physics/generate_manifest.py
```

## Formal Verification

AxiomLab has three sets of formally verified code (30 theorems total, 0 errors).

### Vessel Physics (`verus_verified/vessel_registry.rs`)

11 theorems proved by Verus (Z3 SMT solver).

| Theorem | What It Proves |
| --- | --- |
| `empty_satisfies_inv` | An empty vessel trivially satisfies the volume invariant |
| `proved_add` | Dispensing within capacity preserves `volume_nl ≤ max_nl` |
| `proved_sub` | Aspirating never underflows; result stays ≤ volume_nl |
| `dispense_preserves_inv` | Invariant holds after any valid dispense |
| `aspirate_preserves_inv` | Invariant holds after any valid aspirate |
| `dispense_chain_safe` | Two consecutive dispenses stay within capacity |
| `aspirate_inverts_dispense` | Aspirating exactly what was dispensed returns to original volume |
| `partial_aspirate_safe` | Partial aspirate stays within [0, max] |
| `fill_to_capacity_is_valid` | Filling to exactly max is valid |
| `drain_to_zero_is_valid` | Draining to zero is valid |
| `main` | Concrete exercise: dispense → dispense → aspirate → drain |

### Hardware Safety Bounds (`verus_verified/lab_safety.rs`)

Arm extension, temperature, pressure, rotation speed limits. 6 theorems, 0 errors.

### Protocol Safety (`verus_verified/protocol_safety.rs`)

Protocol-level invariants: step count bounded, total volume bounded, dilution series safe. 13 theorems, 0 errors.

### Other Tooling

| Tool | What It Does | State |
| --- | --- | --- |
| **proof_synthesizer** | LLM-in-the-loop Verus proof repair (observe → reason → act) | Requires Verus + LLM |

## Known Limitations

| Limitation | Detail |
| --- | --- |
| **Simulated hardware** | The SiLA 2 server returns simulated values. No real instruments have been connected. |
| **Local LLM** | qwen2.5-coder:7b is sufficient for structured tool calls but not for novel scientific reasoning. Discovery quality is model-dependent. |
| **Single-agent** | One LLM loop, one hardware pool. No multi-agent coordination. |
| **Replication aggregate is step-count only** | `ReplicateAggregate` reports mean ± SD of *steps succeeded* per replicate, not inter-replicate variability in the measurements themselves (e.g., SD of absorbance readings across replicates). |
| **Calibration is advisory** | The mandate warns when the pH meter calibration is stale, but no tool blocks a `read_ph` call if recalibration hasn't been performed. |
| **Audit is local + Rekor-anchored** | Each event is Ed25519-signed and hash-chained. Protocol conclusions and 15-minute chain-tip checkpoints are submitted to Sigstore Rekor. Log rotation (100 MB / daily) and cross-restart `session_start` chaining are implemented. A complete chain rewrite with a fresh key still passes local checks — HSM-backed keys and an external content mirror are needed for production. |
| **Key management** | Ed25519 signing is implemented but key custody, rotation, and revocation are manual. |

## Project Structure

```text
AxiomLab/
├── server/                  # Axum HTTP/WS server + in-memory event buffer + exploration loop
├── agent_runtime/           # Orchestrator, protocol executor, SiLA 2 clients, sandbox, capabilities, audit, Rekor
│   ├── src/hardware.rs      # SiLA 2 gRPC client pool (6 instruments)
│   ├── src/orchestrator.rs  # 5-stage validation pipeline + protocol execution
│   ├── src/protocol.rs      # Protocol, ProtocolPlan, ProtocolStep types
│   ├── src/protocol_executor.rs  # ProtocolExecutor: drives steps through orchestrator
│   ├── src/audit.rs         # Hash-chained JSONL audit log, per-event Ed25519 signatures
│   ├── src/rekor.rs         # Sigstore Rekor transparency-log anchoring
│   └── tests/               # Integration tests (vessel simulation, e2e, orchestrator)
├── vessel_physics/          # Rust VesselRegistry (u64 nl) + PyO3 Python bindings
│   ├── src/lib.rs           # proved_add, proved_sub, VesselRegistry, PyO3 class
│   └── generate_manifest.py # Runs Verus, writes vessel_physics_manifest.json
├── proof_artifacts/         # Manifest schema, RuntimePolicyEngine, signing
│   └── vessel_physics_manifest.json  # Real Verus compiler output (committed)
├── verus_verified/          # Verus-proved Rust source files
│   ├── vessel_registry.rs   # 11 theorems: volume invariant preservation
│   ├── lab_safety.rs        # 6 theorems: hardware bounds (arm, temp, pressure)
│   ├── protocol_safety.rs   # 13 theorems: step count, volume, dilution series bounds
│   └── lab_safety_UNSAFE.rs # Demonstrates code that Verus correctly rejects
├── scientific_compute/      # nalgebra, rustfft, OLS, Hill equation fitting, lab data parsing
├── physical_types/          # uom dimensional analysis
├── verus_proofs/            # Verus specs + dual-compilation shim
├── proof_synthesizer/       # LLM-driven Verus proof repair
├── sila_mock/               # Python SiLA 2 server (6 instruments, physics via vessel_physics)
│   └── axiomlab_mock/
│       ├── vessel_state.py          # PyO3 adapter → Rust backend
│       └── _vessel_state_python.py  # Pure-Python fallback (no Verus guarantees)
├── visualizer/              # React + Vite web dashboard
└── scripts/                 # proof_release_gate.sh
```

## License

MIT
