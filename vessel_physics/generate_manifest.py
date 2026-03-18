#!/usr/bin/env python3
"""Generate proof_artifacts/vessel_physics_manifest.json from real Verus runs.

Verifies two source files:
  - verus_verified/vessel_registry.rs  (vessel physics, 11 theorems)
  - verus_verified/protocol_safety.rs  (protocol invariants, 13 theorems)

Both must pass for ArtifactStatus = "Passed".

Usage:
    # Via local verus binary (macOS ARM64, Linux):
    python3 vessel_physics/generate_manifest.py

    # Via Docker:
    python3 vessel_physics/generate_manifest.py --docker

    # Check status without writing:
    python3 vessel_physics/generate_manifest.py --status-only [--docker]

Verus install (macOS ARM64):
    Download from https://github.com/verus-lang/verus/releases
    Extract to ~/verus/, then: chmod +x ~/verus/verus
    Also: rustup toolchain install 1.94.0-aarch64-apple-darwin

This script:
  1. Hashes each source file
  2. Runs Verus on each (local binary or Docker)
  3. Sets ArtifactStatus = "Passed" iff all Verus runs exit 0
  4. Writes proof_artifacts/vessel_physics_manifest.json

The manifest is committed alongside the source. CI runs this script and
fails if status != "Passed". The RuntimePolicyEngine loads this file at
startup — ArtifactStatus::Passed is set by the Verus compiler, not by a
developer typing a string.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).parent.parent
VESSEL_SOURCE = ROOT / "verus_verified" / "vessel_registry.rs"
PROTOCOL_SOURCE = ROOT / "verus_verified" / "protocol_safety.rs"
OUT = ROOT / "proof_artifacts" / "vessel_physics_manifest.json"

VERUS_CANDIDATES = [
    str(Path.home() / "verus/verus"),
    "/usr/local/bin/verus",
    "verus",
]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip()
    except Exception:
        return "unknown"


def find_verus_bin() -> str | None:
    for candidate in VERUS_CANDIDATES:
        resolved = candidate
        if not Path(candidate).exists():
            try:
                result = subprocess.run(["which", candidate], capture_output=True, text=True)
                if result.returncode != 0:
                    continue
                resolved = result.stdout.strip()
            except Exception:
                continue
        # Smoke-test: a working verus binary reports its version without error
        try:
            probe = subprocess.run(
                [resolved, "--version"], capture_output=True, text=True, timeout=10
            )
            if probe.returncode == 0:
                return resolved
        except Exception:
            pass
    return None


def run_verus_on(source: Path, use_docker: bool) -> tuple[str, str, str]:
    """Run Verus on a single source file.

    Returns (status, stdout, stderr) where status is "Passed" or "Failed".
    """
    if use_docker:
        rel = source.relative_to(ROOT)
        cmd = [
            "docker", "run", "--rm",
            "-v", f"{ROOT}:/repo:ro",
            "ghcr.io/verus-lang/verus:latest",
            "verus", f"/repo/{rel}",
        ]
    else:
        verus_bin = find_verus_bin()
        if verus_bin is None:
            print(
                "WARNING: verus not found — tried: " + ", ".join(VERUS_CANDIDATES) + "\n"
                "  macOS ARM64: download from https://github.com/verus-lang/verus/releases\n"
                "               extract to ~/verus/, chmod +x ~/verus/verus\n"
                "  Docker:      use --docker flag",
                file=sys.stderr,
            )
            return "Failed", "", "verus not found"
        cmd = [verus_bin, str(source)]

    try:
        result = subprocess.run(cmd, capture_output=True, text=True)
        status = "Passed" if result.returncode == 0 else "Failed"
        return status, result.stdout, result.stderr
    except FileNotFoundError as e:
        print(f"WARNING: could not run verus: {e}", file=sys.stderr)
        return "Failed", "", str(e)


def sign_manifest(manifest: dict, private_key_path: str) -> dict:
    """Sign a manifest dict with Ed25519 and return a SignedProofManifest dict.

    The private key file must contain a base64-encoded 32-byte Ed25519 private key,
    as written by `cargo run -p proof_artifacts --bin keygen`.

    The signature is over the canonical JSON (sorted keys, no extra whitespace)
    of the manifest dict.  The returned dict has the shape:
        { "manifest": {...}, "signature": { "algorithm": "ed25519",
          "key_id": "...", "manifest_sha256": "...", "signature_b64": "..." } }
    """
    import base64
    import hashlib

    try:
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
    except ImportError:
        print(
            "ERROR: 'cryptography' package required for --sign.  Install with:\n"
            "  pip install cryptography",
            file=sys.stderr,
        )
        sys.exit(1)

    raw_b64 = Path(private_key_path).read_text().strip()
    raw_bytes = base64.b64decode(raw_b64)
    if len(raw_bytes) != 32:
        print(
            f"ERROR: private key must be 32 bytes (got {len(raw_bytes)}).  "
            "Regenerate with: cargo run -p proof_artifacts --bin keygen",
            file=sys.stderr,
        )
        sys.exit(1)

    private_key = Ed25519PrivateKey.from_private_bytes(raw_bytes)
    public_key = private_key.public_key()
    pub_bytes = public_key.public_bytes(Encoding.Raw, PublicFormat.Raw)
    key_id = base64.b64encode(pub_bytes).decode()

    canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
    digest = hashlib.sha256(canonical).hexdigest()
    sig_bytes = private_key.sign(canonical)
    sig_b64 = base64.b64encode(sig_bytes).decode()

    return {
        "manifest": manifest,
        "signature": {
            "algorithm": "ed25519",
            "key_id": key_id,
            "manifest_sha256": digest,
            "signature_b64": sig_b64,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--status-only", action="store_true",
                        help="Print Verus status and exit without writing manifest")
    parser.add_argument("--docker", action="store_true",
                        help="Run Verus via Docker")
    parser.add_argument(
        "--sign",
        metavar="PRIVATE_KEY_PATH",
        help="Sign the manifest with the given Ed25519 private key file (base64). "
             "The key is generated by: cargo run -p proof_artifacts --bin keygen. "
             "When provided, writes a SignedProofManifest (manifest + signature) "
             "instead of a raw ProofManifest.",
    )
    args = parser.parse_args()

    vessel_hash = sha256(VESSEL_SOURCE)
    protocol_hash = sha256(PROTOCOL_SOURCE)
    commit = git_commit()

    print(f"Verifying {VESSEL_SOURCE.name} ...", flush=True)
    vessel_status, vessel_stdout, vessel_stderr = run_verus_on(VESSEL_SOURCE, args.docker)
    print(f"  {vessel_status}: {vessel_stdout.strip()[:80] or '(no output)'}")

    print(f"Verifying {PROTOCOL_SOURCE.name} ...", flush=True)
    protocol_status, protocol_stdout, protocol_stderr = run_verus_on(PROTOCOL_SOURCE, args.docker)
    print(f"  {protocol_status}: {protocol_stdout.strip()[:80] or '(no output)'}")

    overall_status = "Passed" if (
        vessel_status == "Passed" and protocol_status == "Passed"
    ) else "Failed"

    if args.status_only:
        print(f"Overall Verus status: {overall_status}")
        return 0 if overall_status == "Passed" else 1

    workspace_hash = sha256(ROOT / "Cargo.toml") if (ROOT / "Cargo.toml").exists() else "n/a"

    manifest = {
        "schema_version": 1,
        "generated_unix_secs": int(time.time()),
        "build": {
            "git_commit": commit,
            "binary_hash": vessel_hash,       # primary source hash (vessel_registry.rs)
            "workspace_hash": workspace_hash,
            "container_image_digest": None,
            "device_id": None,
            "firmware_version": None,
        },
        "artifacts": [
            {
                "id": "lab_safety_verus",
                "source_path": "verus_verified/vessel_registry.rs",
                "source_hash": vessel_hash,
                "mir_path": None,
                "mir_hash": None,
                "lean": [],
                "verus": {
                    "path": "verus_verified/vessel_registry.rs",
                    "hash": vessel_hash,
                    "status": vessel_status,
                },
                "theorem_count": 11,
                "sorry_count": 0,
                "status": vessel_status,
                "metadata": {
                    "verus_stdout": vessel_stdout[:1000],
                    "generated_by": "vessel_physics/generate_manifest.py",
                },
            },
            {
                "id": "protocol_safety_verus",
                "source_path": "verus_verified/protocol_safety.rs",
                "source_hash": protocol_hash,
                "mir_path": None,
                "mir_hash": None,
                "lean": [],
                "verus": {
                    "path": "verus_verified/protocol_safety.rs",
                    "hash": protocol_hash,
                    "status": protocol_status,
                },
                "theorem_count": 13,
                "sorry_count": 0,
                "status": protocol_status,
                "metadata": {
                    "verus_stdout": protocol_stdout[:1000],
                    "generated_by": "vessel_physics/generate_manifest.py",
                },
            },
        ],
        "actions": [
            {
                "action": "dispense",
                "risk_class": "LiquidHandling",
                "required_artifacts": ["lab_safety_verus"],
                "rationale": "Liquid dispensing requires Verus-verified volume safety proof",
            },
            {
                "action": "aspirate",
                "risk_class": "LiquidHandling",
                "required_artifacts": ["lab_safety_verus"],
                "rationale": "Liquid aspiration requires Verus-verified volume safety proof",
            },
            {
                "action": "propose_protocol",
                "risk_class": "LiquidHandling",
                "required_artifacts": ["lab_safety_verus", "protocol_safety_verus"],
                "rationale": "Multi-step protocol execution requires both vessel physics and protocol-level safety proofs",
            },
            {
                "action": "read_absorbance",
                "risk_class": "ReadOnly",
                "required_artifacts": [],
                "rationale": "Read-only spectrophotometer measurement — no proof required",
            },
        ],
    }

    if args.sign:
        output = sign_manifest(manifest, args.sign)
        OUT.write_text(json.dumps(output, indent=2) + "\n")
        print(f"\nWritten (signed): {OUT.relative_to(ROOT)}  (status={overall_status})")
    else:
        OUT.write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"\nWritten (unsigned): {OUT.relative_to(ROOT)}  (status={overall_status})")
        print(
            "  NOTE: manifest is unsigned. Sign it for production use:\n"
            f"    python3 vessel_physics/generate_manifest.py "
            f"--sign ~/Documents/axiomlab_manifest_signing.private"
        )

    for label, stderr, status in [
        ("vessel_registry", vessel_stderr, vessel_status),
        ("protocol_safety", protocol_stderr, protocol_status),
    ]:
        if stderr and status == "Failed":
            print(f"\nVerus stderr ({label}):", stderr[:500], file=sys.stderr)

    return 0 if overall_status == "Passed" else 1


if __name__ == "__main__":
    sys.exit(main())
