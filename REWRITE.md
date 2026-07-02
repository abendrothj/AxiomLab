# AxiomLab Rewrite Plan

> **Historical implementation record.** The rewrite described here is complete.
> Current operation is documented in `README.md` and `OPERATOR_GUIDE.md`; future
> production work and acceptance criteria live in `ROADMAP.md`. Test counts and
> known gaps below describe the rewrite checkpoint and are not current status.

**Decision:** Full rewrite in-place on `main`.  
**Preserved:** `verus_verified/` (binding Verus spec — 30+ theorems), `verus_proofs/` (build-time bridge), `sila_sim/` (Python mock), `agent_runtime/proto/` (SiLA 2 FDL).  
**Deleted:** Everything else — rebuilt from scratch under the architecture below.

---

## Core Principle

The current codebase is an orchestrator with safety bolted on. The rewrite inverts that: **the pipeline is the product.** The LLM is a proposal generator. Every action it produces flows through an ordered chain of fail-closed gates before anything touches hardware. No gate can be skipped, softened, or logged-and-continued.

```
LLM proposes action
  → CapabilityGate   (Verus-verified hardware bounds, per-param)
  → ChemistryGate    (reagent compatibility table)
  → CalibrationGate  (valid calibration record required for measurement tools)
  → ProofGate        (signed artifact present + Rust predicate called with actual params)
  → ApprovalGate     (operator decision for Actuation risk class, scoped to action+params)
  → ExecuteGate      (SiLA 2 gRPC call)
  → AuditGate        (Ed25519 signed entry + Rekor checkpoint)
```

Every arrow is `Result<_, Rejection>`. First `Err` hard-stops the action.

---

## Workspace Layout

```
axiomlab/
│
├── crates/
│   ├── types/        # Shared domain types only — no logic
│   ├── audit/        # Ed25519 chain + Rekor (default-on)
│   ├── chemistry/    # Compatibility table
│   ├── sila/         # SiLA 2 proto codegen + thin gRPC clients
│   ├── proofs/       # Verus artifact loading + runtime predicate dispatch
│   ├── gate/         # The 7-stage pipeline — the entire safety story
│   └── llm/          # LLM client + thin orchestrator (proposal → pipeline)
│
├── server/           # Axum HTTP server, WebSocket, operator API
├── ui/               # React frontend (rebuilt clean)
│
├── verus_verified/   # Binding Verus spec — the formally-verified safety envelope
├── verus_proofs/     # KEPT AS-IS (build-time bridge)
├── sila_sim/         # KEPT AS-IS
└── REWRITE.md        # This file
```

---

## Crate Specifications

### `crates/types/`

Shared domain types. Zero business logic. No dependencies outside `serde`.

```rust
pub struct Action {
    pub tool:       String,
    pub params:     serde_json::Value,
    pub risk_class: RiskClass,
}

pub struct Rejection {
    pub gate:   &'static str,
    pub reason: String,
    pub action: Action,   // the rejected action, for audit
}

pub enum RiskClass { ReadOnly, LiquidHandling, Actuation, Destructive }

// NOTE (deviation): GateContext lives in `crates/gate/`, not here. It references
// `Chain` (from `crates/audit/`), and `audit` depends on `types` — placing
// GateContext in `types` would form a dependency cycle. `types` holds only the
// pure data below; GateContext is assembled where Chain is in scope.
// struct GateContext { experiment_id: String, iteration: u32,
//                      lab_state: Arc<LabState>, audit_chain: Arc<Chain> }

// Physical quantities — newtype wrappers, not raw f64
pub struct VolumeUl(f64);
pub struct TempC(f64);
pub struct Ph(f64);
```

### `crates/audit/`

Two responsibilities, one crate.

**Chain** — append-only log of Ed25519-signed entries. Each entry contains:
- action name, params (redacted for sensitive values), decision, timestamp
- SHA-256 hash of the previous entry (hash chain)
- Ed25519 signature over `(entry_data || prev_hash)`

`Chain::verify()` walks the full chain, checks every signature and every hash link. Any break is a hard error.

**Rekor** — on protocol conclusion, submit the chain-tip hash to Sigstore's transparency log. Store the returned log ID in the final chain entry. Default-on; disable with `AXIOMLAB_REKOR_DISABLED=1`.

**Signing** — KMS (`AXIOMLAB_KMS_KEY_ID` env var) is the default when set. Local Ed25519 key is the fallback, never the preferred default for production.

```rust
pub struct Chain { ... }
impl Chain {
    pub fn append(&self, entry: EntryData, signer: &dyn Signer) -> Result<ChainEntry>;
    pub fn verify(&self) -> Result<VerifyResult>;
    pub fn tip_hash(&self) -> [u8; 32];
}

pub struct RekorClient { ... }
impl RekorClient {
    pub async fn checkpoint(&self, hash: &[u8; 32]) -> Result<LogId>;
}
```

No `DiscoveryJournal`. No SQLite for runs/findings. **The audit chain is the authoritative record.** The server queries the chain for the API; `RunSummary` is derived by parsing audit events.

### `crates/chemistry/`

Compatibility table ported from current `agent_runtime/src/chemistry.rs`. Self-contained — reagent pairs → `HazardLevel`. No other dependencies.

### `crates/sila/`

Proto definitions moved from `agent_runtime/proto/` here. Generated gRPC clients for all 6 instruments. Thin wrappers only — no business logic in this crate.

```rust
pub struct SilaClients {
    pub liquid_handler:     LiquidHandlerClient,
    pub robotic_arm:        RoboticArmClient,
    pub spectrophotometer:  SpectrophotometerClient,
    pub ph_meter:           PhMeterClient,
    pub incubator:          IncubatorClient,
    pub centrifuge:         CentrifugeClient,
}

impl SilaClients {
    pub async fn execute(&self, action: &Action) -> Result<serde_json::Value, SilaError>;
}
```

In simulator mode, `SilaClients::execute` dispatches to the local physics model (ported from `simulator/tools.rs`) instead of gRPC. Same interface, two backends.

### `crates/proofs/`

Two responsibilities:

1. **Artifact verification** — load signed proof manifest, verify Ed25519 signatures, confirm the named artifact exists for a given risk class. Same as current `proof_artifacts/` but clean.

2. **Predicate dispatch** — call Rust functions compiled from the Verus specs with actual proposed parameters. This is the key architectural fix over the current system.

```rust
// predicates.rs — compiled from verus_proofs/ specs
pub fn dispense_safe(volume_ul: f64) -> bool {
    0.5 <= volume_ul && volume_ul <= 1000.0
}
pub fn move_arm_safe(x: f64, y: f64, z: f64) -> bool {
    0.0 <= x && x <= 300.0 &&
    0.0 <= y && y <= 300.0 &&
    0.0 <= z && z <= 250.0
}
pub fn temperature_safe(temp_c: f64) -> bool {
    4.0 <= temp_c && temp_c <= 95.0
}
// ... one predicate per verifiable bound
```

`ProofGate` calls the relevant predicate with the actual proposed parameter values. Predicate returns `false` → `Rejection`. Artifact check confirms CI ran the Verus proof. Both must pass.

### `crates/gate/`

The core of the product. Defines the `Gate` trait and all 7 concrete implementations.

```rust
pub trait Gate: Send + Sync {
    fn name(&self) -> &'static str;
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection>;
}

pub struct Pipeline {
    gates: Vec<Arc<dyn Gate>>,
}

impl Pipeline {
    pub async fn run(&self, action: Action, ctx: &GateContext) -> Result<Action, Rejection> {
        for gate in &self.gates {
            gate.check(&action, ctx).await?;
        }
        Ok(action)
    }
}
```

**Gate implementations:**

`CapabilityGate` — per-action, per-parameter bounds derived from the capability policy. Hard block on any out-of-range value. No LLM retry allowed; reject and report.

`ChemistryGate` — checks proposed reagent against current vessel contents. Queries `lab_state` for what's in the target vessel. `HazardLevel::Dangerous` → reject.

`CalibrationGate` — active only for measurement tool actions (`read_absorbance`, `read_ph`, `read_temperature`). Reads calibration events from the audit chain. Missing or expired record → `Rejection::UncalibratedInstrument`. Expiry is mandatory — `valid_until` must be set on every calibration record.

`ProofGate` — loads proof manifest (from `crates/proofs/`), verifies signature, calls predicate with actual params. Two separate checks, both must pass.

`ApprovalGate` — async. Active only for `RiskClass::Actuation` and `RiskClass::Destructive`. Sends approval request to queue, awaits operator response. Approval is scoped to `hash(action.tool + action.params)` — different params require a new approval. Timeout → auto-deny.

`ExecuteGate` — dispatches to `SilaClients::execute`. gRPC error → reject. Returns instrument response in `GateContext` for the audit entry.

`AuditGate` — runs post-execute. Appends signed entry to chain. Submits to Rekor if this is a protocol conclusion. Never blocks the result — the action has already executed. A Rekor failure is logged but does not retroactively fail the action.

Also in `crates/gate/`:

`analyze_series` — the curve-fitting tool (OLS/Hill/MM). Ported from `scientific_compute/`. Not a gate — a standalone tool callable by the LLM. Auto-records a calibration audit entry when R² ≥ 0.80.

### `crates/llm/`

Thin. The LLM proposes; the pipeline enforces.

```rust
pub struct Orchestrator {
    llm:      LlmClient,
    pipeline: Arc<Pipeline>,
}

impl Orchestrator {
    pub async fn run(&self, directive: &str, ctx: GateContext) -> Result<String, OrchestratorError> {
        let mandate = build_mandate(directive, &ctx);
        loop {
            let proposal = self.llm.propose(&mandate, &TOOL_SCHEMA).await?;
            match proposal {
                Proposal::Action(action) => {
                    self.pipeline.run(action, &ctx).await?;
                }
                Proposal::Protocol(plan) => {
                    for step in plan.steps {
                        self.pipeline.run(step, &ctx).await?;
                    }
                }
                Proposal::Done { summary } => return Ok(summary),
            }
        }
    }
}
```

No hypothesis tracking. No finding counts. No convergence gates. No journal. Done means done.

**Mandate** — built fresh each iteration from:
- The operator directive
- Recent runs (last 5, from audit chain query)
- Calibration status per instrument (from audit chain)
- Parameter coverage summary
- Hardware capability bounds

**Tool schema** — `propose_protocol` and `analyze_series` only. No `update_journal`. No `design_experiment`. Tools the LLM can call are audited; tools not in the schema cannot be proposed.

---

## Server API (`server/`)

Routes only. No business logic in handlers — they delegate to pipeline/audit/queue.

```
GET  /api/status              loop state, iteration count, phase
GET  /api/audit               query + verify chain (paginated)
POST /api/audit/verify        verify full chain integrity
GET  /api/agenda              commissioning agenda with run status
POST /api/queue               operator pushes a directive (session + CSRF required)
GET  /api/queue               list pending/running/completed
DELETE /api/queue/{id}        cancel a queued item
GET  /api/approvals           list pending approval requests
POST /api/approvals/{id}      approve or deny (with notes)
GET  /api/lab                 vessel and reagent state
GET  /ready                   liveness check
GET  /metrics                 Prometheus
WS   /ws                      live event stream
```

No `/api/journal`. No `/api/findings`. No `/api/hypotheses`.

**AppState** — minimal:
```rust
pub struct AppState {
    pub running:        Arc<AtomicBool>,
    pub iteration:      Arc<AtomicU32>,
    pub audit_chain:    Arc<Chain>,
    pub lab_state:      Arc<Mutex<LabState>>,
    pub approval_queue: Arc<ApprovalQueue>,
    pub protocol_queue: Arc<ProtocolQueue>,
    pub tx:             broadcast::Sender<String>,
}
```

No `HypothesisManager`. No `DiscoveryJournal`. No `notebook`.

---

## Fixes vs Current Architecture

| Issue | Current | New |
|---|---|---|
| Proof gate | Checks artifact file exists (signed) | Checks artifact AND calls Rust predicate with actual params |
| Calibration | Advisory text in mandate | Hard gate — blocks measurement if expired or missing |
| Rekor | Opt-in (`AXIOMLAB_REKOR_ENABLED=1`) | Default-on (`AXIOMLAB_REKOR_DISABLED=1` to disable) |
| KMS | Feature-gated (`--features kms`) | Default when `AXIOMLAB_KMS_KEY_ID` is set; local key is fallback |
| Approval scope | Session-level (one approval gates a session) | Action+param hash scoped (new params = new approval) |
| Audit record | Dual-write to chain AND SQLite journal | Chain only — server derives summaries from chain queries |
| RevocationList | Always empty default | Wired to actual revoked key store, checked in `AuditGate` |
| Dead code | `findings`, `hypotheses` still in journal struct | Gone entirely |
| Convergence | Finding count gate (removed last session) | No convergence concept — LLM says done when done |

---

## What Gets Deleted

Done — all legacy crates removed:

- [x] `agent_runtime/` (entire crate)
- [x] `proof_artifacts/` (entire crate)
- [x] `proof_synthesizer/` (entire crate — research spike, not production)
- [x] `scientific_compute/` (fitting math absorbed into `crates/gate/`)
- [x] `physical_types/` (absorbed into `crates/types/`)
- [x] `server/` (replaced by new `server/`)
- [x] `visualizer/` (replaced by new `ui/`)
- [ ] `contracts/AuditVerifier.sol` — removed (was kept for planned future ZK work, but not yet implemented)

---

## Build Order

Each crate is independently compilable and tested before the next is started. No skipping ahead.

- [x] 1. `crates/types/` — domain types, zero deps **(done — 8 tests pass)**
- [x] 2. `crates/audit/` — chain + Rekor, depends on types **(done — 15 tests pass)**
- [x] 3. `crates/chemistry/` — compatibility table **(done — 7 tests pass; returns `HazardLevel`, operates on reagent names)**
- [x] 4. `crates/sila/` — proto codegen + clients **(done — 13 tests pass; unified `execute`, simulator + gRPC backends)**
- [x] 5. `crates/proofs/` — artifact loading + predicates **(done — 14 tests pass; predicates mirror verified bounds, called with actual params)**
- [x] 6. `crates/gate/` — pipeline + all 7 gates **(done — 32 tests pass; full end-to-end pipeline tested)**
- [x] 7. `crates/llm/` — orchestrator **(done — 18 tests pass; scripted client drives full pipeline)**
- [x] 8. `server/` — HTTP server **(done — 15 tests pass; routes + worker, chain-derived, no SQLite/journal)**
- [x] 9. `ui/` — frontend **(done — React + Vite, `npm run build` succeeds)**; legacy crates deleted

Each step gets its own commit with passing tests before moving to the next.

---

## Status: COMPLETE

All nine steps done. Workspace builds clean; full test suite green:

| Crate | Tests |
|---|---|
| `axiom-types` | 8 |
| `axiom-audit` | 15 |
| `axiom-chemistry` | 7 |
| `axiom-sila` | 13 |
| `axiom-proofs` | 14 |
| `axiom-gate` | 32 (incl. end-to-end pipeline) |
| `axiom-llm` | 18 (incl. orchestrator→pipeline→audit) |
| `axiomlab-server` | 15 (incl. HTTP integration) |
| `verus_proofs` (kept) | 31 + integration |

The server boots, loads + verifies a signed proof manifest, and serves the API.

### Running it
- Generate a signed manifest (runtime artifact; `.artifacts/` is gitignored):
  `cargo run -p axiom-proofs --bin gen-manifest`
  then export the printed `AXIOMLAB_MANIFEST_PUBKEY`.
- Without a valid manifest the `ProofGate` **fails closed** — every gated action
  is rejected, which is the safe default.
- Trusted manifest key resolution: `AXIOMLAB_MANIFEST_PUBKEY` env → embedded
  `MANIFEST_SIGNING_PUBLIC_KEY` constant.

### Follow-ups / known gaps
- Superseded by `ROADMAP.md`. Current priorities are production identity,
  transactional operational state, versioned protocol recovery, virtual-lab
  evidence, and deployment hardening.
- Rewrite-era CI references to deleted proof and control binaries have since
  been removed; current CI and runtime commands are documented elsewhere.

---

## What Does Not Change

- `verus_verified/` — kept verbatim. Binding Verus spec for the safety envelope.
- `verus_proofs/` — kept verbatim. Build-time bridge that extracts constants from the spec.
- `sila_sim/` — kept verbatim. Python mock unchanged.
- `agent_runtime/proto/` — moved to `crates/sila/proto/`. Content unchanged.
- `.github/workflows/verus.yml` — unchanged.
- `.github/workflows/ci.yml` — updated to new crate names, otherwise same.
