# AxiomLab Operator Guide

Operational reference for running, testing, and understanding the system. This document describes what the system **actually does** — not aspirations.

## 1) System Architecture

AxiomLab is a Rust workspace (9 crates) + a Python SiLA 2 server + a React web dashboard. It is a **safe execution platform for autonomous agentic lab operation** — not an AI discovery engine. The core loop: operators push protocol directives into a priority queue → the execution loop drains the queue → an LLM executes each directive → a 6-stage validation pipeline checks every proposed action → validated actions execute over SiLA 2 gRPC → results are recorded in the operation log and audit chain.

### 1.1 Crate Roles

**Production path (server + agent_runtime + proof_artifacts + vessel_physics):**

- **server** — Axum HTTP server with WebSocket event streaming, SQLite-indexed audit log, multi-slot experiment scheduler, protocol queue, and the continuous execution loop that drives the agent.
  - [server/src/main.rs](server/src/main.rs) — HTTP router (see §6 for full API surface), JWT middleware, OIDC handlers, experiment scheduler initialization
  - [server/src/simulator/mod.rs](server/src/simulator/mod.rs) — `JoinSet`-based parallel execution loop; drains protocol queue (priority → FIFO), falls back to commissioning agenda; 1–4 concurrent experiment slots via `LabScheduler`; convergence gated on `source: "system"` findings
  - [server/src/protocol_queue.rs](server/src/protocol_queue.rs) — Persistent, priority-ordered queue of operator protocol directives; survives restarts; trimmed to 50 completed/failed items
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
  - [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs) — Hash-chained JSONL audit log, per-event Ed25519 signatures, log rotation (100 MB / daily)

- **proof_artifacts** — Proof manifest schema and runtime policy engine.
  - [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs) — `RuntimePolicyEngine`: maps tool actions → risk classes → required artifacts
  - [proof_artifacts/src/signature.rs](proof_artifacts/src/signature.rs) — Ed25519 manifest signing/verification
  - Manifest bypass only available via compile-time `--features unsafe-bypass`, not an env var

- **vessel_physics** — Formally verified vessel physics with PyO3 Python bindings.
  - Proofs: [verus_verified/vessel_registry.rs](verus_verified/vessel_registry.rs) — 11 theorems verified, 0 errors
  - Protocol proofs: [verus_verified/protocol_safety.rs](verus_verified/protocol_safety.rs) — 13 theorems verified, 0 errors

- **scientific_compute** — Pure-Rust numerics: OLS regression, Hill equation fitting, Michaelis-Menten kinetics, Welch t-test, AIC model selection, DoE generators (full-factorial, central-composite, Latin hypercube), ANOVA, linear regression, GUM uncertainty propagation.

- **zk_audit** — *(planned)* ZK proof layer for audit chain confidentiality. Not yet implemented.

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

1. Sets `running = false` (stops the execution loop)
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

## 4) Protocol Queue

The protocol queue is the **primary interface** between operators and the execution loop. Push a natural-language directive; the loop picks it up in priority order and executes it as the next experiment mandate.

### 4.1 Queue Priority Model

Items are sorted: **highest `priority` first**, FIFO within the same priority. Range: 0 (normal) to 255 (urgent).

When the queue is empty the loop falls back to the built-in **commissioning agenda** — a fixed series of instrument characterisation runs (pH titration, Beer-Lambert extended range, incubator temperature uniformity, pH-absorbance coupling, arm workspace boundary). These fire automatically and never block operator-pushed work.

### 4.2 Using the Queue

```bash
# Push a directive (no JWT required — reading queue is public)
curl -X POST http://localhost:3000/api/queue \
  -H "Content-Type: application/json" \
  -d '{
    "statement": "Characterise the pH meter linearity from pH 4 to pH 10 using buffer standards. Report slope ± std-error and R².",
    "priority": 100
  }'
# Returns: {"id": "<uuid>"}

# List all items (pending + recent history)
curl http://localhost:3000/api/queue

# Remove an item by ID
curl -X DELETE http://localhost:3000/api/queue/<id>
```

Write `statement` as a precise lab instruction: name the instrument, the procedure, and the quantitative outcome expected. The statement becomes the top-level directive in the LLM execution mandate verbatim.

### 4.3 Item Lifecycle

```
Pending → Running (loop picks it up)
        → Completed (converged with quantitative finding)
        → Failed (loop exhausted iterations without convergence)
```

Completed and failed items are retained as history (trimmed to the 50 most recent). The experiment ID assigned when running is stored in `experiment_id`; the outcome summary is in `result_summary`.

### 4.4 JWT Protection for Queue Writes

When `AXIOMLAB_JWT_SECRET` or `AXIOMLAB_TRUSTED_KEYS` is set, `POST /api/queue` and `DELETE /api/queue/:id` require a valid operator JWT in the `Authorization: Bearer` header. `GET /api/queue` is always unauthenticated (read-only). See Section 7 for token generation.

### 4.5 Commissioning Agenda

`GET /api/agenda` returns the five built-in commissioning procedures and their current status:

```bash
curl http://localhost:3000/api/agenda
# Returns:
# {
#   "items": [
#     { "key": "ph_linearity",       "statement": "...", "status": "pending" },
#     { "key": "beer_lambert",        "statement": "...", "status": "completed" },
#     { "key": "temperature_profile", "statement": "...", "status": "pending" },
#     { "key": "ph_absorbance",       "statement": "...", "status": "pending" },
#     { "key": "arm_boundary",        "statement": "...", "status": "pending" }
#   ],
#   "completed_count": 1,
#   "total_count": 5
# }
```

Status values: `pending` → `proposed` → `testing` → `completed` | `rejected`.

When all five items reach `completed`, the `/api/status` response sets `"agenda_complete": true` and the dashboard header shows a **COMMISSIONED** badge.

### 4.6 Live Finding Notifications

When `analyze_series` records a scientific finding with R² ≥ 0.80 and ≥ 5 data points, the server broadcasts a `finding_recorded` WebSocket event. The visualizer displays a green toast overlay for 6 seconds showing the model type and R² value.

---

## 5) ISO 17025 Records

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

## 6) API Surface

### 6.1 Public (no auth required)

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
| GET | `/api/methods` | Method validation records |
| GET | `/api/methods/:id` | Single method validation |
| GET | `/api/lab/reference-materials` | Reference materials |
| GET | `/api/lab/calibration-status` | Per-instrument calibration validity |
| GET | `/api/queue` | Protocol queue — all items (pending + history) |
| GET | `/api/agenda` | Commissioning agenda with live completion status |
| GET | `/api/literature/search?q=` | PubChem compound lookup |
| GET | `/api/auth/oidc/start` | Initiate OIDC PKCE flow |
| GET | `/api/auth/oidc/callback` | OIDC callback (issues JWT) |
| WS | `/ws?token=` | Event stream (JWT required when `AXIOMLAB_WS_AUTH ≠ 0`) |

### 6.2 Protected (JWT required)

| Method | Path | Role | Description |
| --- | --- | --- | --- |
| POST | `/api/emergency-stop` | operator | Halt all instruments |
| POST | `/api/queue` | operator | Push a protocol directive (priority 0–255) |
| DELETE | `/api/queue/:id` | operator | Remove a queued item |
| POST | `/api/studies` | pi | Create study |
| GET | `/api/studies/:id` | any | Get study |
| POST | `/api/studies/:id/protocols` | pi | Pre-register protocol |
| POST | `/api/studies/:id/qa-review` | pi | QA sign-off |
| POST | `/api/methods` | pi | Create method validation |
| POST | `/api/lab/reference-materials` | pi | Register CRM |
| POST | `/api/export/benchling/:study_id` | pi | Export study to Benchling |
| POST | `/api/auth/logout` | any | Invalidate session |

---

## 7) Multi-Slot Experiment Scheduling

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

## 8) Runbook

### 8.1 Quickstart (Simulator Mode)

The fastest path to a running demo requires only Rust and Node:

```bash
# 1. Build all crates
cargo build

# 2. Run the server (simulator mode — no real hardware)
cargo run -p axiomlab-server

# 3. In a second terminal, start the visualizer
cd visualizer && npm install && npm run dev
# Open http://localhost:5173
```

The server starts in simulator mode automatically when `SILA2_ENDPOINT` is unreachable. The commissioning agenda runs immediately; the first finding typically appears within 60–90 seconds.

```bash
# Run the full test suite
cargo test --workspace --lib          # unit tests (134+)
cargo test -p axiomlab-server         # HTTP integration tests (38)
cargo test -p agent_runtime --test pipeline_rejection  # safety gate tests

# Run the release gate (signs manifest, verifies chain, exports replay bundle)
./scripts/proof_release_gate.sh
```

### 8.1.1 Optional: SiLA 2 Hardware Mode

```bash
# Build PyO3 vessel_physics module (required for Python SiLA 2 server)
pip install maturin
maturin develop --manifest-path vessel_physics/Cargo.toml

# Start SiLA 2 mock server
cd sila_sim && python3 -m axiomlab_sim --insecure -p 50052

# In another terminal, start server with hardware mode
SILA2_ENDPOINT=http://127.0.0.1:50052 cargo run -p axiomlab-server
```

### 8.2 Verus Proof Workflow

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

### 8.3 Release Gate

```bash
./scripts/proof_release_gate.sh
```

Runs: build, manifest generation, signing, policy enforcement, sandbox/capability tests, audit chain verification, compliance bundle export.

### 8.4 Audit Log

**Location:** `$AXIOMLAB_DATA_DIR/audit/runtime_audit.jsonl` (default: `.artifacts/audit/runtime_audit.jsonl`)

**Rotation:** Files rotate at 100 MB or daily. Rotated files are archived as `runtime_audit_YYYY-MM-DD[_N].jsonl`.

**Session continuity:** `session_start` entry written on every startup containing session UUID, Ed25519 public key, and git commit. Chains to previous file's last `entry_hash` via `prev_hash`.

**Rekor checkpoints:** After each protocol conclusion, the chain-tip hash is submitted to Sigstore Rekor as a `hashedrekord`. The returned UUID is written back into the audit log as a `rekor_checkpoint` entry. Enabled when `AXIOMLAB_REKOR_ENABLED=1` (opt-in; no-op without it). Override the endpoint with `AXIOMLAB_REKOR_URL`.

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
```

### 8.5 Verify Signed Manifest

```bash
cargo run -p proof_artifacts --bin proofctl -- verify \
  --signed-manifest .artifacts/proof/manifest.signed.json \
  --public-key .artifacts/proof/manifest_signing_key.public.b64
```

### 8.6 Verify Rekor Anchor

After a protocol run with `AXIOMLAB_REKOR_ENABLED=1`, the audit log will contain a `rekor_checkpoint` entry with a `rekor_uuid` field. Verify against Sigstore:

```bash
# Get the Rekor UUID from the audit log
REKOR_UUID=$(jq -r 'select(.action == "rekor_checkpoint") | .rekor_uuid' \
  .artifacts/audit/runtime_audit.jsonl | tail -1)

# Verify it exists in the public transparency log
curl "https://rekor.sigstore.dev/api/v1/log/entries/${REKOR_UUID}"
```

The response contains the integrated timestamp and the artifact hash. Confirm the artifact hash matches your chain-tip: `sha256sum` of the chain-tip `entry_hash` hex bytes.

### 8.7 ELN Export (Benchling)

```bash
# Export a study to Benchling
curl -X POST http://localhost:3000/api/export/benchling/<study_id> \
  -H "Authorization: Bearer <pi-jwt>"
# Returns: {"benchling_url": "https://myorg.benchling.com/..."}
```

Requires `AXIOMLAB_BENCHLING_TOKEN`, `AXIOMLAB_BENCHLING_TENANT`, `AXIOMLAB_BENCHLING_PROJECT_ID`. Returns 503 if unconfigured.

---

## 9) Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `SILA2_ENDPOINT` | `http://127.0.0.1:50052` | SiLA 2 gRPC server address |
| `AXIOMLAB_LLM_ENDPOINT` | — | Ollama or OpenAI-compatible API endpoint |
| `AXIOMLAB_LLM_MODEL` | `qwen2.5-coder:7b` | LLM model name |
| `PORT` | `3000` | HTTP server port |
| `AXIOMLAB_DATA_DIR` | `.artifacts` | Root directory for runtime data (audit log, journal) |
| `AXIOMLAB_AUDIT_LOG` | `$DATA_DIR/audit/runtime_audit.jsonl` | Full path override for audit log |
| `AXIOMLAB_AUDIT_SIGNING_KEY` | — | Base64 Ed25519 private key for per-event audit signatures. Unsigned if unset. |
| `AXIOMLAB_GIT_COMMIT` | `dev` | Git SHA embedded in `session_start` and proof manifest |
| `AXIOMLAB_TRUSTED_KEYS` | — | Colon-separated base64 Ed25519 public keys for JWT verification |
| `AXIOMLAB_OPERATOR_SIGNING_KEY` | — | Ed25519 private key for `tokengen` JWT signing |
| `AXIOMLAB_WS_AUTH` | `1` | Set to `0` to disable JWT check on WebSocket upgrade |
| `AXIOMLAB_EXPERIMENT_SLOTS` | `1` | Parallel experiment slots (1–4) |
| `AXIOMLAB_ALERT_WEBHOOK_URL` | — | Slack/Discord/generic webhook for failure alerts |
| `AXIOMLAB_REKOR_ENABLED` | — | Set to `1` to enable Sigstore Rekor chain-tip submission after each protocol conclusion |
| `AXIOMLAB_REKOR_URL` | `https://rekor.sigstore.dev/api/v1/log/entries` | Rekor endpoint override (air-gapped Rekor instances) |
| `AXIOMLAB_BENCHLING_TOKEN` | — | Benchling API token |
| `AXIOMLAB_BENCHLING_TENANT` | — | Benchling tenant (e.g. `myorg.benchling.com`) |
| `AXIOMLAB_BENCHLING_PROJECT_ID` | — | Benchling project for new entries |
| `AXIOMLAB_OIDC_ISSUER_URL` | — | OIDC issuer (e.g. `https://accounts.google.com`) |
| `AXIOMLAB_OIDC_CLIENT_ID` | — | OIDC client ID |
| `AXIOMLAB_OIDC_CLIENT_SECRET` | — | OIDC client secret |
| `AXIOMLAB_OIDC_REDIRECT_URI` | — | OIDC redirect URI |
| `AXIOMLAB_OIDC_GROUPS_CLAIM` | — | OIDC claim for role mapping |

---

## 10) Security Findings and Risk Priorities

### 10.1 High: Policy construction trust boundary

- Code: [proof_artifacts/src/policy.rs](proof_artifacts/src/policy.rs)
- Issue: `RuntimePolicyEngine` can be constructed with `trusted` flag without prior signature verification. In production, `simulator.rs` calls `mark_signature_verified()` only when manifest hash matches a compile-time constant — not a runtime cryptographic check.
- Mitigation: Use `proofctl verify` with actual Ed25519 keys before deployment.

### 10.2 Low: Audit signing key file is local-only

- Code: [agent_runtime/src/audit.rs](agent_runtime/src/audit.rs) — `FileBackedSigner::load_or_create()`
- Status: **Resolved for single-node deployments.** `audit_signer_from_env()` loads in priority order: `AXIOMLAB_AUDIT_SIGNING_KEY` env var → `AXIOMLAB_AUDIT_SIGNING_KEY_PATH` file → auto-generate at `~/.config/axiomlab/audit_signing.key`. The key survives restarts.
- Remaining risk: Key is on local disk. A complete chain rewrite from the same host with the same persisted key passes local checks. Enable `AXIOMLAB_REKOR_ENABLED=1` for an external timestamp anchor (Sigstore Rekor), or use `AXIOMLAB_KMS_KEY_ID` with the `kms` feature for HSM-backed key custody.

### 10.3 Medium: Hardware is simulated

- Code: [sila_sim/](sila_sim/)
- Issue: All SiLA 2 responses are simulated. Validation pipeline is tested against the mock, not real instruments.
- Mitigation: Hardware-in-the-loop tests behind a `--feature hardware` gate when connecting real instruments.

### 10.4 Low: proof_synthesizer requires external toolchain

- Proof repair loop requires Verus binary + LLM. Not wired into CI gate. All production proofs are pre-generated and committed.

### 10.5 Low: Key lifecycle is external

- Ed25519 key generation, rotation, revocation, and storage are left to the operator. Use HSM/KMS for production.

---

## 11) Operator Checklist

Before running a session:

1. Verify signed manifest (`proofctl verify`).
2. Verify CI gate pass for required artifacts (`ArtifactStatus::Passed`).
3. Verify approval bundle for Actuation or Destructive actions.
4. Verify audit chain integrity after execution (`/api/audit/verify`).
5. Set `AXIOMLAB_AUDIT_SIGNING_KEY` to a persistent key (events are unsigned without it).
7. Point `AXIOMLAB_DATA_DIR` at a durable mount for audit log persistence across restarts.
8. Set `AXIOMLAB_ALERT_WEBHOOK_URL` so failures surface without dashboard monitoring.
9. Pre-register protocols to a study record before execution for ISO 17025 traceability.
10. Ensure calibration records exist for all quantitative instruments before starting protocols.
11. Direct the execution loop via `POST /api/queue` — natural-language directives with a priority (0–255) are picked up in order before the commissioning agenda fires.
12. Check `/api/status` for `hardware_mode: true` before assuming real instrument data (false = simulator).

---

## 12) Suggested Next Hardening Tasks

1. Restrict trusted policy-engine constructor usage to test-only contexts.
2. Replace hardware simulation with injected production driver traits (trait-based `SensorDriver` injection behind `--feature hardware`).
3. Extend integration tests to enforce signed-manifest-only authorization path; add rejection tests for all 6 pipeline stages.
4. Make approval sidecar write + dispatch atomic (write sidecar → dispatch → delete on success).
5. Add CI gate that verifies committed `vessel_physics_manifest.json` was generated from current `verus_verified/*.rs` sources.
6. Add Rekor retry queue: current Rekor submission is fire-and-forget with a `warn` on failure. Add a persistent retry queue so anchors don't silently drop on network outages.
7. Validate string tool parameters (`pump_id`, `sensor_id`, `vessel_id`) against an allowed set at the capability stage.
8. Add external audit mirror: periodically push chain-tip hashes to a Gist or orphan git branch to survive local disk failure.
9. Migrate audit signing key to AWS KMS for production (`kms` feature flag on `agent_runtime`; `AuditSigner` trait already designed for this). Current `FileBackedSigner` only survives single-node restarts.
10. Add `nominal_ph` values to the default reagent catalog so pH simulation works out-of-the-box without manual registration.
11. Extend `run_doe_anova` to support multi-factor grouping (currently only groups by the first factor's low/high bracket).
12. Add a real SiLA 2 instrument driver shim: `--feature hardware` that swaps the simulator for actual gRPC calls without changing orchestrator code.
