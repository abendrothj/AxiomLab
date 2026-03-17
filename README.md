# AxiomLab

> A memory-safe Rust runtime for autonomous AI-driven lab exploration, with formal verification tooling and SiLA 2 hardware integration.

## What This Actually Is

AxiomLab is a working prototype of an autonomous science agent. An LLM (local Ollama) continuously proposes lab experiments, and a Rust orchestrator validates every proposed action through a 5-stage safety pipeline before dispatching it to lab hardware over SiLA 2 gRPC. Results are streamed to a web dashboard in real time.

**What works today (tested, integrated, proven by 19 passing integration tests):**
- **5-stage tool validation pipeline** — sandbox allowlist → two-person approval → capability bounds → proof-artifact policy → audit + dispatch
- **SiLA 2 gRPC hardware layer** — 6 instruments (liquid handler, robotic arm, spectrophotometer, incubator, centrifuge, pH meter) with 12 operations, talking to a real gRPC server
- **Proof-policy gating** — Verus verification artifacts gate high-risk actions (actuation, destructive); read-only actions pass without proofs
- **Continuous autonomous loop** — LLM proposes → orchestrator validates → hardware executes → results feed back → LLM proposes next
- **Web visualizer** — real-time WebSocket dashboard with activity feed, state graph, and discovery journal
- **Immutable SQLite audit log** — append-only, WAL-mode event database that survives server restarts
- **Docker Compose** — three-service stack (Ollama LLM, SiLA 2 Python mock, AxiomLab Rust server) with health checks

**What is simulated / not yet production:**
- Hardware is a **Python SiLA 2 mock server** returning plausible values — not real physical instruments
- LLM is **qwen2.5-coder:7b** via local Ollama — capable enough for structured tool calls, not a frontier reasoning model
- The "science" is constraint-space exploration — the agent probes parameter bounds and reports what it finds, not novel chemistry
- Verus formal verification only runs on **x86-linux** (ARM gets a graceful stub)
- Two-person approval uses Ed25519 signatures but key management is manual / external

## Crate Map

| Crate | Purpose | Status |
|---|---|---|
| `server` | Axum HTTP + WebSocket server, SQLite event log, continuous exploration loop | Working, tested |
| `agent_runtime` | Orchestrator (5-stage validation), SiLA 2 gRPC clients (6 instruments), sandbox, capabilities, approvals, audit, tools | Working, 19 integration tests |
| `proof_artifacts` | Manifest schema, RuntimePolicyEngine, RiskClass/ActionPolicy, Ed25519 signing, CI gate | Working, used in production pipeline |
| `scientific_compute` | Pure-Rust linear algebra (`nalgebra`), FFT (`rustfft`), OLS regression, lab data parsing | Working |
| `physical_types` | Compile-time dimensional analysis via `uom` | Working |
| `verus_proofs` | Verus-compatible specs (dual `rustc`/Verus compilation shim), hardware-bound invariants | Compiles; Verus verification requires x86-linux |
| `proof_synthesizer` | VeruSAGE-inspired observe→reason→act loop for iterative Verus proof repair | Compiles; requires Verus + LLM to run |
| `aeneas_lean_semantics` | Rust MIR → Aeneas → Lean 4 translation pipeline | Compiles; requires Aeneas + Lean toolchain |

## Deployment Hardening: Five Validation Stages

Every tool call from the LLM passes through all five stages in `agent_runtime/src/orchestrator.rs` before reaching hardware. This is real code, not a design doc — it runs in production and is tested by 19 integration tests.

| Stage | Component | What It Does | Tested By |
|---|---|---|---|
| **0. Sandbox** | Command allowlist | Blocks tools not in the allowlist | `sandbox_rejects_disallowed_command`, `orchestrator_sandbox_blocks_unauthorized_tool` |
| **1. Approval** | Two-person control | Ed25519 signatures required for high-risk actions | `proof_policy_enforcement` tests |
| **2. Capability** | Numeric bounds | Rejects parameters outside hardware limits (e.g., volume > 1000µL, x > 300mm) | `capability_rejects_out_of_bounds_*`, `orchestrator_capability_rejects_then_retries` |
| **3. Fail-Closed** | High-risk gate | If no proof policy engine is configured, all actuation/destructive actions are denied | Implicit in orchestrator logic |
| **4. Proof Policy** | Artifact authorization | Checks Verus verification status; blocks actuation if proofs are missing/failed | `proof_policy_blocks_actuation_with_failed_verus`, `orchestrator_proof_policy_blocks_actuation_allows_reads` |
| **5. Dispatch** | Audit + execute | Logs hash-chained audit event, dispatches tool call over SiLA 2 gRPC | `orchestrator_drives_multi_step_experiment_through_sila2` |

## SiLA 2 Hardware Integration

AxiomLab talks to lab hardware using the [SiLA 2](https://sila-standard.com/) standard over gRPC. Six instruments are implemented:

| Instrument | Operations | Risk Class |
|---|---|---|
| Liquid Handler | `dispense`, `aspirate` | LiquidHandling |
| Robotic Arm | `move_arm` | Actuation |
| Spectrophotometer | `read_absorbance` | ReadOnly |
| Incubator | `set_temperature`, `read_temperature`, `incubate` | Actuation / ReadOnly |
| Centrifuge | `spin_centrifuge`, `read_centrifuge_temperature` | Actuation / ReadOnly |
| pH Meter | `read_ph`, `calibrate_ph` | ReadOnly |

**Current state:** A Python SiLA 2 mock server (`sila_mock/`) implements all six instruments with the `sila2` v0.14.0 library. The Rust side (`agent_runtime/src/hardware.rs`) uses `tonic` v0.12 gRPC clients. All 12 operations are tested end-to-end through the validation pipeline.

**To use real hardware:** Replace the Python mock with actual SiLA 2-compliant instrument drivers. The Rust client code doesn't change — SiLA 2 is the interface contract.

## Quick Start

### Option A: Docker Compose (recommended — everything works out of the box)

```bash
docker compose up --build
# Starts: Ollama (LLM) + SiLA 2 mock (hardware) + AxiomLab server
# Web dashboard: http://localhost:3000
# The agent begins autonomous exploration automatically
```

**What happens:** Ollama pulls `qwen2.5-coder:7b`, the Python SiLA 2 mock starts on port 50052, and the Rust server connects to both. The LLM proposes experiments, the orchestrator validates them through all 5 stages, and SiLA 2 gRPC calls execute against the mock instruments. Results stream to the web dashboard via WebSocket.

### Option B: Local development (no Docker, no LLM)

```bash
# Build the workspace
cargo build

# Run unit + pure-Rust tests (no external dependencies)
cargo test -p agent_runtime -- --test sila2_e2e capability sandbox proof_policy

# Run integration tests (requires SiLA 2 mock on localhost:50052)
cd sila_mock && python -m axiomlab_mock --insecure &
cargo test -p agent_runtime -- --test sila2_e2e --test orchestrator_sila2
```

### Option C: Run just the SiLA 2 integration tests

```bash
# Start the mock hardware server
cd sila_mock && python -m axiomlab_mock --insecure &

# Run all 19 integration tests
cargo test -p agent_runtime --test sila2_e2e --test orchestrator_sila2 2>&1 | tail -5
# Expected: test result: ok. 19 passed; 0 failed
```

## Test Coverage

### Integration Tests (19 total — all passing)

**`agent_runtime/tests/sila2_e2e.rs`** — 14 tests covering the validation pipeline at the component level:
- 8 tests hit the live SiLA 2 mock over gRPC (dispense, move_arm, read_absorbance, spin_centrifuge, pH, full pipeline with/without proof policy)
- 6 pure-Rust tests validate sandbox rejection, capability bounds, and proof policy logic without any network

**`agent_runtime/tests/orchestrator_sila2.rs`** — 5 tests using the real `Orchestrator.run_experiment()` with a scripted LLM:
- Multi-step experiment (move_arm → dispense → read_absorbance → conclude)
- Proof policy blocks actuation but allows reads
- Capability bounds reject then retry within limits
- Sandbox blocks unauthorized tools
- Full 5-instrument titration workflow (calibrate_ph → move_arm → dispense → read_ph → read_absorbance)

**What these tests prove:** The complete validation pipeline works end-to-end — LLM output is parsed, validated through all 5 stages, dispatched over real gRPC to a real server, and results are captured with audit trails. Rejection at each stage is independently tested.

**What these tests don't prove:** Real physical hardware safety (the mock returns plausible values, not real sensor data). LLM reasoning quality (the scripted LLM always makes the right call). Network failure handling. Concurrent multi-agent scenarios.

## Docker Compose Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────────────┐
│   Ollama     │     │  SiLA 2 Mock │     │   AxiomLab       │
│  (LLM)      │◄────│  (Hardware)  │◄────│   (Rust Server)  │
│  port 11434  │     │  port 50052  │     │   port 3000      │
│  qwen2.5     │     │  6 instruments│    │   axum + ws      │
│  -coder:7b   │     │  Python/gRPC │     │   SQLite audit   │
└─────────────┘     └──────────────┘     └──────────────────┘
```

**Environment variables:**
- `SILA2_ENDPOINT` — gRPC endpoint for hardware (default: `http://sila2-mock:50052`)
- `AXIOMLAB_LLM_ENDPOINT` — Ollama API (default: `http://ollama:11434/v1`)
- `AXIOMLAB_LLM_MODEL` — model name (default: `qwen2.5-coder:7b`)
- `VERUS_VERIFIED` — set to `1` in Docker to indicate Verus verification passed at build time
- `AXIOMLAB_DOCKER` — set to `1` inside the container
## Proof Release Gate

```bash
./scripts/proof_release_gate.sh
```

Runs a 10-step release gate that builds the binary, generates and signs a proof manifest, enforces CI policy checks, runs sandbox/policy tests, verifies the audit chain, and exports a replayable compliance bundle. Outputs are in `.artifacts/proof/`.

## Formal Verification Tooling

AxiomLab includes three formal verification paths. These are **tooling integrations**, not claims that the entire system is formally verified.

| Tool | What It Does | Current State |
|---|---|---|
| **Verus** | SMT-based verification of Rust code via Z3 | `verus_verified/lab_safety.rs` and `dilution_protocol.rs` are verified. Verus only runs on x86-linux. ARM gets a stub. |
| **Aeneas** | Translates Rust MIR → pure lambda calculus → Lean 4 | Pipeline is implemented in `aeneas_lean_semantics/`. Requires Aeneas binary. |
| **Lean 4** | Interactive theorem prover for verifying translated code | Lean files exist in `lean4/`. Requires Lean toolchain. |

**Proof synthesis** (`proof_synthesizer/`): An LLM-in-the-loop agent that invokes the Verus compiler, parses diagnostics, and asks the LLM to fix proof annotations. Inspired by VeruSAGE. Requires both Verus and an LLM to run.

## Web Visualizer

A React + Vite dashboard that connects to the server via WebSocket:
- **Left panel:** Live activity feed — tool executions with success/rejection status
- **Center panel:** State transition graph via ReactFlow
- **Right panel:** Discovery journal — experiment conclusions logged by the agent
- **Header:** Iteration counter, current stage, connection status

Persists across refreshes — journal and history are loaded from SQLite on page load.

```bash
cd visualizer && npm install && npm run dev
# Connects to AxiomLab server on localhost:3000
```

## Known Limitations

These are honest assessments — not future roadmap items.

| Limitation | Detail |
|---|---|
| **Mock hardware** | The SiLA 2 mock returns plausible fake data. No real instruments have been connected. |
| **Local LLM** | qwen2.5-coder:7b is good enough for structured tool calls but is not a frontier reasoning model. Discovery quality depends on the model. |
| **Verus is x86-only** | Formal verification only runs on x86-linux. ARM builds skip Verus gracefully. |
| **No real science yet** | The agent explores parameter bounds of mock instruments. It hasn't discovered anything novel. |
| **Single-agent** | One LLM loop, one hardware pool. No multi-agent coordination. |
| **Key management** | Ed25519 signing is implemented but key custody, rotation, and revocation are manual. |
| **Audit is local** | Hash-chained audit log detects local tampering but has no external anchor or per-event signatures. |
| **No real error recovery** | If gRPC fails or LLM returns garbage, the loop logs and retries. No sophisticated retry/fallback logic. |

## Project Structure

```
AxiomLab/
├── server/              # Axum HTTP/WS server + SQLite + exploration loop
├── agent_runtime/       # Orchestrator, SiLA 2 clients, sandbox, capabilities, audit
│   ├── src/hardware.rs  # SiLA 2 gRPC client pool (6 instruments)
│   ├── src/orchestrator.rs  # 5-stage validation pipeline
│   ├── tests/sila2_e2e.rs   # 14 integration tests
│   └── tests/orchestrator_sila2.rs  # 5 orchestrator-level tests
├── proof_artifacts/     # Manifest schema, policy engine, signing
├── scientific_compute/  # nalgebra, rustfft, OLS, lab data
├── physical_types/      # uom dimensional analysis
├── verus_proofs/        # Verus specs + verification shim
├── proof_synthesizer/   # LLM-driven Verus proof repair
├── aeneas_lean_semantics/  # MIR → Aeneas → Lean pipeline
├── lean4/               # Lean 4 theorem files
├── verus_verified/      # Verified Rust source (lab_safety.rs)
├── sila_mock/           # Python SiLA 2 mock server (6 instruments)
├── visualizer/          # React + Vite web dashboard
├── scripts/             # proof_release_gate.sh
├── docker-compose.yml   # Three-service stack
└── Dockerfile           # Multi-stage build (Verus + Aeneas + Lean + Rust)
```

## License

MIT
