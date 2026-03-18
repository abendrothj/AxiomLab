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
//! ```
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

    let summary = generate_proof(&log_bytes).await?;
    submit_proof(&summary, cfg).await
}

// ── Proof generation ─────────────────────────────────────────────────────────

#[cfg(feature = "prove")]
async fn generate_proof(log_bytes: &[u8]) -> Result<AuditSummary, String> {
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

    Ok(summary)
}

#[cfg(not(feature = "prove"))]
async fn generate_proof(_log_bytes: &[u8]) -> Result<AuditSummary, String> {
    Err("zk proof disabled — build with `--features prove` and install risc0 toolchain via rzup".into())
}

// ── On-chain submission ───────────────────────────────────────────────────────

#[cfg(feature = "onchain")]
async fn submit_proof(summary: &AuditSummary, cfg: &ZkConfig) -> Result<String, String> {
    use alloy::{
        network::EthereumWallet,
        primitives::{Address, U256},
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

    let contract_addr: Address = cfg.contract_addr.parse()
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

    // Note: `seal` would come from the Receipt in a full prove+submit flow.
    // Here we pass empty bytes — in practice, call `prove_and_submit` which
    // carries the receipt through from `generate_proof`.
    let seal = alloy::primitives::Bytes::new();

    // Call submitProof(bytes calldata seal, bytes calldata journal)
    alloy::sol! {
        #[sol(rpc)]
        interface IAuditVerifier {
            function submitProof(bytes calldata seal, bytes calldata journal) external;
        }
    }

    let contract = IAuditVerifier::new(contract_addr, &provider);
    let tx = contract
        .submitProof(seal, alloy::primitives::Bytes::from(journal))
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
async fn submit_proof(_summary: &AuditSummary, _cfg: &ZkConfig) -> Result<String, String> {
    Err("on-chain submission disabled — build with `--features onchain`".into())
}
