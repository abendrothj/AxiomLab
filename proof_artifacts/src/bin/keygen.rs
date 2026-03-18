//! Generates an Ed25519 manifest-signing keypair for AxiomLab.
//!
//! Writes the private key (base64) to ~/Documents/axiomlab_manifest_signing.private
//! with permissions 0o600. Prints the public key (base64) to stdout.
//!
//! After running this binary:
//!   1. Copy the printed public key
//!   2. Paste it into `proof_artifacts/src/signature.rs` as `MANIFEST_SIGNING_PUBLIC_KEY`
//!   3. Re-sign the manifest:
//!      python3 vessel_physics/generate_manifest.py --sign ~/Documents/axiomlab_manifest_signing.private
//!
//! Usage:
//!   cargo run -p proof_artifacts --bin keygen

use base64::{Engine as _, engine::general_purpose::STANDARD};
use proof_artifacts::signature::keygen;
use std::path::PathBuf;

fn main() {
    let (sk, pk) = keygen();
    let sk_b64 = STANDARD.encode(&sk);
    let pk_b64 = STANDARD.encode(&pk);

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    let key_path = PathBuf::from(&home)
        .join("Documents")
        .join("axiomlab_manifest_signing.private");

    if key_path.exists() {
        eprintln!(
            "ERROR: {} already exists.",
            key_path.display()
        );
        eprintln!("Delete it first if you intentionally want to rotate the signing key.");
        eprintln!("WARNING: rotating the key invalidates all previously signed manifests.");
        std::process::exit(1);
    }

    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("Failed to create {}: {}", parent.display(), e);
            std::process::exit(1);
        });
    }

    std::fs::write(&key_path, format!("{}\n", sk_b64)).unwrap_or_else(|e| {
        eprintln!("Failed to write private key to {}: {}", key_path.display(), e);
        std::process::exit(1);
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|e| {
                eprintln!(
                    "Warning: could not set 0o600 on {}: {}",
                    key_path.display(),
                    e
                );
            });
    }

    eprintln!("Private key written to: {}", key_path.display());
    eprintln!();
    eprintln!("Next steps:");
    eprintln!("  1. Paste the public key below into proof_artifacts/src/signature.rs");
    eprintln!("     as: pub const MANIFEST_SIGNING_PUBLIC_KEY: &str = \"<key>\";");
    eprintln!("  2. Re-sign the manifest:");
    eprintln!(
        "     python3 vessel_physics/generate_manifest.py --sign {}",
        key_path.display()
    );
    eprintln!();
    eprintln!("Public key (base64):");
    println!("{}", pk_b64);
}
