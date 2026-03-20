# AxiomLab Operator Guide

Operational reference for running, testing, and understanding the system. This document describes what the system **actually does** — not aspirations.

## 1) System Architecture

AxiomLab is a Rust workspace (9 crates) + a Python SiLA 2 server + a React web dashboard. The core loop: an LLM proposes lab experiments → a 6-stage validation pipeline checks every proposed action → validated actions execute over SiLA 2 gRPC → results feed back to the LLM.

### 1.1 Crate Roles

**Production path (server + agent_runtime + proof_artifacts + vessel_physics):**

- **server** — Axum HTTP server with WebSocket event streaming, SQLite-indexed audit log, multi-slot experiment scheduler, and the continuous exploration loop that drives the agent.
  - [server/src/main.rs](server/src/main.rs) — HTTP router (see §5 for full API surface), JWT middleware, OIDC handlers, experiment scheduler initialization, ZK status route
  - [server/src/simulator/mod.rs](server/src/simulator/mod.rs) — `JoinSet`-based parallel exploration loop; 1–4 concurrent experiment slots via `LabScheduler`; convergence gated on `source: "system"` findings
  - [server/src/lab_scheduler.rs](server/src/lab_scheduler.rs) — Slot pool + instrument contention tracking
  - [server/src/approvals_ui.rs](server/src/approvals_ui.rs) — `GET /approvals` handler; serves `approvals.html` via `include_str!`
  - [server/src/approvals.html](server/src/approvals.html) — Vanilla HTML approval dashboard: auto-refreshes every 5 s, shows pending action cards with Deny/Approve buttons
  - [server/src/ws_sink.rs](server/src/ws_sink.rs) — WebSocket broadcast sink + in-memory EventBuffer (up to 2000 events per type)
  - [server/src/audit_query.rs](server/src/audit_query.rs) — Streaming JSONL query (BufReader line iterator, never loads full file); `/api/audit/raw` streams via `tokio_util::io::ReaderStream`
  - [server/src/discovery.rs](server/src/discovery.rs) — Discovery journal + ISO 17025 record types (`MethodValidation`, `ReferenceMaterial`, `StudyRecord`)
  - [server/src/auth.rs](server/src/auth.rs) — JWT middleware (`require_operator_jwt`), `validate_jwt()` for WebSocket upgrade
  - [server/src/oidc.rs](server/src/oidc.rs) — PKCE flow handlers (`/api/auth/oidc/start`, `/api/auth/oidc/callback`)
  - [server/src/literature.rs](server/src/literature.rs) — PubChem REST proxy (`search_pubchem`, `fetch_protocol_hints`)
  - [server/src/eln.rs](server/src/eln.rs) — ELN export trait + `BenchlingAdapter`
  - [server/src/simulator/protocol_library.rs](server/src/simulator/protocol_library.rs) — Protocol template registry (`beer-lambert-scan-v1`, `ph-titration-v1`)

- **agent_runtime** — Orchestrator, protocol executor, and all safety layers.
  - [agent_runtime/src/orchestrator.rs](agent_runtime/src/orchestrator.rs) — 6-stage validation pipeline (`try_tool_call`), LLM chat loop (`run_experiment`), protocol execution (`run_protocol`)
  - [agent_runtime/src/notifications.rs](agent_runtime/src/notifications.rs) — `NotificationSink` trait + `WebhookNotifier` (Slack/Discord/generic auto-detect)
  - [agent_runtime/src/protocol.rs](agent_runtime/src/protocol.rs) — `Protocol`, `ProtocolRunResult`, `UncertaintyBudget`, `DoeAnovaResult` types; `doe_design_json` field links protocols to DoE run matrices
  - [agent_runtime/src/hardware.rs](agent_runtime/src/hardware.rs) — SiLA 2 gRPC client pool: 6 instruments, 12 methods, `abort_all()`
  - [agent_runtime/src/sandbox.rs](agent_runtime/src/sandbox.rs) — Path/command allowlist
  - [agent_runtime/src/capabilities.rs](agent_runtime/src/capabilities.rs) — Numeric parameter bounds
  - [agent_runtime/src/approvals.rs](agent_runtime/src/approvals.rs) — Two-person Ed25519 approval records
  - [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs) — Hash-chained JSONL audit log, per-event Ed25519 signatures, log rotation (100 MB / daily), Rekor checkpointing

- **proof_artifacts** — Proof manifest schema and runtime policy engine.
  - [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs) — `RuntimePolicyEngine`: maps tool actions → risk classes → required artifacts
  - [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs) — Ed25519 manifest signing/verification
  - Manifest bypass only available via compile-time `--features unsafe-bypass`, not an env var

- **vessel_physics** — Formally verified vessel physics with PyO3 Python bindings.
  - Proofs: [verus_verified/vessel_registry.rs](verus_verified/vessel_registry.rs) — 11 theorems verified, 0 errors
  - Protocol proofs: [verus_verified/protocol_safety.rs](verus_verified/protocol_safety.rs) — 13 theorems verified, 0 errors

- **scientific_compute** — Pure-Rust numerics: OLS regression, Hill equation fitting, Michaelis-Menten kinetics, Welch t-test, AIC model selection, DoE generators (full-factorial, central-composite, Latin hypercube), ANOVA, linear regression, GUM uncertainty propagation.

- **zk_audit** — ZK proof for audit chain confidentiality. Proves event count, violation count, and chain validity without revealing log contents. Complements (does not replace) Rekor timestamping. Configured via `ZkUseCase`: `confidential_regulatory` or `confidential_audit`.

### 1.2 The 6-Stage Validation Pipeline

When the LLM returns a tool call, `Orchestrator::try_tool_call()` extracts `reasoning_text` from the LLM response and executes these stages in order:

| Stage | Name | What it checks |
| --- | --- | --- |
| 0 | **Sandbox** | Tool name in allowlist |
| 0.1 | **Calibration** | Instrument has valid calibration record; hard-deny for quantitative tools if no record exists; warn for others |
| 0.25 | **Chemical compatibility** | Reagent being added is compatible with vessel contents (GHS/NFPA 704 table); deny on conflict |
| 1 | **Approval** | High-risk actions have valid Ed25519 two-person approval; checks revocation list |
| 2 | **Capability** | Numeric parameters within hardware bounds |
| 3 | **Fail-closed** | High-risk actions without proof policy engine → deny |
| 4 | **Proof policy** | `RuntimePolicyEngine` authorizes based on Verus artifact status |
| 5 | **Dispatch** | Signed audit event emitted, SiLA 2 gRPC called; LabState updated on dispense/aspirate |

Every stage emits a signed, hash-chained audit entry with `reasoning_text` attached. A rejection at any stage stops the pipeline.

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
```

---

## 2) Authentication

### 2.1 Ed25519 JWT (automated pipelines)

Operators generate a signed JWT with the `tokengen` binary:

```bash
# Generate a token for an operator
AXIOMLAB_OPERATOR_SIGNING_KEY=<base64-ed25519-privkey> \
  cargo run -p agent_runtime --bin tokengen -- \
    --operator-id alice --role operator --ttl-hours 24
```

Include the token in requests:

```bash
curl -X POST http://localhost:3000/api/emergency-stop \
  -H "Authorization: Bearer <token>"
```

JWT claims: `{ sub: operator_id, role: "operator"|"pi"|"machine", exp, iat, iss: "axiomlab-ed25519" }`.

Trusted public keys are registered via `AXIOMLAB_TRUSTED_KEYS` (colon-separated base64 keys).

### 2.2 OIDC / SSO (human operators)

Browser-based login via institutional SSO (Google, Okta, etc.):

1. Visit `/api/auth/oidc/start` → redirected to IdP
2. After login, IdP redirects to `/api/auth/oidc/callback`
3. AxiomLab issues an internal JWT identical in format to Ed25519 JWTs (`iss: "axiomlab-oidc"`)
4. The same `require_operator_jwt` middleware handles both

OIDC configuration env vars:

```sh
AXIOMLAB_OIDC_ISSUER_URL      # e.g. https://accounts.google.com
AXIOMLAB_OIDC_CLIENT_ID
AXIOMLAB_OIDC_CLIENT_SECRET
AXIOMLAB_OIDC_REDIRECT_URI
AXIOMLAB_OIDC_GROUPS_CLAIM    # maps IdP groups to operator roles
```

### 2.3 Protected vs. Public Routes

All `POST`/`DELETE`/`PUT` routes require a valid JWT. Read-only GET routes are public (dashboard embedding). WebSocket (`/ws`) requires JWT by default when `AXIOMLAB_WS_AUTH ≠ 0`; send the token as `?token=<jwt>` or `Sec-WebSocket-Protocol: <token>`.

---

## 3) Safety Features

### 3.1 Chemical Compatibility (Stage 0.25)

Before any `dispense` or `aspirate` operation, the pipeline looks up the reagent being added against the contents already in the target vessel. The check uses a bundled static table (`agent_runtime/src/chemistry_table.json`) covering common GHS/NFPA 704 incompatibilities (acids/bases, oxidizers/reducers, water-reactive compounds).

On a conflict: the action is **denied** and a `chemical_compatibility_violation` audit event is emitted. This stage is a no-op if `LabState` is not initialized.

### 3.2 Calibration Enforcement (Stage 0.1)

Quantitative reading tools (`read_ph`, `read_absorbance`) are **hard-denied** if no calibration record exists for the target instrument. Expired calibration (past `valid_until_secs`) emits a `calibration_warning` audit event and allows continuation (operators may have extended validity periods).

Check calibration status:

```bash
curl http://localhost:3000/api/lab/calibration-status
```

### 3.3 Emergency Stop

```bash
curl -X POST http://localhost:3000/api/emergency-stop \
  -H "Authorization: Bearer <token>"
```

This:

1. Sets `running = false` (stops the exploration loop)
2. Calls `SiLA2Clients::abort_all()` — sends SiLA 2 `Abort` gRPC to all 6 instruments concurrently
3. Emits an `emergency_stop` audit event with operator identity
4. Returns `{ "status": "stopped", "instrument_results": [...] }` with per-instrument results

### 3.4 Notifications / Alerts

When `AXIOMLAB_ALERT_WEBHOOK_URL` is set, `WebhookNotifier` fires on:

- Experiment failure
- Emergency stop triggered
- Approval timeout
- Audit chain invalid
- Calibration expired
- Rekor anchor failed

The payload is auto-formatted for Slack (if URL contains `hooks.slack.com`), Discord (if `discord.com/api/webhooks`), or generic JSON otherwise.

### 3.5 Approval UI

When a high-risk action requires two-person approval, open the built-in approval dashboard in any browser:

```text
http://localhost:3000/approvals
```

The page:

- Auto-refreshes every 5 seconds
- Lists each pending action with tool name, risk class, hypothesis, parameters, and age
- **Deny** — one click, no credentials required (the approval bundle itself carries cryptographic authority)
- **Approve…** — opens a dialog to paste the signed approval bundle JSON produced by `approvalctl sign --pending-id <id>`

No JWT required for the page itself. The approval bundle carries the cryptographic authorization.

```bash
# View pending approvals (JSON)
curl http://localhost:3000/api/approvals/pending

# Deny without the UI
curl -X POST http://localhost:3000/api/approvals/submit \
  -H "Content-Type: application/json" \
  -d '{"pending_id": "<id>", "bundle": null}'
```

---

## 4) ISO 17025 Records

The discovery journal (`DiscoveryJournal`) includes ISO 17025 record types. Mutations are available via the API; all writes are stored in memory and serialized to the JSONL journal.

### 4.1 Method Validations

```bash
# List all method validations
curl http://localhost:3000/api/methods

# Register a new validation record (PI JWT required)
curl -X POST http://localhost:3000/api/methods \
  -H "Authorization: Bearer <pi-token>" \
  -H "Content-Type: application/json" \
  -d '{ "method_name": "pH titration", "analyte": "HCl", ... }'

# Get a specific record
curl http://localhost:3000/api/methods/<id>
```

### 4.2 Reference Materials

```bash
# List CRMs
curl http://localhost:3000/api/lab/reference-materials

# Register a CRM (PI JWT required)
curl -X POST http://localhost:3000/api/lab/reference-materials \
  -H "Authorization: Bearer <pi-token>" \
  -d '{ "name": "NIST SRM 2390", "supplier": "NIST", ... }'
```

### 4.3 Studies and QA Sign-Off

```bash
# Create a study
curl -X POST http://localhost:3000/api/studies \
  -H "Authorization: Bearer <pi-token>" \
  -d '{ "title": "Caffeine solubility", "study_director_id": "alice" }'

# Pre-register a protocol to a study (before execution)
curl -X POST http://localhost:3000/api/studies/<id>/protocols \
  -H "Authorization: Bearer <pi-token>" \
  -d '{ "protocol_id": "beer-lambert-scan-v1" }'

# QA sign-off (reviewer must differ from study_director_id)
curl -X POST http://localhost:3000/api/studies/<id>/qa-review \
  -H "Authorization: Bearer <qa-token>" \
  -d '{ "reviewer_id": "bob" }'
```

QA sign-off computes `SHA-256(canonical_json(study_record))` and stores it as `qa_sign_off_hash`. This hash is included in the next audit log entry, making the QA review part of the tamper-evident chain.

---

## 5) API Surface

### 5.1 Public (no auth required)

| Method | Path | Description |
| --- | --- | --- |
| GET | `/approvals` | Browser approval UI — view, approve, or deny pending high-risk actions |
| GET | `/api/approvals/pending` | Pending approvals as JSON |
| POST | `/api/approvals/submit` | Submit approval bundle or deny (`bundle: null`) |
| GET | `/api/status` | Server status + active slot count |
| GET | `/api/history` | Recent experiment results |
| GET | `/api/journal` | Full discovery journal |
| GET | `/api/journal/findings` | Findings only |
| GET | `/api/journal/hypotheses` | Hypotheses only |
| GET | `/api/audit` | Filtered audit log (`?action=`, `?decision=`, `?since=`, `?limit=`) |
| GET | `/api/audit/verify` | Hash-chain integrity check |
| GET | `/api/audit/raw` | Full audit log stream (zero-copy JSONL) |
| GET | `/api/audit/zk-status` | Last ZK proof status |
| GET | `/api/methods` | Method validation records |
| GET | `/api/methods/:id` | Single method validation |
| GET | `/api/lab/reference-materials` | Reference materials |
| GET | `/api/lab/calibration-status` | Per-instrument calibration validity |
| GET | `/api/literature/search?q=` | PubChem compound lookup |
| GET | `/api/auth/oidc/start` | Initiate OIDC PKCE flow |
| GET | `/api/auth/oidc/callback` | OIDC callback (issues JWT) |
| WS | `/ws?token=` | Event stream (JWT required when `AXIOMLAB_WS_AUTH ≠ 0`) |

### 5.2 Protected (JWT required)

| Method | Path | Role | Description |
| --- | --- | --- | --- |
| POST | `/api/emergency-stop` | operator | Halt all instruments |
| POST | `/api/studies` | pi | Create study |
| GET | `/api/studies/:id` | any | Get study |
| POST | `/api/studies/:id/protocols` | pi | Pre-register protocol |
| POST | `/api/studies/:id/qa-review` | pi | QA sign-off |
| POST | `/api/methods` | pi | Create method validation |
| POST | `/api/lab/reference-materials` | pi | Register CRM |
| POST | `/api/export/benchling/:study_id` | pi | Export study to Benchling |
| POST | `/api/auth/logout` | any | Invalidate session |

---

## 6) Multi-Slot Experiment Scheduling

By default, one experiment runs at a time. Increase parallelism:

```bash
AXIOMLAB_EXPERIMENT_SLOTS=2 cargo run -p axiomlab-server
```

Valid range: 1–4 (values outside are clamped). Each slot gets its own `Orchestrator` instance. Instrument contention is tracked at the `LabScheduler` level — a slot cannot start if required instruments are already in use by another slot.

Check active slots:

```bash
curl http://localhost:3000/api/status
# {"slot_count": 2, "active_experiments": [...], ...}
```

---

## 7) Runbook

### 7.1 Local Development

```bash
# Build all crates
cargo build

# Build PyO3 vessel_physics module (required for Python SiLA 2 server)
pip install maturin
maturin develop --manifest-path vessel_physics/Cargo.toml

# Run pure-Rust unit tests
cargo test -p agent_runtime
cargo test -p server
cargo test -p scientific_compute
cargo test -p zk_audit

# Start SiLA 2 mock server
cd sila_mock && python3 -m axiomlab_mock --insecure -p 50052 &

# Run integration tests
cargo test -p agent_runtime --test vessel_simulation_e2e -- --ignored --test-threads=1
cargo test -p agent_runtime --test sila2_e2e -- --ignored --test-threads=1
```

### 7.2 Verus Proof Workflow

```bash
# Install Verus (macOS ARM64, Linux x86-64, Linux ARM64)
# Download: https://github.com/verus-lang/verus/releases
# Extract to ~/verus/

# Verify all proofs
~/verus/verus verus_verified/vessel_registry.rs
# Expected: 11 verified, 0 errors

~/verus/verus verus_verified/protocol_safety.rs
# Expected: 13 verified, 0 errors

~/verus/verus verus_verified/lab_safety.rs
# Expected: 6 verified, 0 errors

# Regenerate manifest
python3 vessel_physics/generate_manifest.py
```

### 7.3 Release Gate

```bash
./scripts/proof_release_gate.sh
```

Runs: build, manifest generation, signing, policy enforcement, sandbox/capability tests, audit chain verification, compliance bundle export.

### 7.4 Audit Log

**Location:** `$AXIOMLAB_DATA_DIR/audit/runtime_audit.jsonl` (default: `.artifacts/audit/runtime_audit.jsonl`)

**Rotation:** Files rotate at 100 MB or daily. Rotated files are archived as `runtime_audit_YYYY-MM-DD[_N].jsonl`.

**Session continuity:** `session_start` entry written on every startup containing session UUID, Ed25519 public key, and git commit. Chains to previous file's last `entry_hash` via `prev_hash`.

**Rekor checkpoints:** Every 15 minutes (when `AXIOMLAB_AUDIT_SIGNING_KEY` is set), chain tip is signed and submitted to Sigstore Rekor. UUID + integrated timestamp written back as `rekor_checkpoint`.

**Streaming raw log (zero-copy):**

```bash
curl http://localhost:3000/api/audit/raw > backup.jsonl
```

**Verify chain:**

```bash
cargo run -p agent_runtime --bin auditctl -- verify \
  --path .artifacts/audit/runtime_audit.jsonl
```

**Filter queries:**

```bash
# All calibration events
curl "http://localhost:3000/api/audit?action=calibration"

# Last 50 denied actions since a Unix timestamp
curl "http://localhost:3000/api/audit?decision=deny&since=1700000000&limit=50"

# Hash-chain integrity
curl "http://localhost:3000/api/audit/verify"

# ZK proof status
curl "http://localhost:3000/api/audit/zk-status"
```

### 7.5 Verify Signed Manifest

```bash
cargo run -p proof_artifacts --bin proofctl -- verify \
  --signed-manifest .artifacts/proof/manifest.signed.json \
  --public-key .artifacts/proof/manifest_signing_key.public.b64
```

### 7.6 Verify Rekor Anchor

```bash
# Via rekor-cli
rekor-cli verify --uuid <uuid> --artifact-hash <sha256_hex>

# Via REST API
curl https://rekor.sigstore.dev/api/v1/log/entries/<uuid>
```

### 7.7 ELN Export (Benchling)

```bash
# Export a study to Benchling
curl -X POST http://localhost:3000/api/export/benchling/<study_id> \
  -H "Authorization: Bearer <pi-jwt>"
# Returns: {"benchling_url": "https://myorg.benchling.com/..."}
```

Requires `AXIOMLAB_BENCHLING_TOKEN`, `AXIOMLAB_BENCHLING_TENANT`, `AXIOMLAB_BENCHLING_PROJECT_ID`. Returns 503 if unconfigured.

---

## 8) Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `SILA2_ENDPOINT` | `http://127.0.0.1:50052` | SiLA 2 gRPC server address |
| `AXIOMLAB_LLM_ENDPOINT` | — | Ollama or OpenAI-compatible API endpoint |
| `AXIOMLAB_LLM_MODEL` | `qwen2.5-coder:7b` | LLM model name |
| `PORT` | `3000` | HTTP server port |
| `AXIOMLAB_DATA_DIR` | `.artifacts` | Root directory for runtime data (audit log, journal) |
| `AXIOMLAB_AUDIT_LOG` | `$DATA_DIR/audit/runtime_audit.jsonl` | Full path override for audit log |
| `AXIOMLAB_AUDIT_SIGNING_KEY` | — | Base64 Ed25519 private key for per-event signatures and Rekor. Unsigned if unset. |
| `AXIOMLAB_GIT_COMMIT` | `dev` | Git SHA embedded in `session_start` and proof manifest |
| `AXIOMLAB_TRUSTED_KEYS` | — | Colon-separated base64 Ed25519 public keys for JWT verification |
| `AXIOMLAB_OPERATOR_SIGNING_KEY` | — | Ed25519 private key for `tokengen` JWT signing |
| `AXIOMLAB_WS_AUTH` | `1` | Set to `0` to disable JWT check on WebSocket upgrade |
| `AXIOMLAB_EXPERIMENT_SLOTS` | `1` | Parallel experiment slots (1–4) |
| `AXIOMLAB_ALERT_WEBHOOK_URL` | — | Slack/Discord/generic webhook for failure alerts |
| `AXIOMLAB_BASE_RPC_URL` | — | Base L2 RPC endpoint for ZK proof anchoring |
| `AXIOMLAB_BASE_CONTRACT_ADDR` | — | `AuditVerifier` contract address on Base |
| `AXIOMLAB_BASE_WALLET_KEY` | — | Hex-encoded private key for Base transactions |
| `AXIOMLAB_ZK_USE_CASE` | `confidential_audit` | `confidential_regulatory` or `confidential_audit` |
| `AXIOMLAB_BENCHLING_TOKEN` | — | Benchling API token |
| `AXIOMLAB_BENCHLING_TENANT` | — | Benchling tenant (e.g. `myorg.benchling.com`) |
| `AXIOMLAB_BENCHLING_PROJECT_ID` | — | Benchling project for new entries |
| `AXIOMLAB_OIDC_ISSUER_URL` | — | OIDC issuer (e.g. `https://accounts.google.com`) |
| `AXIOMLAB_OIDC_CLIENT_ID` | — | OIDC client ID |
| `AXIOMLAB_OIDC_CLIENT_SECRET` | — | OIDC client secret |
| `AXIOMLAB_OIDC_REDIRECT_URI` | — | OIDC redirect URI |
| `AXIOMLAB_OIDC_GROUPS_CLAIM` | — | OIDC claim for role mapping |

---

## 9) Security Findings and Risk Priorities

### 9.1 High: Policy construction trust boundary

- Code: [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs)
- Issue: `RuntimePolicyEngine` can be constructed with `trusted` flag without prior signature verification. In production, `simulator.rs` calls `mark_signature_verified()` only when manifest hash matches a compile-time constant — not a runtime cryptographic check.
- Mitigation: Use `proofctl verify` with actual Ed25519 keys before deployment.

### 9.2 Low: Audit signing key file is local-only

- Code: [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs) — `FileBackedSigner::load_or_create()`
- Status: **Resolved for single-node deployments.** `audit_signer_from_env()` loads in priority order: `AXIOMLAB_AUDIT_SIGNING_KEY` env var → `AXIOMLAB_AUDIT_SIGNING_KEY_PATH` file → auto-generate at `~/.config/axiomlab/audit_signing.key`. The key survives restarts.
- Remaining risk: Key is on local disk. A complete chain rewrite from the same host with the same persisted key passes local checks. Rekor anchors remain the external witness. HSM or KMS custody is needed for production.

### 9.3 Medium: Hardware is simulated

- Code: [sila_mock/](sila_mock/)
- Issue: All SiLA 2 responses are simulated. Validation pipeline is tested against the mock, not real instruments.
- Mitigation: Hardware-in-the-loop tests behind a `--feature hardware` gate when connecting real instruments.

### 9.4 Low: proof_synthesizer requires external toolchain

- Proof repair loop requires Verus binary + LLM. Not wired into CI gate. All production proofs are pre-generated and committed.

### 9.5 Low: Key lifecycle is external

- Ed25519 key generation, rotation, revocation, and storage are left to the operator. Use HSM/KMS for production.

---

## 10) Operator Checklist

Before running high-risk actions:

1. Verify signed manifest (`proofctl verify`).
2. Verify CI gate pass for required artifacts (`ArtifactStatus::Passed`).
3. Verify approval bundle for Actuation or Destructive actions.
4. Verify audit chain integrity after execution (`/api/audit/verify`).
5. Confirm Rekor anchor UUID logged for any protocol conclusion.
6. Set `AXIOMLAB_AUDIT_SIGNING_KEY` to a persistent key (events are unsigned and Rekor disabled without it).
7. Point `AXIOMLAB_DATA_DIR` at a durable mount for audit log persistence across restarts.
8. Set `AXIOMLAB_ALERT_WEBHOOK_URL` so failures surface without dashboard monitoring.
9. Pre-register protocols to a study record before execution for ISO 17025 traceability.
10. Ensure calibration records exist for all quantitative instruments before starting protocols.

---

## 11) Suggested Next Hardening Tasks

1. Restrict trusted policy-engine constructor usage to test-only contexts.
2. Replace hardware simulation with injected production driver traits (trait-based `SensorDriver` injection behind `--feature hardware`).
3. Extend integration tests to enforce signed-manifest-only authorization path; add rejection tests for all 6 pipeline stages.
4. Make approval sidecar write + dispatch atomic (write sidecar → dispatch → delete on success).
5. Add CI gate that verifies committed `vessel_physics_manifest.json` was generated from current `verus_verified/*.rs` sources.
6. Add Rekor submission retry queue for network outages at conclusion time.
7. Validate string tool parameters (`pump_id`, `sensor_id`, `vessel_id`) against an allowed set at the capability stage.
8. Add external audit mirror: periodically push chain-tip hashes to a Gist or orphan git branch to survive local disk failure.
9. Implement full embedding-based RAG (vector DB + paper chunking) to replace the PubChem keyword stub in `literature.rs`.
10. Migrate audit signing key to HSM or KMS for production; current `FileBackedSigner` only survives single-node restarts.
11. Add `nominal_ph` values to the default reagent catalog so pH simulation works out-of-the-box without manual registration.
12. Extend `run_doe_anova` to support multi-factor grouping (currently only groups by the first factor's low/high bracket).
