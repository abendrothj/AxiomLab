# Archived Verus specs

These specs are **not wired into the runtime** and are no longer part of CI
verification. They are kept for reference only.

The single binding, load-bearing spec is `verus_verified/lab_safety.rs`: its
constants are extracted at build time by `verus_proofs/build.rs` and enforced by
the runtime `ProofGate`. CI (`.github/workflows/verus.yml`) verifies only that
file.

- `vessel_registry.rs` — vessel-capacity invariants (not enforced at runtime)
- `protocol_safety.rs` — protocol-ordering specs (superseded by the gate pipeline)
- `dilution_protocol.rs` — dilution conservation (not invoked on the dispense path)
- `lab_safety_UNSAFE.rs` — deliberately-unsafe counterexample for teaching

To make any of these load-bearing again, wire its constants/predicates through
`verus_proofs` (as `hardware_bounds` does) and add it back to `verus.yml`.
