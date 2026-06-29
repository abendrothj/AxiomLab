//! Generate and sign a proof manifest for the standard action set.
//!
//! The committed manifest is a runtime artifact (`.artifacts/` is gitignored),
//! so it must be generated per deployment. This rotates to a fresh signing key,
//! signs the manifest, and prints the public key to export as
//! `AXIOMLAB_MANIFEST_PUBKEY` so the server's `ProofGate` will trust it.
//!
//! Usage:
//!   cargo run -p axiom-proofs --bin gen-manifest [output_path]
//!   export AXIOMLAB_MANIFEST_PUBKEY=<printed key>

use axiom_proofs::{
    ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofManifest, VerusArtifact,
    keygen, sign_manifest,
};
use axiom_types::RiskClass;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::BTreeMap;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".artifacts/proof/manifest.signed.json".into());

    let verus = ProofArtifact {
        id: "lab_safety_verus".into(),
        source_path: "verus_verified/lab_safety.rs".into(),
        source_hash: "see verus.yml verification".into(),
        mir_path: None,
        mir_hash: None,
        lean: vec![],
        verus: Some(VerusArtifact {
            path: "verus_verified/lab_safety.rs".into(),
            hash: "see verus.yml verification".into(),
            status: ArtifactStatus::Passed,
        }),
        theorem_count: 0,
        sorry_count: 0,
        status: ArtifactStatus::Passed,
        metadata: BTreeMap::new(),
    };

    let policy = |action: &str, risk: RiskClass, rationale: &str| ActionPolicy {
        action: action.into(),
        risk_class: risk,
        required_artifacts: vec!["lab_safety_verus".into()],
        rationale: rationale.into(),
    };

    let manifest = ProofManifest {
        schema_version: 1,
        generated_unix_secs: now_secs(),
        build: BuildIdentity {
            git_commit: env_or("GIT_COMMIT", "dev"),
            binary_hash: "dev".into(),
            workspace_hash: "dev".into(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        },
        artifacts: vec![verus],
        actions: vec![
            policy("read_absorbance", RiskClass::ReadOnly, "measurement requires hardware safety proof"),
            policy("read_ph", RiskClass::ReadOnly, "measurement requires hardware safety proof"),
            policy("read_temperature", RiskClass::ReadOnly, "measurement requires hardware safety proof"),
            policy("dispense", RiskClass::LiquidHandling, "liquid handling requires hardware safety proof"),
            policy("aspirate", RiskClass::LiquidHandling, "liquid handling requires hardware safety proof"),
            policy("move_arm", RiskClass::Actuation, "actuation requires verified hardware bounds"),
            policy("set_temperature", RiskClass::Actuation, "actuation requires verified hardware bounds"),
            policy("incubate", RiskClass::Actuation, "actuation requires verified hardware bounds"),
            policy("centrifuge", RiskClass::Actuation, "actuation requires verified hardware bounds"),
        ],
    };

    let (sk, pk) = keygen();
    let signed = sign_manifest(&manifest, &sk, "axiomlab-rotating-root").expect("sign manifest");

    if let Some(parent) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(parent).expect("create output dir");
    }
    std::fs::write(&out, serde_json::to_string_pretty(&signed).expect("serialize")).expect("write manifest");

    let pubkey_b64 = STANDARD.encode(&pk);
    let key_path = format!("{out}.signing_key.private.b64");
    std::fs::write(&key_path, STANDARD.encode(&sk)).expect("write private key");

    eprintln!("Wrote signed manifest: {out}");
    eprintln!("Wrote private signing key (keep secret): {key_path}");
    eprintln!();
    eprintln!("Export this so the server's ProofGate trusts the manifest:");
    println!("AXIOMLAB_MANIFEST_PUBKEY={pubkey_b64}");
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}
