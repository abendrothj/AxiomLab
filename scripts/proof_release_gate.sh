#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

OUT_DIR="${OUT_DIR:-.artifacts/proof}"
mkdir -p "$OUT_DIR"

MANIFEST_PATH="$OUT_DIR/manifest.json"
CACHE_PATH="$OUT_DIR/cache.json"
SPEC_PATH="$OUT_DIR/spec.json"
POLICY_PATH="$OUT_DIR/policy.json"

if command -v git >/dev/null 2>&1; then
  GIT_COMMIT="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
  WORKSPACE_HASH="$(git ls-files -z | xargs -0 shasum -a 256 | shasum -a 256 | awk '{print $1}')"
else
  GIT_COMMIT="unknown"
  WORKSPACE_HASH="unknown"
fi

# Runtime/build identity hash. In CI this can be replaced by image digest,
# signed build artifact hash, or release binary hash.
BINARY_HASH="$(shasum -a 256 Cargo.lock | awk '{print $1}')"

cat > "$SPEC_PATH" <<EOF
{
  "build": {
    "git_commit": "$GIT_COMMIT",
    "binary_hash": "$BINARY_HASH",
    "workspace_hash": "$WORKSPACE_HASH"
  },
  "artifacts": [
    {
      "id": "ols_functional",
      "source_path": "scientific_compute/src/discovery.rs",
      "mir_path": null,
      "lean_paths": ["lean4/OlsFunctional.lean"],
      "verus_proof_path": null,
      "metadata": {
        "domain": "discovery",
        "criticality": "high"
      }
    },
    {
      "id": "ols_rational",
      "source_path": "scientific_compute/src/discovery.rs",
      "mir_path": null,
      "lean_paths": ["lean4/OlsRational.lean"],
      "verus_proof_path": null,
      "metadata": {
        "domain": "discovery",
        "criticality": "high"
      }
    }
  ],
  "actions": [
    {
      "action": "move_arm",
      "required_artifacts": ["ols_functional", "ols_rational"],
      "rationale": "Only permit robotic actuation when discovery math proofs are valid"
    },
    {
      "action": "dispense",
      "required_artifacts": ["ols_functional", "ols_rational"],
      "rationale": "Only permit liquid handling when discovery math proofs are valid"
    }
  ]
}
EOF

cat > "$POLICY_PATH" <<EOF
{
  "required_artifacts": ["ols_functional", "ols_rational"],
  "require_zero_sorry": true,
  "expected_git_commit": "$GIT_COMMIT",
  "expected_binary_hash": "$BINARY_HASH"
}
EOF

echo "[1/5] Building proofctl"
cargo build -p proof_artifacts --bin proofctl

echo "[2/5] Generating proof manifest"
cargo run -p proof_artifacts --bin proofctl -- generate \
  --spec "$SPEC_PATH" \
  --out "$MANIFEST_PATH" \
  --cache "$CACHE_PATH"

echo "[3/5] Enforcing CI proof gate"
cargo run -p proof_artifacts --bin proofctl -- gate \
  --manifest "$MANIFEST_PATH" \
  --policy "$POLICY_PATH"

echo "[4/5] Running proof-artifact subsystem tests"
cargo test -p proof_artifacts -- --nocapture

echo "[5/5] Running runtime policy integration tests"
cargo test -p agent_runtime proof_policy -- --nocapture
cargo test -p agent_runtime sim2_orchestrator -- --nocapture

echo "Release gate passed. Manifest: $MANIFEST_PATH"
