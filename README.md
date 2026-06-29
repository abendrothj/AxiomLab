# AxiomLab

**A natural-language operations layer for laboratory automation, with provable
safety and a tamper-evident record of everything that happens.**

AxiomLab lets an operator drive lab instruments with a plain-language directive.
An LLM turns that directive into concrete instrument actions — but the LLM has no
authority. Every action it proposes passes through an ordered chain of
fail-closed safety gates before any hardware moves, and every decision is written
to a cryptographically chained, externally-anchored audit log.

The LLM is a convenience. **The safety pipeline and the audit chain are the
product.**

---

## Two pillars

### 1. Provable safety

Actions flow through a fixed, fail-closed pipeline. The first gate to object
hard-stops the action; nothing is logged-and-continued, softened, or skipped.

```
LLM proposes action
  → CapabilityGate    operational bounds (per-action, per-parameter)
  → ChemistryGate     reagent compatibility vs. current vessel contents
  → CalibrationGate   measurement tools require a valid, unexpired calibration
  → ProofGate         signed proof artifact + a Verus-verified bound predicate
                      called with the actual proposed parameters
  → ApprovalGate      operator sign-off for actuation/destructive actions
  → ExecuteGate       SiLA 2 gRPC call (or the offline simulator)
  → AuditGate         append a signed entry to the chain
```

The `ProofGate`'s numeric bounds are **formally verified**. The safety envelope
lives in `verus_verified/lab_safety.rs`, proven for all inputs (no overflow, no
boundary bypass) by the Verus compiler + Z3 in CI. `verus_proofs/build.rs`
extracts those constants at build time, so the bound the runtime enforces is
*mechanically derived from what was proven* — not hand-copied.

Precise claim, not marketing: **the actuation bounds are formally verified; the
rest of the system is enforced by Rust types and tests.**

### 2. Tamper-evident audit

The **audit chain is the evidence system of record.** Operational directives,
leases, sessions, and approvals are persisted in SQLite. Every gate decision
and executed action is an append-only entry
that:

- hashes the previous entry (a SHA-256 hash chain), and
- is signed (Ed25519; AWS KMS in production, local key in dev).

`Chain::verify()` walks the whole chain and checks every hash link and every
signature; any break is a hard error. On protocol conclusion the chain tip is
anchored in **Sigstore Rekor**, giving an independent, timestamped, public
witness. Anchoring is on by default.

---

## Workspace layout

```
crates/
  types/        shared domain types — no logic
  audit/        Ed25519 hash chain + Rekor + signer (KMS/local) + revocations
  chemistry/    reagent compatibility table
  sila/         SiLA 2 gRPC clients + offline physics simulator
  proofs/       signed-manifest verification + verified bound predicates
  gate/         the 7-stage pipeline — the whole safety story
  llm/          thin orchestrator: propose → pipeline → conclude
server/         Axum HTTP + WebSocket API + background run worker
ui/             React dashboard
verus_verified/ lab_safety.rs — the binding, formally-verified spec
verus_proofs/   build-time bridge: generates runtime constants from the spec
sila_sim/       Python SiLA 2 mock (kept)
benchmarks/      reproducible virtual-lab protocol scenarios
```

---

## Quickstart

```bash
# 1. Build and test
cargo test --workspace

# 2. Generate a signed proof manifest (a runtime artifact; .artifacts/ is gitignored)
cargo run -p axiom-proofs --bin gen-manifest
#    prints:  AXIOMLAB_MANIFEST_PUBKEY=<key>   — export it so the ProofGate trusts it
export AXIOMLAB_MANIFEST_PUBKEY=<key>

# 3. Run the server (simulator backend by default)
AXIOMLAB_DEV_AUTH=1 cargo run -p axiomlab-server
#    → listening on 0.0.0.0:8080

# 4. Install and test the UI
cd ui && npm install
npm test && npm run build && npm run test:e2e

# 5. (optional, separate terminal) Run the UI dev server
npm run dev

# 6. Generate virtual-lab validation evidence from the repository root
cd ..
cargo run -p axiom-sila --bin validate-virtual-lab
```

Without a valid manifest the `ProofGate` **fails closed** — every gated action is
rejected. That is the safe default.

---

## API

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/status` | loop state, iteration, backend |
| GET | `/api/audit` | query entries (paginated) + verify summary |
| POST | `/api/audit/verify` | verify full chain integrity |
| GET | `/api/agenda` | commissioning agenda |
| GET/POST | `/api/queue` | list / submit a directive (operator session) |
| DELETE | `/api/queue/{id}` | cancel a queued directive |
| POST | `/api/queue/{id}/reconcile` | resolve an interrupted run after physical-state review |
| GET | `/api/approvals` | pending approval requests |
| GET | `/api/approvals/history` | durable approval lifecycle, including restart interruptions |
| POST | `/api/approvals/{id}` | approve or deny |
| GET | `/api/lab` | reagent inventory + vessel contents |
| GET | `/ready` | liveness |
| GET | `/metrics` | Prometheus |
| WS | `/ws` | live event stream |

See [OPERATOR_GUIDE.md](OPERATOR_GUIDE.md) for operational configuration
(signing, key rotation, Rekor, OIDC, approvals) and
[OUTLINE & RESEARCH.md](OUTLINE%20&%20RESEARCH.md)
for scope and positioning.

## Status and next steps

AxiomLab is a **hardware-free integrated alpha**: the safety path, simulator,
durable restart behavior, operator UI, and browser validation are implemented.
It is useful for safety-policy development, SiLA integration work, operator
workflow evaluation, and reproducible failure testing. It is not production lab
software yet.

OIDC/RBAC, SQLite directives and approvals, leases, and explicit reconciliation
are implemented. The next release should add versioned protocols and migrate
inventory/calibration projections. The phased plan is in
[ROADMAP.md](ROADMAP.md).

---

## What AxiomLab is not

- Not a discovery engine or "autonomous scientist." The science lives in the
  protocols and analysis; the LLM only composes known operations.
- Not a system that trusts the LLM. The entire architecture exists because it
  doesn't — authority lives in the gates and the chain.
