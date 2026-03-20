# AxiomLab

> A memory-safe Rust runtime for autonomous AI-driven lab exploration, with formal verification, SiLA 2 hardware integration, and ISO 17025 compliance infrastructure.

## What This Is

AxiomLab is a working prototype of an autonomous science agent. An LLM continuously proposes lab experiments. A Rust orchestrator validates every proposed action through a multi-stage safety pipeline before dispatching it to lab hardware over SiLA 2 gRPC. Hardware physics — volumes, capacities, overflow prevention — are enforced by formally verified Rust code proved correct by the Verus SMT solver. Multi-step experiments are expressed as structured protocols with cryptographically signed audit records anchored to the Sigstore Rekor transparency log.

The system is designed to be grounded in real practice: GUM-compliant uncertainty budgets, ISO 17025 method validation records, QA sign-off with tamper-evident hashes, and reagent inventory tracking with chemical compatibility checks.

---

## What Works Today (tested, integrated)

### Safety and Validation

- **Multi-stage tool validation pipeline** — every tool call traverses stages in sequence:
  - Stage 0.1 — calibration check: hard-deny for uncalibrated quantitative instruments (`read_ph`, `read_absorbance`); warn-only for expired calibration on qualitative tools
  - Stage 0.25 — chemical compatibility: checks proposed reagent addition against current vessel contents using an embedded GHS/NFPA 704 incompatibility table; denies if hazardous combination detected
  - Stage 1 — sandbox allowlist: blocks tools not in the explicit allowlist
  - Stage 2 — two-person approval: Ed25519 signatures required for high-risk actions; revocation list checked
  - Stage 3 — capability bounds: rejects parameters outside hardware limits (volume, position, temperature, etc.)
  - Stage 4 — fail-closed / proof policy: if no proof engine is configured, all actuation is denied; otherwise checks Verus artifact status from manifest
  - Stage 5 — audit + dispatch: logs Ed25519-signed hash-chained event, dispatches over SiLA 2 gRPC

- **Hardware e-stop** — `POST /api/emergency-stop` (JWT required): sets the running flag to false, sends SiLA 2 `Abort` to all 6 instruments concurrently, and emits a signed audit event. In simulator mode, only the flag is cleared.

- **Chemical compatibility matrix** — embedded JSON table (GHS/NFPA 704) covers common wet-chemistry incompatibilities (acids/bases, oxidizers/reducers, water-reactives). Stage 0.25 in the orchestrator pipeline; graceful no-op when lab state is not initialized.

### Hardware

- **SiLA 2 gRPC hardware layer** — 6 instruments (liquid handler, robotic arm, spectrophotometer, incubator, centrifuge, pH meter) with 12 operations
- **In-process physics simulator** — when `SILA2_ENDPOINT` is unreachable, falls back to an in-process simulator with Beer-Lambert absorbance, pH response curves, and enzyme kinetics

### Formal Verification (30 theorems, 0 errors)

- **Vessel physics** (`verus_verified/vessel_registry.rs`) — 11 theorems: volume invariant preservation, overflow prevention, aspirate/dispense correctness, sequential safety
- **Hardware safety bounds** (`verus_verified/lab_safety.rs`) — 6 theorems: arm extension, temperature, pressure, rotation speed limits
- **Protocol safety** (`verus_verified/protocol_safety.rs`) — 13 theorems: step count ≤ 20, total volume ≤ 200 mL, dilution series correctness

The Verus proofs gate high-risk actions at runtime: `proved_add`/`proved_sub` operations (Z3-verified) are what the Python SiLA 2 server actually calls via PyO3.

### Audit and Integrity

- **Hash-chained JSONL audit log** — every event is individually Ed25519-signed and chained; log rotation (100 MB / daily) with cross-restart session continuity
- **Sigstore Rekor anchoring** — protocol conclusions and 15-minute chain-tip checkpoints are submitted to the public Rekor transparency log
- **Streaming audit query API** — `GET /api/audit` reads line-by-line via `BufReader` (never loads the full file into RAM); `GET /api/audit/raw` streams via `tokio::fs::File` + `ReaderStream` for zero-copy delivery of large logs
- **ZK audit proof layer** — `zk_audit` crate proves chain properties (event count, violation count, chain validity) without disclosing log content; use cases are `ConfidentialRegulatory` (IP protection with regulators) or `ConfidentialAudit` (compliance proof to sponsor). `GET /api/audit/zk-status` reports configuration. The ZK layer complements, not replaces, Rekor (Rekor provides public timestamping; ZK adds content confidentiality).

### Authentication and Access Control

- **JWT HTTP authentication** — HS256 middleware on all mutating routes (`POST`, `DELETE`, `PUT`) and `/api/audit/raw`; read-only `GET` routes remain open for dashboard embedding. Dev mode (no `AXIOMLAB_JWT_SECRET`) logs a warning and accepts all.
- **WebSocket JWT auth** — upgrade handler checks `?token=<jwt>` query param; rejects with 401 on invalid/expired token. Set `AXIOMLAB_WS_AUTH=0` to disable (e.g., for local dashboards).
- **OIDC PKCE flow** — `GET /api/auth/oidc/start` and `GET /api/auth/oidc/callback` implement the browser PKCE flow; issues the same HS256 JWT format as the Ed25519 token path. Enabled when `AXIOMLAB_OIDC_*` env vars are set.
- **Token generator CLI** — `cargo run -p agent_runtime --bin tokengen -- --operator-id alice --role operator --ttl-hours 8`

### Scientific Compute

- **Design of Experiments (DoE)** — three designs, all tested:
  - `full_factorial` — 2^k design, all combinations of low/high; k ≤ 5 (max 32 runs)
  - `central_composite` — face-centered CCD for response-surface models; 2 ≤ k ≤ 4
  - `latin_hypercube` — space-filling design with LCG RNG + Fisher-Yates shuffle; reproducible by seed, n ≤ 200
  - Exposed to the LLM via the `design_experiment` tool; returns a JSON run matrix the LLM uses to propose structured `Protocol`s
  - **DoE audit linkage** — pass `design_json` from `design_experiment` as `doe_design_json` in `propose_protocol`; the runtime stores the matrix in the `Protocol` record and runs one-way ANOVA automatically at conclusion (`ProtocolRunResult.doe_anova`: F-statistic, p-value, group counts)
- **Curve fitting** — OLS regression (slope, R², intercept), Hill equation (EC50, E_max, Hill coefficient), Michaelis-Menten (Vmax, Km), AIC-based model selection; exposed via `analyze_series` tool
- **One-way ANOVA** — F-statistic, p-value (incomplete beta / Lentz continued fraction), eta-squared
- **OLS linear regression** — multi-variable OLS with Gaussian elimination + partial pivoting; R², adjusted R², residual standard error
- **GUM uncertainty propagation** — combined standard uncertainty from (u_i, sensitivity_coeff) pairs

### Uncertainty Quantification (GUM-compliant)

- **Instrument uncertainty specs** — `ToolSpec.instrument_uncertainty` carries Type A (repeatability fraction σ/reading) and Type B (absolute systematic) for `read_ph`, `read_absorbance`, `read_temperature`
- **Per-protocol uncertainty budgets** — `ProtocolRunResult.uncertainty_budgets` includes one `UncertaintyBudget` per measured parameter: combined standard uncertainty, Welch-Satterthwaite effective degrees of freedom, t-distribution coverage factor (k), and expanded uncertainty U = k·u_c at 95% confidence

### Reagent Inventory and Lab State

- **Reagent inventory** — `LabState` tracks reagents (id, name, CAS number, lot, concentration, volume, expiry, GHS hazard codes, `nominal_ph`) and vessel contents; persisted to `.artifacts/lab_state.json`
- **LabState sync during tool execution** — `dispense` accepts an optional `reagent_id` param; on success the orchestrator calls `lab_state.add_to_vessel()` and `deduct_volume()`. `aspirate` calls `remove_from_vessel()`. Both sync and save on every successful call.
- **Physics-based pH simulation** — `read_ph` computes a weighted-average `nominal_ph` from all reagents registered in the vessel (`lab_state.vessel_contents`), applies ±1% noise, and falls back to 7.0 if no `nominal_ph` is set. Replaces the previous hardcoded 7.2 stub.
- **API**: `GET /api/lab/reagents`, `POST /api/lab/reagents` (JWT), `DELETE /api/lab/reagents/{id}` (JWT), `GET/PUT /api/lab/vessels/{id}/contents` (PUT requires JWT)
- **Calibration status API** — `GET /api/lab/calibration-status` lists per-instrument calibration validity; Stage 0.1 hard-denies quantitative tools on uncalibrated instruments

### ISO 17025 Compliance Infrastructure

All records are persisted in the discovery journal (`journal.json`) and survive restarts.

- **Method validation records** (`MethodValidation`, ISO 17025 §7.2.2) — documents linearity range, LOD, LOQ, repeatability CV%, reproducibility CV%, spike recovery %, validated_by, run_ids as audit trail. Routes: `GET/POST /api/methods`, `GET /api/methods/{id}`
- **Certified reference materials** (`ReferenceMaterial`, ISO 17025 §6.4) — certified values as `(value, expanded_uncertainty)` pairs, lot number, expiry, certificate URL. Routes: `GET/POST /api/lab/reference-materials`
- **Study records with QA sign-off** (`StudyRecord`, ISO 17025 §8.3) — pre-registered protocol IDs, completed run IDs, study director, independent QA reviewer (enforced: reviewer ≠ director), SHA-256 sign-off hash embedded in the audit chain. Routes: `GET/POST /api/studies`, `GET /api/studies/{id}`, `POST /api/studies/{id}/protocols`, `POST /api/studies/{id}/qa-review`

### Multi-Agent Scheduling

- **Concurrent experiment slots** — `AXIOMLAB_EXPERIMENT_SLOTS` (default 1, max 4) controls how many experiments run simultaneously via a `JoinSet`; each slot gets its own `Orchestrator` instance
- **`LabScheduler`** — tracks active slots and per-slot instrument locks; prevents two experiments from competing for the same physical instrument; clamp to [1, 4]
- **Backward compatible** — `slot_count == 1` is identical in behavior to the original sequential loop

### SQLite Persistence

- **`server/src/db.rs`** — WAL-mode SQLite (bundled), schema for `findings`, `hypotheses`, `runs`, `calibrations`, `audit_index`; opened at startup with graceful reconstruction from the JSON backup if the DB file is missing

### Notifications and Alerts

- **`NotificationSink` trait** — `send(event) → Future<()>`, dyn-compatible via boxed future
- **`WebhookNotifier`** — POSTs JSON to `AXIOMLAB_ALERT_WEBHOOK_URL`; auto-detects Slack (`hooks.slack.com`) and Discord (`discord.com/api/webhooks`) by URL and formats accordingly; generic JSON fallback for custom receivers
- **Events covered**: `ExperimentFailed`, `EmergencyStopTriggered`, `ApprovalTimeout`, `AuditChainInvalid`, `CalibrationExpired`, `RekorAnchorFailed`

### Integration Layer

- **PubChem proxy** — `GET /api/literature/search?q=<compound>` proxies to PubChem PUG REST API; returns CID, IUPAC name, molecular formula, MW, canonical SMILES; `fetch_protocol_hints()` extracts compound properties from a hypothesis string for LLM mandate injection
- **ELN adapter** — `ELNAdapter` trait + `BenchlingAdapter`; `POST /api/export/benchling/{study_id}` (JWT required) maps a `StudyRecord` to a Benchling notebook entry and returns the entry URL; returns 503 when `AXIOMLAB_BENCHLING_*` vars are not set

### Discovery and Experiment Management

- **Continuous autonomous loop** — LLM proposes → orchestrator validates → hardware executes → results feed back → journal records → LLM proposes next; convergence detection slows the loop when all hypotheses are settled
- **Multi-slot JoinSet loop** — up to 4 independent experiments run concurrently, each with its own iteration context
- **Hypothesis lifecycle** — proposed → testing → confirmed / rejected; journal tracks all transitions with timestamps
- **Auto-findings from curve fits** — when `analyze_series` produces R² ≥ 0.80 (linear) or valid Hill fit, a `source: "system"` finding with typed `Measurement` structs is auto-recorded in the discovery journal and signed into the audit chain
- **Evidence-gated convergence** — the loop only marks an experiment converged when at least one `source: "system"` finding exists (written by `analyze_series` at R² ≥ 0.80). The LLM cannot fake convergence by calling `confirm_hypothesis` with no data.
- **Chain-of-thought in audit** — `reasoning_text` extracted from every LLM response is forwarded to all `audit_decision` calls across all 6 pipeline stages, making the LLM's rationale part of the tamper-evident audit record.
- **Parameter-space coverage tracking** — numeric tool inputs logged as `ParameterProbe` records (capped at 500); `coverage_summary_for_llm()` injects `[min, max] · N values` per parameter into the LLM mandate

---

## What Is Simulated / Not Yet Production

| Item | Reality |
|------|---------|
| **Physical hardware** | Python SiLA 2 mock server returns simulated values. No real instruments connected. |
| **LLM** | OpenAI-compatible API; tested with local Ollama (`qwen2.5-coder:7b`). Not a frontier reasoning model. Discovery quality is model-dependent. |
| **Key management** | Ed25519 audit signing key is persisted across restarts via `FileBackedSigner` (env var → file → auto-generate at `~/.config/axiomlab/`). HSM storage and rotation policy are external. |
| **Benchling export** | `BenchlingAdapter` constructs correctly-shaped API requests; not tested against the live Benchling API (requires a real token). |
| **PubChem integration** | `search_pubchem()` makes live HTTP requests; requires network access and is subject to PubChem rate limits. |
| **OIDC** | PKCE flow is implemented and tested in unit tests; requires a real OpenID Connect identity provider (Google, Keycloak, etc.) to function end-to-end. |
| **ZK proofs** | `zk_audit` crate defines types and use-case semantics; proof generation requires RISC Zero + on-chain contract deployment. |
| **Multi-agent contention** | `LabScheduler` enforces slot and instrument locks correctly (unit-tested); not validated against real concurrent SiLA 2 hardware. |

---

## Proof Chain

```text
LLM response (tool call JSON + reasoning_text extracted)
  → Stage 0:   sandbox allowlist
  → Stage 0.1: calibration check (hard deny if uncalibrated)
  → Stage 0.25: chemical compatibility (deny if GHS/NFPA incompatible)
  → Stage 1:   approval — Ed25519 two-person control, revocation checked
  → Stage 2:   capability bounds (hardware parameter limits)
  → Stage 3:   fail-closed (deny all actuation if no proof engine)
  → Stage 4:   proof policy (reads vessel_physics_manifest.json)
               RuntimePolicyEngine checks ArtifactStatus::Passed for LiquidHandling
  → Stage 5:   audit + dispatch
               Ed25519-signed, hash-chained event written to JSONL
               reasoning_text included in every audit entry at every stage
               SiLA 2 gRPC call to instrument
                 → Python server → PyO3 → Rust VesselRegistry
                     → proved_add / proved_sub (Z3-verified integer arithmetic)
               LabState updated (add_to_vessel / deduct_volume) on dispense/aspirate
```

For structured protocol runs:
```text
LLM emits ProtocolPlan JSON  (optionally with doe_design_json)
  → parsed and validated (step count ≤ 20, tool names non-empty)
  → ProtocolExecutor iterates steps, each through the full pipeline above
  → ProtocolStepRecord (tool, params, result, proof_artifact_hash, chain hash, Ed25519 sig)
  → UncertaintyBudget computed per measured parameter (GUM §4.3)
  → ProtocolConclusionRecord (LLM conclusion, Ed25519 signed)
  → if doe_design_json set: one-way ANOVA run on step responses by factor level
      → DoeAnovaResult (F-statistic, p-value) stored in ProtocolRunResult
  → Sigstore Rekor submission (UUID + integrated timestamp)
```

---

## Crate Map

| Crate | Purpose | Workspace |
|-------|---------|-----------|
| `server` | Axum HTTP + WebSocket server, exploration loop (JoinSet multi-slot), all API routes | ✓ |
| `agent_runtime` | Orchestrator (6-stage pipeline), protocol executor, SiLA 2 clients, approvals, audit, Rekor, lab state, chemistry, notifications | ✓ |
| `proof_artifacts` | Manifest schema, RuntimePolicyEngine, Ed25519 signing, CI gate | ✓ |
| `scientific_compute` | DoE (full factorial, CCD, LHC), OLS regression, Hill/MM fitting, ANOVA, FFT, GUM uncertainty propagation | ✓ |
| `zk_audit` | ZK proof types and use-case semantics | ✓ |
| `vessel_physics` | Formally verified VesselRegistry (u64 nl) + PyO3 Python bindings | external |
| `physical_types` | Compile-time dimensional analysis via `uom` | external |
| `verus_proofs` | Verus-compatible specs + dual-compilation shim | external |
| `proof_synthesizer` | LLM-driven Verus proof repair (requires Verus + LLM) | external |

---

## API Surface

### Public (no auth required)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/approvals` | Browser approval UI — review, approve, or deny pending high-risk actions |
| GET | `/ws?token=<jwt>` | WebSocket event stream (token required when `AXIOMLAB_WS_AUTH` ≠ 0) |
| GET | `/api/status` | Running state, iteration counter, slot count |
| GET | `/api/history` | In-memory event snapshot |
| GET | `/api/journal` | Full discovery journal |
| GET | `/api/journal/findings` | Findings array |
| GET | `/api/audit` | Filtered JSONL query (`?action=&decision=&since=&limit=`) |
| GET | `/api/audit/verify` | Hash-chain integrity check |
| GET | `/api/audit/zk-status` | ZK proof configuration and use-case |
| GET | `/api/approvals/stalled` | Stalled approval IDs |
| GET | `/api/lab/reagents` | Reagent inventory |
| GET | `/api/lab/vessels` | Vessel contents |
| GET | `/api/lab/calibration-status` | Per-instrument calibration validity |
| GET | `/api/methods` | Method validation records |
| GET | `/api/methods/{id}` | Single method validation |
| GET | `/api/lab/reference-materials` | Certified reference materials |
| GET | `/api/studies` | Study records |
| GET | `/api/studies/{id}` | Single study record |
| GET | `/api/literature/search?q=` | PubChem compound lookup |
| GET | `/api/approvals/pending` | Pending approvals (JSON) |
| POST | `/api/approvals/submit` | Submit approval decision |
| GET | `/api/auth/oidc/start` | OIDC PKCE login redirect |
| GET | `/api/auth/oidc/callback` | OIDC callback + JWT issuance |

### Protected (JWT required)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/emergency-stop` | Halt loop + abort all SiLA 2 instruments |
| GET | `/api/audit/raw` | Stream full JSONL audit log |
| POST | `/api/approvals/recover/{id}` | Clear stalled dispatch |
| POST | `/api/approvals/recover/{id}/cancel` | Cancel stalled dispatch |
| POST | `/api/lab/reagents` | Register reagent |
| DELETE | `/api/lab/reagents/{id}` | Remove reagent |
| PUT | `/api/lab/vessels/{id}/contents` | Set vessel contents |
| POST | `/api/methods` | Create method validation record |
| POST | `/api/lab/reference-materials` | Register reference material |
| POST | `/api/studies` | Create study record |
| POST | `/api/studies/{id}/protocols` | Pre-register protocol in study |
| POST | `/api/studies/{id}/qa-review` | QA sign-off (reviewer ≠ director enforced) |
| POST | `/api/export/benchling/{study_id}` | Export study to Benchling |
| POST | `/api/auth/logout` | Stateless logout advisory |

---

## Validation Pipeline Stages

| Stage | Component | What It Does |
|-------|-----------|--------------|
| **0.1** | Calibration check | Hard-deny `read_ph`/`read_absorbance` if instrument has no calibration record; warn-only if expired |
| **0.25** | Chemical compatibility | Checks vessel contents + proposed reagent against GHS/NFPA table; denies incompatible addition |
| **1** | Sandbox allowlist | Blocks tools not in the explicit allowlist |
| **2** | Approval | Ed25519 two-person control for high-risk actions; revocation list checked |
| **3** | Capability bounds | Rejects parameters outside hardware limits |
| **4** | Fail-closed + proof policy | Deny all actuation if no proof engine; check Verus artifact status from manifest |
| **5** | Audit + dispatch | Hash-chained Ed25519 audit event; SiLA 2 gRPC dispatch |

---

## SiLA 2 Hardware

| Instrument | Operations | Risk Class |
|------------|------------|------------|
| Liquid Handler | `dispense`, `aspirate` | LiquidHandling |
| Robotic Arm | `move_arm` | Actuation |
| Spectrophotometer | `read_absorbance` | ReadOnly |
| Incubator | `set_temperature`, `read_temperature`, `incubate` | Actuation / ReadOnly |
| Centrifuge | `spin_centrifuge`, `read_centrifuge_temperature` | Actuation / ReadOnly |
| pH Meter | `read_ph`, `calibrate_ph` | ReadOnly |

---

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `AXIOMLAB_JWT_SECRET` | — | Base64-encoded HS256 secret (≥16 bytes); absent = dev/open mode |
| `AXIOMLAB_WS_AUTH` | `1` | Set to `0` to allow unauthenticated WebSocket connections |
| `AXIOMLAB_EXPERIMENT_SLOTS` | `1` | Concurrent experiment slots (1–4) |
| `AXIOMLAB_ALERT_WEBHOOK_URL` | — | Slack / Discord / generic webhook for failure alerts |
| `AXIOMLAB_AUDIT_SIGNING_KEY` | — | Ed25519 private key (hex) for per-event audit signatures |
| `AXIOMLAB_BASE_RPC_URL` | — | Base L2 RPC endpoint for ZK proof anchoring |
| `AXIOMLAB_BASE_CONTRACT_ADDR` | — | Deployed `AuditVerifier` contract address |
| `AXIOMLAB_BASE_WALLET_KEY` | — | Hex private key for on-chain transaction submission |
| `AXIOMLAB_ZK_USE_CASE` | `confidential_audit` | `confidential_regulatory` or `confidential_audit` |
| `AXIOMLAB_BENCHLING_TOKEN` | — | Benchling API token |
| `AXIOMLAB_BENCHLING_TENANT` | — | Benchling tenant (e.g. `myorg.benchling.com`) |
| `AXIOMLAB_BENCHLING_PROJECT_ID` | — | Benchling project for entry creation |
| `AXIOMLAB_OIDC_ISSUER_URL` | — | OIDC provider URL (e.g. `https://accounts.google.com`) |
| `AXIOMLAB_OIDC_CLIENT_ID` | — | OIDC client ID |
| `AXIOMLAB_OIDC_CLIENT_SECRET` | — | OIDC client secret |
| `AXIOMLAB_OIDC_REDIRECT_URI` | — | OIDC redirect URI |
| `PORT` | `3000` | HTTP server port |
| `SILA2_ENDPOINT` | `http://127.0.0.1:50052` | SiLA 2 gRPC endpoint |

---

## Quick Start

```bash
# Build the Rust workspace
cargo build

# Run pure-Rust unit tests (no external dependencies)
cargo test --workspace --exclude zk_audit

# Run integration tests (requires SiLA 2 mock server)
cd sila_mock && python3 -m axiomlab_mock --insecure -p 50052 &
cargo test -p agent_runtime --test vessel_simulation_e2e -- --ignored --test-threads=1

# Generate a JWT for protected API calls
cargo run -p agent_runtime --bin tokengen -- --operator-id alice --role operator --ttl-hours 8

# Start the server (simulator mode, no hardware required)
cargo run -p axiomlab-server

# Start with 2 concurrent experiment slots
AXIOMLAB_EXPERIMENT_SLOTS=2 cargo run -p axiomlab-server

# Verify Verus proofs (requires Verus binary at ~/verus/verus)
~/verus/verus verus_verified/vessel_registry.rs    # 11 verified, 0 errors
~/verus/verus verus_verified/lab_safety.rs          # 6 verified, 0 errors
~/verus/verus verus_verified/protocol_safety.rs     # 13 verified, 0 errors
```

---

## Formal Verification

### Vessel Physics (`verus_verified/vessel_registry.rs`) — 11 theorems

| Theorem | What It Proves |
|---------|----------------|
| `empty_satisfies_inv` | Empty vessel trivially satisfies the volume invariant |
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

### Hardware Safety Bounds (`verus_verified/lab_safety.rs`) — 6 theorems

Arm extension, temperature, pressure, rotation speed limits.

### Protocol Safety (`verus_verified/protocol_safety.rs`) — 13 theorems

Step count ≤ 20, total volume ≤ 200 mL, dilution series correctness.

---

## Known Limitations

| Limitation | Detail |
|------------|--------|
| **Simulated hardware** | The SiLA 2 server returns simulated values. No real instruments connected. |
| **Local LLM** | `qwen2.5-coder:7b` handles structured tool calls; not a frontier reasoning model. Scientific hypothesis quality is model-dependent. |
| **Key management** | Ed25519 audit key persisted via `FileBackedSigner`; HSM storage and rotation policy are external. |
| **Replication measurement stats** | `ReplicateAggregate` reports mean ± SD of steps-succeeded per replicate, not inter-replicate variability in the measurements themselves. |
| **Audit integrity bound** | Each event is Ed25519-signed and hash-chained. A complete chain rewrite with a fresh key still passes local checks. HSM-backed keys and an external content mirror are needed for production. |
| **DoE ANOVA grouping** | Auto-ANOVA groups step responses by low/high bracket of the first DoE factor only. Multi-factor or interaction effects require manual analysis. |
| **LabState ↔ physics sync** | `LabState` tracks reagent identity; `SimVesselState` tracks volumes. They share vessel IDs but the pH computation uses identity (nominal_ph) not concentration-weighted physics. |
| **PubChem rate limits** | `search_pubchem()` makes live HTTP calls; subject to PubChem's unauthenticated request limits. |
| **Benchling integration** | `BenchlingAdapter` constructs correct API requests; not tested against the live Benchling API. |

---

## Project Structure

```text
AxiomLab/
├── server/                     # Axum HTTP/WS server, exploration loop (JoinSet), all routes
│   ├── src/main.rs             # Route registration, AppState, startup
│   ├── src/simulator/mod.rs    # JoinSet multi-slot exploration loop
│   ├── src/simulator/mandate.rs  # LLM system prompt (mandate) builder
│   ├── src/simulator/tools.rs    # Tool schema definitions for LLM (physics pH, sensor dispatch)
│   ├── src/approvals_ui.rs     # GET /approvals — serves static approval page
│   ├── src/approvals.html      # Vanilla HTML approval dashboard (auto-refresh, Deny/Approve)
│   ├── src/discovery.rs        # DiscoveryJournal + ISO 17025 types
│   ├── src/db.rs               # SQLite persistence (WAL mode)
│   ├── src/auth.rs             # JWT middleware (HS256)
│   ├── src/oidc.rs             # OIDC PKCE flow
│   ├── src/lab_scheduler.rs    # LabScheduler (slots + instrument locks)
│   ├── src/eln.rs              # ELNAdapter trait + BenchlingAdapter
│   ├── src/literature.rs       # PubChem proxy + fetch_protocol_hints
│   └── src/audit_query.rs      # Streaming JSONL query API
├── agent_runtime/              # Orchestrator, safety pipeline, all tools
│   ├── src/orchestrator.rs     # 6-stage validation pipeline
│   ├── src/hardware.rs         # SiLA 2 gRPC clients (6 instruments) + abort_all()
│   ├── src/audit.rs            # Hash-chained JSONL, Ed25519, Rekor
│   ├── src/chemistry.rs        # Chemical compatibility checker
│   ├── src/lab_state.rs        # Reagent inventory + vessel contents
│   ├── src/notifications.rs    # NotificationSink + WebhookNotifier
│   ├── src/protocol.rs         # Protocol types + UncertaintyBudget (GUM)
│   ├── src/units.rs            # Unit validation (is_known_unit)
│   └── src/bin/tokengen.rs     # JWT generator CLI
├── scientific_compute/         # DoE, ANOVA, OLS regression, GUM, FFT, fitting
│   ├── src/doe.rs              # full_factorial, central_composite, latin_hypercube
│   └── src/stats.rs            # anova_one_way, linear_regression, propagate_uncertainty
├── proof_artifacts/            # Manifest schema, RuntimePolicyEngine, Ed25519, CI gate
│   └── vessel_physics_manifest.json  # Real Verus compiler output (committed)
├── zk_audit/                   # ZK proof types (ZkUseCase, AuditSummary, ZkConfig)
├── vessel_physics/             # u64 nl VesselRegistry + PyO3 bindings + proved_add/sub
├── verus_verified/             # Verus source files (30 theorems, 0 errors)
├── verus_proofs/               # Dual rustc/Verus compilation shim + specs
├── proof_synthesizer/          # LLM-driven Verus proof repair
├── physical_types/             # uom dimensional analysis
├── sila_mock/                  # Python SiLA 2 server (6 instruments, vessel_physics via PyO3)
├── visualizer/                 # React + Vite web dashboard
└── contracts/AuditVerifier.sol # On-chain audit verification (Base L2)
```

## License

MIT
