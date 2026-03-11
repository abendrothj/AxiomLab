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

## Docker Testing & Verification

AxiomLab includes formal verification infrastructure (Verus, Aeneas, Lean 4) available inside Docker.

### Local development (macOS/Linux):
```bash
# Regular tests only (skip Verus, Aeneas, Lean tests)
cargo test

# This skips tests marked #[ignore] that need Docker
```

### Full verification inside Docker:
```bash
# Start the container and run all tests (including ignored ones)
docker compose run --rm axiomlab cargo test -- --include-ignored

# Run only ignored tests (Verus, Aeneas, Lean)
docker compose run --rm axiomlab cargo test -- --ignored

# Run specific Docker-only test
docker compose run --rm axiomlab cargo test real_aeneas_translates_simple_function -- --ignored

# Build and start interactive shell
docker compose run --rm axiomlab bash
```

**Inside Docker:** All 29 Verus safety proofs are validated, Aeneas translation is executable, and Lean type-checker confirms theorem correctness.

## License

MIT
