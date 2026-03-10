# AxiomLab

> A bare-metal, memory-safe, and formally verified Rust runtime for autonomous AI scientists and self-driving laboratories.

## Crate Map

| Crate | Phase | Purpose |
|---|---|---|
| `scientific_compute` | 1 | Pure-Rust linear algebra (`nalgebra`), FFT (`rustfft`), and numerical primitives — no C/Fortran FFI. |
| `physical_types` | 1 | Compile-time dimensional analysis via `uom` — prevents unit-mismatch bugs at the type level. |
| `agent_runtime` | 2 | Sandboxed agent orchestrator: path + command allowlists, resource limits, LLM-driven tool dispatch, experiment lifecycle state machine. |
| `verus_proofs` | 3 | Verus-compatible specs (macro shim for dual `rustc`/Verus compilation), concurrency token proofs, hardware-bound invariants, verified resource allocator. |
| `proof_synthesizer` | 3 | VeruSAGE-inspired observe→reason→act loop: invokes Verus compiler, parses diagnostics, asks LLM to refine proof annotations until verification succeeds. |
| `aeneas_lean_semantics` | 4 | End-to-end Rust MIR → Aeneas → Lean 4 pipeline: MIR export, Aeneas translation, Lean type-checking. |

## Quick Start

```bash
# Build the entire workspace
cargo build

# Run all tests
cargo test
```

## License

MIT
