//! ZK guest program: verifies the AxiomLab audit log hash chain inside the
//! RISC Zero zkVM.
//!
//! # Inputs (private — never revealed on-chain)
//! Raw JSONL bytes of the audit log, read via `risc0_zkvm::guest::env::read`.
//!
//! # Outputs (public — committed to journal)
//! [`AuditSummary`]: chain validity, event count, violation count, tip hash,
//! and timestamp range.  Only this summary is sent on-chain; zero content from
//! the audit log is disclosed.
//!
//! # Verification
//! ```sh
//! # After the proof is generated, anyone can verify it locally:
//! risc0-client verify --receipt receipt.bin
//! # Or check on Base:
//! cast call $CONTRACT_ADDR "latestTipHash()(bytes32)" --rpc-url $BASE_RPC_URL
//! ```

#![no_main]
risc0_zkvm::guest::entry!(main);

use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

/// What the guest commits to the journal — the only data that appears on-chain.
#[derive(Serialize, Deserialize)]
struct AuditSummary {
    chain_valid:    bool,
    event_count:    u64,
    violation_count: u64,
    /// SHA-256 of the last audit entry (hex-encoded).
    tip_hash:       [u8; 32],
    first_unix_secs: u64,
    last_unix_secs:  u64,
}

fn main() {
    // Read the full audit log from the private prover input.
    let log_bytes: Vec<u8> = risc0_zkvm::guest::env::read();
    let log = core::str::from_utf8(&log_bytes).expect("audit log must be valid UTF-8");

    let mut event_count:    u64 = 0;
    let mut violation_count: u64 = 0;
    let mut chain_valid = true;
    let mut prev_hash: Option<String> = None;
    let mut tip_hash   = [0u8; 32];
    let mut first_unix_secs: u64 = 0;
    let mut last_unix_secs:  u64 = 0;

    for line in log.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                chain_valid = false;
                continue;
            }
        };

        // Verify hash-chain linkage.
        let this_prev = entry["prev_hash"].as_str().unwrap_or("");
        if let Some(ref expected_prev) = prev_hash {
            if this_prev != expected_prev {
                chain_valid = false;
            }
        }

        // Compute this entry's hash.
        let entry_hash_str = entry["entry_hash"].as_str().unwrap_or("");
        let computed = {
            let mut h = Sha256::new();
            h.update(line.as_bytes());
            hex::encode(h.finalize())
        };
        if !entry_hash_str.is_empty() && computed != entry_hash_str {
            chain_valid = false;
        }

        // Decode tip hash.
        if let Ok(bytes) = hex::decode(entry_hash_str) {
            if bytes.len() == 32 {
                tip_hash.copy_from_slice(&bytes);
            }
        }

        // Update prev_hash for next iteration.
        prev_hash = Some(entry_hash_str.to_owned());

        // Track timestamps.
        let ts = entry["unix_secs"].as_u64().unwrap_or(0);
        if event_count == 0 {
            first_unix_secs = ts;
        }
        last_unix_secs = ts;

        // Count violations.
        if entry["decision"].as_str() == Some("deny") {
            violation_count += 1;
        }

        event_count += 1;
    }

    risc0_zkvm::guest::env::commit(&AuditSummary {
        chain_valid,
        event_count,
        violation_count,
        tip_hash,
        first_unix_secs,
        last_unix_secs,
    });
}
