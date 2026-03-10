# AxiomLab

> A bare-metal, memory-safe, and formally verified Rust runtime for autonomous AI scientists and self-driving laboratories.

## Crate Map

| Crate | Phase | Purpose |
|---|---|---|
| `scientific_compute` | 1 | Pure-Rust linear algebra, FFT, and numerical primitives (no C/Fortran FFI). |
| `physical_types` | 1 | Compile-time dimensional analysis via `uom` – prevents unit-mismatch bugs at the type level. |
| `agent_runtime` | 2 | Sandboxed agent orchestrator: allowlisted filesystem access, resource limits, tool dispatch. |
| `verus_proofs` | 3 | Verus SMT specifications for concurrent hardware control and resource invariants. |
| `proof_synthesizer` | 3 | VeruSAGE-inspired agent loop that auto-generates Verus proof annotations. |
| `aeneas_lean_semantics` | 4 | Rust MIR → Lean 4 translation for theorem-prover-level algorithmic verification. |

## Quick Start

```bash
# Build the entire workspace
cargo build

# Run all tests
cargo test
```

## License

MIT
