# AxiomLab — Formally Verified Autonomous Science Runtime
# Multi-stage build: Verus (pre-built) + Aeneas/Charon (OCaml) + Lean 4
#
# Targets linux/amd64 because Verus only ships x86-linux binaries.
# On Apple Silicon this runs via Rosetta — totally transparent.
#
# Usage:
#   docker compose build
#   docker compose run --rm axiomlab cargo test
#   docker compose run --rm axiomlab verus --version
#   docker compose run --rm axiomlab aeneas --help

# ── Stage 1: Download pre-built Verus ────────────────────────────
FROM --platform=linux/amd64 debian:bookworm-slim AS verus-fetch

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl ca-certificates unzip \
    && rm -rf /var/lib/apt/lists/*

ARG VERUS_VERSION=0.2026.03.10.13c14a1
RUN curl -fsSL \
    "https://github.com/verus-lang/verus/releases/download/release%2F${VERUS_VERSION}/verus-${VERUS_VERSION}-x86-linux.zip" \
    -o /tmp/verus.zip \
    && unzip /tmp/verus.zip -d /opt/verus \
    && rm /tmp/verus.zip \
    && chmod +x /opt/verus/verus-* -R 2>/dev/null; \
    find /opt/verus -type f -name 'verus' -executable | head -3

# ── Stage 2: Build Aeneas + Charon (OCaml project) ───────────────
FROM --platform=linux/amd64 ocaml/opam:debian-12-ocaml-5.2 AS aeneas-builder

RUN sudo apt-get update && sudo apt-get install -y --no-install-recommends \
        cmake git curl build-essential pkg-config libssl-dev libgmp-dev \
    && sudo rm -rf /var/lib/apt/lists/*

# Charon (Rust→LLBC compiler) needs Rust.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/home/opam/.cargo/bin:${PATH}"

# Install OCaml dependencies for Aeneas.
RUN opam update && opam install -y \
        ppx_deriving visitors easy_logging zarith yojson core_unix \
        odoc ocamlgraph menhir unionFind progress domainslib

WORKDIR /home/opam
RUN git clone --depth 1 https://github.com/AeneasVerif/aeneas.git

WORKDIR /home/opam/aeneas
RUN eval $(opam env) && make setup-charon
RUN eval $(opam env) && make

# ── Stage 3: Install elan + Lean 4 ───────────────────────────────
FROM --platform=linux/amd64 debian:bookworm-slim AS lean-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

ENV ELAN_HOME=/opt/elan
ENV PATH="${ELAN_HOME}/bin:${PATH}"

RUN curl -sSf https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh \
    | bash -s -- -y --default-toolchain leanprover/lean4:stable --no-modify-path

RUN ${ELAN_HOME}/bin/lean --version

# ── Stage 4: Final runtime image ─────────────────────────────────
FROM --platform=linux/amd64 rust:1.85-bookworm AS runtime

LABEL maintainer="AxiomLab" \
      description="Formally verified Rust runtime for autonomous scientific discovery"

RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev git libgmp-dev \
    && rm -rf /var/lib/apt/lists/*

# ── Copy Verus (pre-built release) ──
COPY --from=verus-fetch /opt/verus /opt/verus
# The zip extracts into a versioned directory. Symlink the binary.
RUN VERUS_BIN=$(find /opt/verus -name 'verus' -type f | head -1) \
    && ln -sf "$VERUS_BIN" /usr/local/bin/verus \
    && Z3_BIN=$(find /opt/verus -name 'z3' -type f | head -1) \
    && ln -sf "$Z3_BIN" /usr/local/bin/z3

# Verus needs a specific Rust toolchain to drive its internal compiler.
ARG VERUS_RUST_TOOLCHAIN=1.93.1-x86_64-unknown-linux-gnu
RUN rustup install "${VERUS_RUST_TOOLCHAIN}"

# ── Copy Aeneas binary + Charon ──
COPY --from=aeneas-builder /home/opam/aeneas/bin/aeneas /usr/local/bin/aeneas
COPY --from=aeneas-builder /home/opam/aeneas/charon/bin/charon /usr/local/bin/charon
COPY --from=aeneas-builder /home/opam/aeneas/backends/lean /opt/aeneas-lean-backend

# ── Copy Lean 4 (elan-managed) ──
COPY --from=lean-builder /opt/elan /opt/elan
ENV ELAN_HOME=/opt/elan
ENV PATH="${ELAN_HOME}/bin:${PATH}"

# ── Environment variables for AxiomLab crates ──
ENV VERUS_PATH=/usr/local/bin/verus
ENV AENEAS_PATH=/usr/local/bin/aeneas
ENV CHARON_PATH=/usr/local/bin/charon
ENV AENEAS_LEAN_BACKEND=/opt/aeneas-lean-backend

# ── Pre-fetch workspace deps (Docker cache layer) ──
WORKDIR /axiomlab
COPY Cargo.toml Cargo.lock ./
COPY scientific_compute/Cargo.toml scientific_compute/Cargo.toml
COPY physical_types/Cargo.toml physical_types/Cargo.toml
COPY agent_runtime/Cargo.toml agent_runtime/Cargo.toml
COPY verus_proofs/Cargo.toml verus_proofs/Cargo.toml
COPY proof_synthesizer/Cargo.toml proof_synthesizer/Cargo.toml
COPY aeneas_lean_semantics/Cargo.toml aeneas_lean_semantics/Cargo.toml

RUN mkdir -p scientific_compute/src physical_types/src agent_runtime/src \
             verus_proofs/src proof_synthesizer/src aeneas_lean_semantics/src \
    && for d in scientific_compute physical_types agent_runtime \
                verus_proofs proof_synthesizer aeneas_lean_semantics; do \
        echo "" > "$d/src/lib.rs"; \
    done \
    && cargo fetch || true \
    && rm -rf scientific_compute/src physical_types/src agent_runtime/src \
              verus_proofs/src proof_synthesizer/src aeneas_lean_semantics/src

# ── Copy full source and build ──
COPY . .
RUN cargo build --release 2>&1 && cargo build --tests 2>&1

# Smoke-test: verify all tools are accessible.
RUN echo "=== Toolchain check ===" \
    && verus --version 2>&1 | head -3 || echo "WARN: verus" \
    && lean --version 2>&1 | head -1 || echo "WARN: lean" \
    && aeneas --help 2>&1 | head -3 || echo "WARN: aeneas" \
    && z3 --version 2>&1 | head -1 || echo "WARN: z3" \
    && echo "=== All tools checked ==="

CMD ["cargo", "test"]
