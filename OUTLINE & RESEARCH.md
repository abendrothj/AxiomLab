# AxiomLab — Scope & Positioning

## What this is

A **natural-language operations layer for laboratory automation** with two
guarantees that most agent systems lack:

1. **Provable safety** — proposed actions pass a fail-closed gate pipeline whose
   hardware-actuation bounds are formally verified (Verus + Z3) and mechanically
   linked to the running code.
2. **Tamper-evident audit** — every decision is an Ed25519-signed entry in a hash
   chain, with the chain tip anchored in Sigstore Rekor.

The LLM is the convenience layer; it composes known instrument operations from a
plain-language directive. It holds no authority. The defensible asset is the
**enforcement substrate** — the gate pipeline and the audit chain — which is
valuable independent of who or what is proposing the actions.

## What this is not

- **Not a discovery engine / "autonomous scientist."** The scientific value lives
  in the protocols and the deterministic analysis, not in the model. Earlier
  versions of this project carried discovery framing; it has been removed.
- **Not a system that trusts the LLM.** The whole architecture is an admission
  that you can't — so authority lives in the gates and the chain, and the LLM is
  interchangeable.

## Why it matters

Self-driving / cloud labs are real, and their bottleneck is reliability, safety,
and auditability — not idea generation. A plain-English front end that reliably
composes *known* operations, behind a hard safety envelope, with a
cryptographically attestable record of every actuation, is useful in exactly the
settings where mistakes are expensive or regulated. The same shape —
untrusted proposer → ordered fail-closed gates → signed audit chain — generalizes
to any agent touching consequential systems.

## The formal-verification claim (stated honestly)

Verus proves one thing well: the scalar hardware-safety bounds in
`verus_verified/lab_safety.rs` hold for **all** inputs, with no integer overflow
or boundary bypass. Those constants are extracted at build time and enforced by
the `ProofGate`, so "what runs" is derived from "what was proven."

It is deliberately **narrow**. Verus is the right tool for bounded, deterministic
numeric properties and the wrong tool for concurrency, cryptography, or external
systems — so it is not pointed at the pipeline or the audit chain (those are
enforced by Rust types and tests). The honest one-line claim:

> Actuation bounds are formally verified; the action history is tamper-evident;
> everything else is well-typed and tested.

## Roadmap (aligned to the two pillars)

The enforcement substrate is implemented and usable as a hardware-free alpha.
Identity, SQLite run/approval persistence, leases, and fail-closed recovery are
implemented. Production work should now prioritize versioned protocols and the
remaining inventory/calibration migration—not broader autonomy or more devices.

The authoritative phased plan is [ROADMAP.md](ROADMAP.md). In order:

1. OIDC sessions, RBAC, and separation of duties.
2. SQLite repositories, leases, migrations, and crash recovery.
3. Immutable protocol versions, checkpoints, pause/stop, and reconciliation.
4. Deeper deterministic virtual-lab faults and chemistry policy provenance.
5. Deployment, observability, backup, and incident runbooks.
6. An optional narrow hardware pilot after recovery semantics are proven.

Formal verification should expand only when a new property is both
load-bearing and mechanically connected to runtime enforcement.
