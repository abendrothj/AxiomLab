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

/// Identifies the intended use case for the ZK proof.
///
/// The ZK proof does NOT replace Rekor (which provides public timestamping).
/// It adds *content confidentiality* — you can prove audit chain properties
/// (event count, violation count, chain validity) without revealing what any
/// individual event contained.
///
/// # Use cases
///
/// * [`ZkUseCase::ConfidentialRegulatory`] — prove chain integrity to regulators
///   without disclosing proprietary experiment details.  Useful when sharing with
///   external audit bodies under NDA or in patent-pending research.
///
/// * [`ZkUseCase::ConfidentialAudit`] — prove compliance to a contract research
///   sponsor without granting full log access.  The sponsor verifies the proof
///   receipt on-chain; the raw log stays confidential.
///
/// # Distinction from Rekor
/// Rekor checkpointing (via `anchor_chain_tip_to_rekor`) provides an immutable
/// public timestamp for the chain tip.  The ZK proof adds the ability to make
/// quantitative claims about the chain *contents* (e.g. "0 violations in 127
/// events") without any content disclosure.  Both mechanisms complement each other.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZkUseCase {
    /// Prove chain integrity to regulators without revealing experiment details.
    /// Use case: IP protection during collaborative research or patent review.
    ConfidentialRegulatory,
    /// Prove chain integrity to a counter-party without sharing the log.
    /// Use case: contract research org proving compliance to sponsor.
    ConfidentialAudit,
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
    /// Intended use case — determines how the proof receipt is presented to
    /// verifiers.  Defaults to `ConfidentialAudit` when not explicitly set.
    pub use_case: ZkUseCase,
}

impl ZkConfig {
    /// Load from environment variables.  Returns `None` if any variable is unset.
    pub fn from_env() -> Option<Self> {
        let base_rpc_url  = std::env::var("AXIOMLAB_BASE_RPC_URL").ok()?;
        let contract_addr = std::env::var("AXIOMLAB_BASE_CONTRACT_ADDR").ok()?;
        let wallet_key    = std::env::var("AXIOMLAB_BASE_WALLET_KEY").ok()?;
        let use_case = std::env::var("AXIOMLAB_ZK_USE_CASE").ok()
            .and_then(|v| match v.as_str() {
                "confidential_regulatory" => Some(ZkUseCase::ConfidentialRegulatory),
                "confidential_audit"      => Some(ZkUseCase::ConfidentialAudit),
                _                         => None,
            })
            .unwrap_or(ZkUseCase::ConfidentialAudit);
        Some(Self { base_rpc_url, contract_addr, wallet_key, use_case })
    }
}
