//! `tokengen` — generate a signed AxiomLab JWT for operator authentication.
//!
//! # Usage
//! ```
//! cargo run -p agent_runtime --bin tokengen -- \
//!   --operator-id alice \
//!   --role operator \
//!   --ttl-hours 8
//! ```
//!
//! # Environment
//! `AXIOMLAB_JWT_SECRET` — base64-encoded shared secret (same as the server).
//! `AXIOMLAB_OPERATOR_ID` — default operator ID (overridden by --operator-id).
//! `AXIOMLAB_OPERATOR_ROLE` — default role (overridden by --role).
//!
//! Prints the Bearer token to stdout.  Use it in API calls:
//! ```
//! TOKEN=$(cargo run -p agent_runtime --bin tokengen -- --operator-id alice --role operator)
//! curl -X POST http://localhost:3000/api/emergency-stop \
//!   -H "Authorization: Bearer $TOKEN"
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut operator_id = std::env::var("AXIOMLAB_OPERATOR_ID")
        .unwrap_or_else(|_| "operator".into());
    let mut role = std::env::var("AXIOMLAB_OPERATOR_ROLE")
        .unwrap_or_else(|_| "operator".into());
    let mut ttl_hours: u64 = 8;

    // Minimal arg parser — avoids adding a CLI framework dep.
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--operator-id" | "-u" => {
                i += 1;
                if let Some(v) = args.get(i) { operator_id = v.clone(); }
            }
            "--role" | "-r" => {
                i += 1;
                if let Some(v) = args.get(i) { role = v.clone(); }
            }
            "--ttl-hours" | "-t" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    ttl_hours = v.parse().unwrap_or_else(|_| {
                        eprintln!("Invalid --ttl-hours value '{v}', using 8");
                        8
                    });
                }
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {other}");
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Validate role.
    if !matches!(role.as_str(), "operator" | "pi" | "machine") {
        eprintln!("Error: --role must be one of: operator, pi, machine");
        std::process::exit(1);
    }

    // Load secret.
    let secret_b64 = std::env::var("AXIOMLAB_JWT_SECRET").unwrap_or_else(|_| {
        eprintln!(
            "Error: AXIOMLAB_JWT_SECRET is not set.\n\
             Generate a secret with:\n\
             openssl rand -base64 32 | tee .axiomlab_jwt_secret"
        );
        std::process::exit(1);
    });
    let secret = STANDARD.decode(secret_b64.trim()).unwrap_or_else(|e| {
        eprintln!("Error: AXIOMLAB_JWT_SECRET is not valid base64: {e}");
        std::process::exit(1);
    });
    if secret.len() < 16 {
        eprintln!("Error: AXIOMLAB_JWT_SECRET is too short (must be ≥16 bytes when decoded)");
        std::process::exit(1);
    }

    // Build claims.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    let exp = now + ttl_hours * 3600;

    let header = r#"{"alg":"HS256","typ":"JWT"}"#;
    let payload = format!(
        r#"{{"sub":"{operator_id}","role":"{role}","iat":{now},"exp":{exp}}}"#
    );

    let header_b64 = STANDARD.encode(header);
    let payload_b64 = STANDARD.encode(payload);

    // URL-safe base64 without padding (standard JWT encoding).
    let header_b64u  = to_base64url(&header_b64);
    let payload_b64u = to_base64url(&payload_b64);

    let signing_input = format!("{header_b64u}.{payload_b64u}");

    // HMAC-SHA256.
    let signature = hmac_sha256(&secret, signing_input.as_bytes());
    let sig_b64u  = to_base64url(&STANDARD.encode(&signature));

    let token = format!("{signing_input}.{sig_b64u}");

    // Print the token and a ready-to-use curl header.
    println!("{token}");
    eprintln!(
        "Token generated for operator_id={operator_id} role={role} ttl={ttl_hours}h\n\
         Paste into Authorization header:\n  Authorization: Bearer {token}"
    );
}

fn print_usage() {
    eprintln!(
        "Usage: tokengen [OPTIONS]\n\
         \n\
         Options:\n  \
         --operator-id, -u  Operator identifier (default: $AXIOMLAB_OPERATOR_ID or 'operator')\n  \
         --role, -r         Role: operator|pi|machine (default: operator)\n  \
         --ttl-hours, -t    Token validity in hours (default: 8)\n  \
         --help, -h         Show this message\n\
         \n\
         Environment:\n  \
         AXIOMLAB_JWT_SECRET  Base64-encoded shared secret (required)"
    );
}

fn to_base64url(b64: &str) -> String {
    b64.replace('+', "-").replace('/', "_").replace('=', "")
}

// Minimal HMAC-SHA256 implementation using sha2 (workspace dep).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    // Key padding/hashing per HMAC spec.
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let hash = Sha256::digest(key);
        k[..32].copy_from_slice(&hash);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    // i_key_pad XOR 0x36, o_key_pad XOR 0x5c.
    let mut i_key_pad = [0u8; 64];
    let mut o_key_pad = [0u8; 64];
    for (i, &b) in k.iter().enumerate() {
        i_key_pad[i] = b ^ 0x36;
        o_key_pad[i] = b ^ 0x5c;
    }

    let inner_hash = {
        let mut h = Sha256::new();
        h.update(&i_key_pad);
        h.update(msg);
        h.finalize()
    };

    let mut outer = Sha256::new();
    outer.update(&o_key_pad);
    outer.update(&inner_hash);
    outer.finalize().into()
}
