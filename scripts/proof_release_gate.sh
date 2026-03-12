#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

OUT_DIR="${OUT_DIR:-.artifacts/proof}"
mkdir -p "$OUT_DIR"

MANIFEST_PATH="$OUT_DIR/manifest.json"
SIGNED_MANIFEST_PATH="$OUT_DIR/manifest.signed.json"
CACHE_PATH="$OUT_DIR/cache.json"
SPEC_PATH="$OUT_DIR/spec.json"
POLICY_PATH="$OUT_DIR/policy.json"
PRIVATE_KEY_PATH="$OUT_DIR/manifest_signing_key.private.b64"
PUBLIC_KEY_PATH="$OUT_DIR/manifest_signing_key.public.b64"
AUDIT_LOG_PATH="$OUT_DIR/runtime_audit.jsonl"

if command -v git >/dev/null 2>&1; then
  GIT_COMMIT="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
  WORKSPACE_HASH="$(git ls-files -z | xargs -0 shasum -a 256 | shasum -a 256 | awk '{print $1}')"
else
  GIT_COMMIT="unknown"
  WORKSPACE_HASH="unknown"
fi

BINARY_HASH="$(shasum -a 256 Cargo.lock | awk '{print $1}')"
CONTAINER_IMAGE_DIGEST="${AXIOMLAB_CONTAINER_IMAGE_DIGEST:-local-dev-image}"
DEVICE_ID="${AXIOMLAB_DEVICE_ID:-dev-rig}"
FIRMWARE_VERSION="${AXIOMLAB_FIRMWARE_VERSION:-dev-firmware}"

cat > "$SPEC_PATH" <<EOF_JSON
{
  "build": {
    "git_commit": "$GIT_COMMIT",
    "binary_hash": "$BINARY_HASH",
    "workspace_hash": "$WORKSPACE_HASH",
    "container_image_digest": "$CONTAINER_IMAGE_DIGEST",
    "device_id": "$DEVICE_ID",
    "firmware_version": "$FIRMWARE_VERSION"
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
    },
    {
      "id": "lab_safety_verus",
      "source_path": "verus_verified/lab_safety.rs",
      "mir_path": null,
      "lean_paths": [],
      "verus_proof_path": "verus_verified/lab_safety.rs",
      "metadata": {
        "domain": "hardware",
        "criticality": "critical"
      }
    }
  ],
  "actions": [
    {
      "action": "read_sensor",
      "risk_class": "ReadOnly",
      "required_artifacts": ["ols_functional"],
      "rationale": "Sensor reads require validated scientific compute artifacts"
    },
    {
      "action": "move_arm",
      "risk_class": "Actuation",
      "required_artifacts": ["ols_functional", "ols_rational", "lab_safety_verus"],
      "rationale": "Robotic actuation requires discovery and hardware safety proofs"
    },
    {
      "action": "dispense",
      "risk_class": "LiquidHandling",
      "required_artifacts": ["ols_functional", "lab_safety_verus"],
      "rationale": "Liquid handling requires science + hardware safety proofs"
    }
  ]
}
EOF_JSON

cat > "$POLICY_PATH" <<EOF_JSON
{
  "required_artifacts": ["ols_functional", "ols_rational", "lab_safety_verus"],
  "require_zero_sorry": true,
  "expected_git_commit": "$GIT_COMMIT",
  "expected_binary_hash": "$BINARY_HASH",
  "expected_workspace_hash": "$WORKSPACE_HASH",
  "expected_container_image_digest": "$CONTAINER_IMAGE_DIGEST",
  "max_manifest_age_secs": 900
}
EOF_JSON

echo "[1/8] Building proofctl"
cargo build -p proof_artifacts --bin proofctl

echo "[2/8] Generating proof manifest"
cargo run -p proof_artifacts --bin proofctl -- generate \
  --spec "$SPEC_PATH" \
  --out "$MANIFEST_PATH" \
  --cache "$CACHE_PATH"

echo "[3/8] Generating signing keys (first run only)"
if [[ ! -f "$PRIVATE_KEY_PATH" || ! -f "$PUBLIC_KEY_PATH" ]]; then
  cargo run -p proof_artifacts --bin proofctl -- keygen \
    --private "$PRIVATE_KEY_PATH" \
    --public "$PUBLIC_KEY_PATH"
fi

echo "[4/8] Signing manifest"
cargo run -p proof_artifacts --bin proofctl -- sign \
  --manifest "$MANIFEST_PATH" \
  --private-key "$PRIVATE_KEY_PATH" \
  --out "$SIGNED_MANIFEST_PATH" \
  --key-id "axiomlab-local-root"

echo "[5/8] Verifying signed manifest"
cargo run -p proof_artifacts --bin proofctl -- verify \
  --signed-manifest "$SIGNED_MANIFEST_PATH" \
  --public-key "$PUBLIC_KEY_PATH"

echo "[6/8] Enforcing CI proof gate on signed manifest"
cargo run -p proof_artifacts --bin proofctl -- gate \
  --signed-manifest "$SIGNED_MANIFEST_PATH" \
  --public-key "$PUBLIC_KEY_PATH" \
  --policy "$POLICY_PATH"

echo "[7/8] Running proof-artifact subsystem tests"
cargo test -p proof_artifacts -- --nocapture

echo "[8/9] Running runtime policy + sandbox integration tests"
AXIOMLAB_AUDIT_LOG="$AUDIT_LOG_PATH" cargo test -p agent_runtime proof_policy -- --nocapture
AXIOMLAB_AUDIT_LOG="$AUDIT_LOG_PATH" cargo test -p agent_runtime sim2_orchestrator -- --nocapture

echo "[9/9] Verifying tamper-evident runtime audit chain"
cargo run -p agent_runtime --bin auditctl -- verify --path "$AUDIT_LOG_PATH"

echo "Release gate passed. Signed manifest: $SIGNED_MANIFEST_PATH"
echo "Audit log: $AUDIT_LOG_PATH"
