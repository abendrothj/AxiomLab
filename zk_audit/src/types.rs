//! Shared types between the ZK guest and the host prover.

use serde::{Deserialize, Serialize};

/// The public output committed to the ZK proof journal.
///
/// This is the only data that goes on-chain — zero audit log content is
/// disclosed.  Anyone holding the proof receipt can verify that:
/// - The hash chain was intact at the time of proving.
/// - There were `event_count` events with `violation_count` denials.
/// - The chain tip matched `tip_hash` at `last_unix_secs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSummary {
    /// `true` if every `prev_hash → entry_hash` link was verified.
    pub chain_valid: bool,
    /// Total number of audit events in the log.
    pub event_count: u64,
    /// Number of events with `decision: "deny"`.
    pub violation_count: u64,
    /// SHA-256 of the last audit entry (raw 32 bytes).
    pub tip_hash: [u8; 32],
    /// Unix seconds of the first audit event.
    pub first_unix_secs: u64,
    /// Unix seconds of the last audit event.
    pub last_unix_secs: u64,
}

/// Environment variables that enable ZK anchoring.
pub struct ZkConfig {
    /// Base L2 RPC endpoint (e.g. Alchemy or Infura Base endpoint).
    /// Required env var: `AXIOMLAB_BASE_RPC_URL`
    pub base_rpc_url: String,
    /// Deployed `AuditVerifier` contract address on Base.
    /// Required env var: `AXIOMLAB_BASE_CONTRACT_ADDR`
    pub contract_addr: String,
    /// Hex-encoded private key for submitting transactions (funded with ETH on Base).
    /// Required env var: `AXIOMLAB_BASE_WALLET_KEY`
    pub wallet_key: String,
}

impl ZkConfig {
    /// Load from environment variables.  Returns `None` if any variable is unset.
    pub fn from_env() -> Option<Self> {
        let base_rpc_url  = std::env::var("AXIOMLAB_BASE_RPC_URL").ok()?;
        let contract_addr = std::env::var("AXIOMLAB_BASE_CONTRACT_ADDR").ok()?;
        let wallet_key    = std::env::var("AXIOMLAB_BASE_WALLET_KEY").ok()?;
        Some(Self { base_rpc_url, contract_addr, wallet_key })
    }
}
