//! Zero-knowledge audit proof generation and Base L2 anchoring.
//!
//! # Feature flags
//! - `prove`   — enables actual RISC Zero proof generation (requires `rzup`)
//! - `onchain` — enables Base L2 submission via `alloy`
//!
//! Without either feature, `prove_and_submit` is a no-op that returns the
//! error string `"zk proof disabled"`.  This lets the runtime compile and run
//! without the ZK toolchain installed.
//!
//! # Operator setup
//! 1. Install the RISC Zero toolchain: `curl -L https://risczero.com/install | bash && rzup`
//! 2. Set env vars:
//!    - `AXIOMLAB_BASE_RPC_URL`      — Base L2 RPC endpoint
//!    - `AXIOMLAB_BASE_CONTRACT_ADDR` — deployed AuditVerifier address
//!    - `AXIOMLAB_BASE_WALLET_KEY`    — funded wallet private key (hex)
//! 3. Build with: `cargo build --features prove,onchain`
//!
//! # Verification
//! After each protocol conclusion, the Base transaction is logged at INFO:
//! ```text
//! ZK audit proof submitted: https://basescan.org/tx/0xabc...
//! ```
//! The `AuditProofVerified` event on-chain contains the chain tip hash,
//! event count, and violation count — verifiable by anyone, zero content disclosed.

pub mod types;
pub use types::{AuditSummary, ZkConfig};

use std::fs;
#[allow(unused_imports)]
use tracing::{error, info, warn};

// ── ELF bytes for the guest ───────────────────────────────────────────────────

/// The compiled guest ELF, embedded at build time when `prove` is enabled.
///
/// Build the guest first:
/// ```sh
/// cargo build -p zk_audit_guest --target riscv32im-risc0-zkvm-elf --release
/// ```
#[cfg(feature = "prove")]
const AUDIT_GUEST_ELF: &[u8] =
    include_bytes!("../../target/riscv32im-risc0-zkvm-elf/release/zk_audit_guest");

// ── Public API ───────────────────────────────────────────────────────────────

/// Generate a ZK proof over the audit log and submit it to the Base L2
/// verifier contract.
///
/// # Arguments
/// - `audit_log_path` — path to the JSONL audit log
/// - `cfg`            — Base L2 connection config; use [`ZkConfig::from_env`]
///
/// # Returns
/// `Ok(tx_hash)` on success, `Err(reason)` on failure or if features are disabled.
///
/// This function runs synchronously.  Callers should `tokio::spawn` it to avoid
/// blocking the protocol conclusion path.
pub async fn prove_and_submit(audit_log_path: &str, cfg: &ZkConfig) -> Result<String, String> {
    let log_bytes = fs::read(audit_log_path)
        .map_err(|e| format!("failed to read audit log: {e}"))?;

    let (summary, seal) = generate_proof(&log_bytes).await?;
    submit_proof(&summary, &seal, cfg).await
}

// ── Proof generation ─────────────────────────────────────────────────────────

#[cfg(feature = "prove")]
async fn generate_proof(log_bytes: &[u8]) -> Result<(AuditSummary, Vec<u8>), String> {
    use risc0_zkvm::{default_prover, ExecutorEnv};

    let env = ExecutorEnv::builder()
        .write(log_bytes)
        .map_err(|e| format!("zkvm env error: {e}"))?
        .build()
        .map_err(|e| format!("zkvm env build: {e}"))?;

    let prover = default_prover();
    let receipt = prover
        .prove(env, AUDIT_GUEST_ELF)
        .map_err(|e| format!("zkvm prove: {e}"))?
        .receipt;

    let seal = receipt.inner.seal()
        .map_err(|e| format!("seal extract: {e}"))?;

    let summary: AuditSummary = receipt
        .journal
        .decode()
        .map_err(|e| format!("journal decode: {e}"))?;

    info!(
        event_count     = summary.event_count,
        violation_count = summary.violation_count,
        chain_valid     = summary.chain_valid,
        tip_hash        = %hex::encode(summary.tip_hash),
        "ZK proof generated"
    );

    if !summary.chain_valid {
        return Err("ZK proof generated but chain is INVALID — audit log may be tampered".into());
    }

    Ok((summary, seal))
}

#[cfg(not(feature = "prove"))]
async fn generate_proof(_log_bytes: &[u8]) -> Result<(AuditSummary, Vec<u8>), String> {
    Err("zk proof disabled — build with `--features prove` and install risc0 toolchain via rzup".into())
}

// ── On-chain submission ───────────────────────────────────────────────────────

#[cfg(feature = "onchain")]
async fn submit_proof(summary: &AuditSummary, seal: &[u8], cfg: &ZkConfig) -> Result<String, String> {
    use alloy::{
        network::EthereumWallet,
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
    };

    let signer: PrivateKeySigner = cfg.wallet_key.parse()
        .map_err(|e| format!("wallet key parse: {e}"))?;
    let wallet = EthereumWallet::from(signer);

    let rpc_url = cfg.base_rpc_url.parse()
        .map_err(|e| format!("rpc url parse: {e}"))?;
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_http(rpc_url);

    let contract_addr: alloy::primitives::Address = cfg.contract_addr.parse()
        .map_err(|e| format!("contract addr parse: {e}"))?;

    // ABI-encode the AuditSummary as (bool,uint64,uint64,bytes32,uint64,uint64).
    let journal = alloy::sol_types::abi::encode(&(
        summary.chain_valid,
        summary.event_count,
        summary.violation_count,
        alloy::primitives::FixedBytes::<32>::from(summary.tip_hash),
        summary.first_unix_secs,
        summary.last_unix_secs,
    ));

    // Call submitProof(bytes calldata seal, bytes calldata journal)
    alloy::sol! {
        #[sol(rpc)]
        interface IAuditVerifier {
            function submitProof(bytes calldata seal, bytes calldata journal) external;
        }
    }

    let contract = IAuditVerifier::new(contract_addr, &provider);
    let tx = contract
        .submitProof(
            alloy::primitives::Bytes::copy_from_slice(seal),
            alloy::primitives::Bytes::from(journal),
        )
        .send()
        .await
        .map_err(|e| format!("submitProof tx error: {e}"))?;

    let receipt = tx.get_receipt().await
        .map_err(|e| format!("tx receipt error: {e}"))?;

    let tx_hash = format!("{:?}", receipt.transaction_hash);
    info!(
        tx_hash = %tx_hash,
        "ZK audit proof submitted to Base — https://basescan.org/tx/{tx_hash}"
    );

    Ok(tx_hash)
}

#[cfg(not(feature = "onchain"))]
async fn submit_proof(_summary: &AuditSummary, _seal: &[u8], _cfg: &ZkConfig) -> Result<String, String> {
    Err("on-chain submission disabled — build with `--features onchain`".into())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ZkUseCase;

    // ── Type serialization ────────────────────────────────────────────────────

    #[test]
    fn audit_summary_round_trips_json() {
        let s = AuditSummary {
            chain_valid:     true,
            event_count:     42,
            violation_count: 1,
            tip_hash:        [0xab; 32],
            first_unix_secs: 1_700_000_000,
            last_unix_secs:  1_700_001_000,
        };
        let json = serde_json::to_string(&s).unwrap();
        let decoded: AuditSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.chain_valid,     s.chain_valid);
        assert_eq!(decoded.event_count,     s.event_count);
        assert_eq!(decoded.violation_count, s.violation_count);
        assert_eq!(decoded.tip_hash,        s.tip_hash);
        assert_eq!(decoded.first_unix_secs, s.first_unix_secs);
        assert_eq!(decoded.last_unix_secs,  s.last_unix_secs);
    }

    #[test]
    fn zk_use_case_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&ZkUseCase::ConfidentialRegulatory).unwrap(),
            "\"confidential_regulatory\""
        );
        assert_eq!(
            serde_json::to_string(&ZkUseCase::ConfidentialAudit).unwrap(),
            "\"confidential_audit\""
        );
    }

    #[test]
    fn zk_use_case_round_trips() {
        for case in [ZkUseCase::ConfidentialRegulatory, ZkUseCase::ConfidentialAudit] {
            let s = serde_json::to_string(&case).unwrap();
            let decoded: ZkUseCase = serde_json::from_str(&s).unwrap();
            assert_eq!(decoded, case);
        }
    }

    // ── ZkConfig::from_env ────────────────────────────────────────────────────
    // NOTE: env var mutation is inherently racy in a multi-threaded test runner.
    // Run these with `cargo test -- --test-threads=1` if tests interfere.

    #[test]
    fn zk_config_from_env_returns_none_when_vars_absent() {
        // SAFETY: single-threaded test; no other thread reads these vars.
        unsafe {
            std::env::remove_var("AXIOMLAB_BASE_RPC_URL");
            std::env::remove_var("AXIOMLAB_BASE_CONTRACT_ADDR");
            std::env::remove_var("AXIOMLAB_BASE_WALLET_KEY");
        }
        assert!(ZkConfig::from_env().is_none());
    }

    #[test]
    fn zk_config_from_env_returns_some_when_all_vars_present() {
        // SAFETY: single-threaded test; no other thread reads these vars.
        unsafe {
            std::env::set_var("AXIOMLAB_BASE_RPC_URL",       "https://base-rpc.example.com");
            std::env::set_var("AXIOMLAB_BASE_CONTRACT_ADDR", "0xDeaD");
            std::env::set_var("AXIOMLAB_BASE_WALLET_KEY",    "0xBEEF");
        }
        let cfg = ZkConfig::from_env();
        unsafe {
            std::env::remove_var("AXIOMLAB_BASE_RPC_URL");
            std::env::remove_var("AXIOMLAB_BASE_CONTRACT_ADDR");
            std::env::remove_var("AXIOMLAB_BASE_WALLET_KEY");
        }
        let cfg = cfg.unwrap();
        assert_eq!(cfg.base_rpc_url,  "https://base-rpc.example.com");
        assert_eq!(cfg.contract_addr, "0xDeaD");
        assert_eq!(cfg.wallet_key,    "0xBEEF");
        assert_eq!(cfg.use_case, ZkUseCase::ConfidentialAudit); // default
    }

    #[test]
    fn zk_config_parses_use_case_env_var() {
        // SAFETY: single-threaded test; no other thread reads these vars.
        unsafe {
            std::env::set_var("AXIOMLAB_BASE_RPC_URL",       "https://x");
            std::env::set_var("AXIOMLAB_BASE_CONTRACT_ADDR", "0x1");
            std::env::set_var("AXIOMLAB_BASE_WALLET_KEY",    "0x2");
            std::env::set_var("AXIOMLAB_ZK_USE_CASE",        "confidential_regulatory");
        }
        let cfg = ZkConfig::from_env();
        unsafe {
            std::env::remove_var("AXIOMLAB_BASE_RPC_URL");
            std::env::remove_var("AXIOMLAB_BASE_CONTRACT_ADDR");
            std::env::remove_var("AXIOMLAB_BASE_WALLET_KEY");
            std::env::remove_var("AXIOMLAB_ZK_USE_CASE");
        }
        assert_eq!(cfg.unwrap().use_case, ZkUseCase::ConfidentialRegulatory);
    }

    #[test]
    fn zk_config_unknown_use_case_falls_back_to_default() {
        // SAFETY: single-threaded test; no other thread reads these vars.
        unsafe {
            std::env::set_var("AXIOMLAB_BASE_RPC_URL",       "https://x");
            std::env::set_var("AXIOMLAB_BASE_CONTRACT_ADDR", "0x1");
            std::env::set_var("AXIOMLAB_BASE_WALLET_KEY",    "0x2");
            std::env::set_var("AXIOMLAB_ZK_USE_CASE",        "not_a_real_case");
        }
        let cfg = ZkConfig::from_env();
        unsafe {
            std::env::remove_var("AXIOMLAB_BASE_RPC_URL");
            std::env::remove_var("AXIOMLAB_BASE_CONTRACT_ADDR");
            std::env::remove_var("AXIOMLAB_BASE_WALLET_KEY");
            std::env::remove_var("AXIOMLAB_ZK_USE_CASE");
        }
        assert_eq!(cfg.unwrap().use_case, ZkUseCase::ConfidentialAudit);
    }

    // ── Disabled feature stubs ────────────────────────────────────────────────

    #[tokio::test]
    async fn prove_and_submit_returns_err_when_features_disabled() {
        // Without `prove` + `onchain` features, both steps return Err.
        // Reading a non-existent path exercises the fs::read failure first.
        let cfg = ZkConfig {
            base_rpc_url:  String::new(),
            contract_addr: String::new(),
            wallet_key:    String::new(),
            use_case:      ZkUseCase::ConfidentialAudit,
        };
        let result = prove_and_submit("/tmp/axiomlab_nonexistent_audit.jsonl", &cfg).await;
        assert!(result.is_err());
    }
}
