# AxiomLab Operator Guide

Operational reference for running AxiomLab in development and production. It
describes what the system **actually does**, grounded in the current code.

AxiomLab is a natural-language operations layer over lab automation with two
guarantees: **provable safety** (a fail-closed gate pipeline whose hardware
bounds are formally verified) and **tamper-evident audit** (an Ed25519 hash
chain anchored in Sigstore Rekor). The LLM proposes; the pipeline enforces.

---

## 1. The gate pipeline

Every proposed action runs through seven gates in a fixed order. The first to
object returns a `Rejection`, which hard-stops the action and is itself written
to the audit chain as a `deny`. Nothing is skipped or softened.

| # | Gate | What it enforces |
|---|---|---|
| 1 | `CapabilityGate` | Operational per-parameter bounds (e.g. dispense 0.5–1000 µL, arm 0–300/250 mm). Configurable policy; a **subset** of the verified envelope. |
| 2 | `ChemistryGate` | The reagent being added is compatible with the target vessel's current contents (GHS/NFPA table). |
| 3 | `CalibrationGate` | Measurement tools (`read_absorbance`, `read_ph`, `read_temperature`) require a calibration record whose `valid_until` is in the future. |
| 4 | `ProofGate` | (a) the signed manifest lists a `Passed`, sorry-free artifact for the action (Verus-backed for high-risk); (b) the **verified bound predicate** passes with the actual parameters; (c) for a dispense, the **verified cumulative-capacity check** (`safe_add_volume`) confirms the vessel's running total stays within capacity. |
| 5 | `ApprovalGate` | `Actuation`/`Destructive` actions require operator sign-off, scoped to `sha256(tool‖params)`. Timeout → auto-deny. |
| 6 | `ExecuteGate` | Dispatches to SiLA 2 gRPC (or the simulator); records the result. |
| 7 | `AuditGate` | Appends a signed entry to the chain. |

**Two-tier bounds.** `CapabilityGate` is operational policy you can tune.
`ProofGate` enforces the *formally verified* hardware envelope from
`verus_verified/lab_safety.rs` — and it is binding: the constants it checks are
generated from the verified source at build time (`verus_proofs/build.rs`).

---

## 2. Signing the audit chain

Every chain entry is Ed25519-signed. Resolution order (`signer_from_env`):

1. **AWS KMS** — set `AXIOMLAB_KMS_KEY_ID` (and build with `--features kms` on
   `axiom-audit`). Preferred for production; the private key never leaves KMS.
2. **Inline key** — `AXIOMLAB_AUDIT_SIGNING_KEY` (base64 of a 32-byte key), for CI.
3. **File-backed** — `AXIOMLAB_AUDIT_SIGNING_KEY_PATH`, else
   `~/.config/axiomlab/audit_signing.key` (auto-created, mode 0600). Dev default.

Verify the chain at any time:

```bash
curl -X POST localhost:8080/api/audit/verify
# → {"ok":true,"entries_checked":N,"signatures_verified":N,"tip_hash":"..."}
```

`Chain::verify()` checks every hash link and every signature; any break is a
hard failure.

---

## 3. Rekor transparency anchoring

On protocol conclusion the chain tip is submitted to Sigstore Rekor as a
`hashedrekord`, and the returned log UUID is written back into the chain. This is
an independent, public, timestamped witness of the chain state.

- **On by default.** Disable with `AXIOMLAB_REKOR_DISABLED=1` (e.g. offline CI).
- Override the log URL with `AXIOMLAB_REKOR_URL`.
- A Rekor failure is logged but never retroactively fails a run.

---

## 4. The proof manifest

The `ProofGate`'s artifact check loads a signed manifest. The manifest is a
**runtime artifact** (`.artifacts/` is gitignored) and must be generated per
deployment.

```bash
# Generate + sign a manifest for the standard action set, rotating to a fresh key
cargo run -p axiom-proofs --bin gen-manifest [output_path]
#   writes:  .artifacts/proof/manifest.signed.json
#            .artifacts/proof/manifest.signed.json.signing_key.private.b64  (keep secret)
#   prints:  AXIOMLAB_MANIFEST_PUBKEY=<pubkey>
```

Trusted-key resolution: `AXIOMLAB_MANIFEST_PUBKEY` (env) → the embedded
`MANIFEST_SIGNING_PUBLIC_KEY` constant. Set the env var to the printed key, or
embed it and recompile to make it the default.

**Fail-closed:** if the manifest is missing or fails verification, the server
loads an empty manifest and the `ProofGate` rejects every gated action. This is
intentional — no valid proof, no actuation.

---

## 5. Operator approvals

`Actuation`/`Destructive` actions block at the `ApprovalGate` until an operator
decides.

```bash
curl localhost:8080/api/approvals          # list pending {id, tool, params, scope_hash}
curl -X POST localhost:8080/api/approvals/<id> \
  -H 'content-type: application/json' \
  -d '{"approved":true,"notes":"reviewed","approver_id":"alice"}'
```

- Approval is **scoped to `sha256(tool‖params)`** — once granted, an identical
  action does not re-prompt for the session; *different* params require a fresh
  approval.
- A revoked approver key or approval id (see `AXIOMLAB_REVOCATION_LIST`) is
  rejected even when "approved".
- No decision within the timeout → auto-deny.

---

## 6. Submitting work

```bash
# Submit a directive (JWT required when AXIOMLAB_JWT_SECRET is set)
curl -X POST localhost:8080/api/queue \
  -H 'authorization: Bearer <jwt>' \
  -H 'content-type: application/json' \
  -d '{"directive":"Calibrate the spectrophotometer, then dispense 50 µL into tube_1"}'
```

A background worker claims the next pending directive, builds a `GateContext`,
and runs the `Orchestrator`: the LLM proposes `propose_protocol` /
`analyze_series` / `done`; protocol steps run through the pipeline. A gate
rejection does not end the run — the reason is fed back into the next mandate so
the model can revise, bounded by `AXIOMLAB_MAX_REJECTIONS`. The gates still
reject every unsafe action; only the orchestrator's patience is bounded.

**Calibration is traceable, not self-certified.** `analyze_series` will only
propose a calibration when the x-axis is drawn from **registered reference
materials** (reagents with a `reference_material_id`), there are ≥5 distinct
standard levels, and the fit clears R² ≥ 0.80. Even then it is not recorded
until an operator approves it (calibration unlocks measurement). The signed
calibration entry records the standards, level count, model, and approver — so
an instrument can never be calibrated against arbitrary data it produced about
unknown samples.

---

## 7. Backends

- **Simulator (default):** an offline Beer-Lambert physics model. Used unless an
  endpoint is configured.
- **gRPC:** set `AXIOMLAB_SILA_ENDPOINT=http://host:port` to dispatch the
  liquid/spectrophotometer/thermal services over gRPC (`instruments.proto`).
- **Full SiLA 2:** set `AXIOMLAB_SILA_PROTOCOL=sila2` with
  `AXIOMLAB_SILA_ENDPOINT=http://host:port` to speak the SiLA 2 feature packages
  used by the Python `sila_sim` server (`LiquidHandler`, `Spectrophotometer`,
  `Incubator`, with `sila2.org.silastandard` wrapper types).

### End-to-end gRPC without hardware

A reference instrument server (the `instruments.proto` contract, backed by the
same physics simulator) ships in this repo. Use it to exercise the entire gRPC
path end to end:

```bash
# terminal 1 — the instrument server
cargo run -p axiom-sila --bin mock-instrument-server          # listens on :50051

# terminal 2 — point the system at it
AXIOMLAB_SILA_ENDPOINT=http://127.0.0.1:50051 cargo run -p axiomlab-server
# /api/status now reports "backend":"hardware"
```

This is verified automatically: `cargo test -p axiom-sila --test grpc_e2e` round-trips
over a real connection, and `axiom-gate`'s `pipeline_executes_over_grpc` runs the
full gate pipeline through it.

### End-to-end against the Python SiLA 2 simulator

The Rust backend can also speak the full SiLA 2 feature protocol emitted by
`sila_sim`:

```bash
# terminal 1 — Python SiLA 2 simulator
cd sila_sim
python -m axiomlab_sim --insecure --disable-discovery --port 50052

# terminal 2 — point AxiomLab at full SiLA 2
AXIOMLAB_SILA_ENDPOINT=http://127.0.0.1:50052 \
AXIOMLAB_SILA_PROTOCOL=sila2 \
  cargo run -p axiomlab-server
```

The opt-in integration test starts `sila_sim` and round-trips dispense,
absorbance, and incubator temperature over the real SiLA 2 wire format:

```bash
AXIOMLAB_RUN_SILA2_E2E=1 cargo test -p axiom-sila --test full_sila2_e2e -- --nocapture
```

---

## 8. Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `AXIOMLAB_BIND` | `0.0.0.0:8080` | Listen address |
| `AXIOMLAB_JWT_SECRET` | _(unset → open dev mode)_ | HS256 secret for `POST /api/queue` |
| `AXIOMLAB_KMS_KEY_ID` | — | KMS key for audit signing (needs `--features kms`) |
| `AXIOMLAB_AUDIT_SIGNING_KEY` | — | Inline base64 signing key (CI) |
| `AXIOMLAB_AUDIT_SIGNING_KEY_PATH` | `~/.config/axiomlab/audit_signing.key` | File-backed key |
| `AXIOMLAB_AUDIT_LOG` | `.artifacts/audit/runtime_audit.jsonl` | Chain file |
| `AXIOMLAB_REKOR_DISABLED` | _(unset → on)_ | `1` disables Rekor anchoring |
| `AXIOMLAB_REKOR_URL` | sigstore public log | Rekor endpoint |
| `AXIOMLAB_REVOCATION_LIST` | — | JSON `{key_ids:[], approval_ids:[]}` |
| `AXIOMLAB_PROOF_MANIFEST` | `.artifacts/proof/manifest.signed.json` | Manifest path |
| `AXIOMLAB_MANIFEST_PUBKEY` | embedded constant | Trusted manifest signing key |
| `AXIOMLAB_SILA_ENDPOINT` | _(unset → simulator)_ | gRPC instrument endpoint |
| `AXIOMLAB_SILA_PROTOCOL` | `instruments` | Set `sila2` for the full SiLA 2 protocol |
| `AXIOMLAB_SILA_BIND` | `127.0.0.1:50051` | Bind address for the mock instrument server |
| `AXIOMLAB_LAB_STATE_PATH` | `.artifacts/lab_state.json` | Reagent/vessel state |
| `AXIOMLAB_LLM_ENDPOINT` / `_API_KEY` / `_MODEL` | localhost / `no-key` / `claude-opus-4-8` | LLM (OpenAI-compatible) |
| `AXIOMLAB_MAX_ITERATIONS` | `50` | Orchestrator iteration cap |
| `AXIOMLAB_MAX_REJECTIONS` | `5` | Gate rejections tolerated before a run aborts |
| `AXIOMLAB_CALIBRATION_TTL_SECS` | `86400` | Lifetime of a recorded calibration |

---

## 9. Formal verification (honest scope)

What is proven (`verus_verified/lab_safety.rs`, for all inputs, no integer
overflow): the scalar actuation bounds (arm, temperature, pressure, volume) and
the **stateful cumulative-capacity property** — `safe_add_volume` proves a
dispense never pushes a vessel's running total past its capacity. The `ProofGate`
calls the runtime twin on every dispense. CI (`.github/workflows/verus.yml`) runs
the Verus compiler on that file and asserts the runtime uses the generated
constants.

What is **not** proven (enforced by Rust + tests, not Verus): the pipeline
ordering, the audit chain's cryptographic properties, chemistry, calibration,
approvals, the server. Verus covers the physical actuation envelope — the one
place a "holds for all inputs" guarantee is most worth having — and nothing more.
Other specs are kept, unwired, under `verus_verified/archive/`.

---

## 10. Evaluating the LLM

The LLM's *enforcement* is covered by the gate tests; its *judgement* is measured
by a scenario suite in `crates/llm/tests/eval.rs`. Each scenario is a directive +
lab setup + an expectation over the resulting audit chain (concluded safely,
calibrated before measuring, recovered from a rejection, chain verifies).

- CI runs the scenarios against scripted reference solutions
  (`cargo test -p axiom-llm --test eval`) — this exercises the orchestration and
  recovery loop with no network.
- To evaluate a **real model**, point it at an endpoint and run the live
  scorecard:

  ```bash
  AXIOMLAB_LLM_ENDPOINT=… AXIOMLAB_LLM_API_KEY=… AXIOMLAB_LLM_MODEL=claude-opus-4-8 \
    cargo test -p axiom-llm --test eval -- --ignored --nocapture
  ```

  It prints pass/fail per scenario and an overall score. Add scenarios as the
  tool surface grows; the harness is client-agnostic.
