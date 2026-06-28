//! The authoritative record of everything AxiomLab does.
//!
//! Two responsibilities, one crate:
//!
//! 1. [`Chain`] — an append-only log of Ed25519-signed entries. Each entry hashes
//!    the previous (a hash chain) and is signed over `(entry_data || prev_hash)`.
//!    [`Chain::verify`] walks the whole chain checking every hash link and every
//!    signature; any break is a hard error. **This chain is the system of record**
//!    — there is no separate database of runs or findings.
//!
//! 2. [`RekorClient`] — anchors the chain-tip hash in Sigstore's public
//!    transparency log. Default-on; set `AXIOMLAB_REKOR_DISABLED=1` to skip.
//!
//! Signing defaults to AWS KMS when `AXIOMLAB_KMS_KEY_ID` is set (and the `kms`
//! feature is compiled); a local Ed25519 key is the fallback.

mod chain;
mod rekor;
mod revocation;
mod signer;

pub use chain::{Chain, ChainEntry, ChainError, EntryData, VerifyResult};
pub use rekor::{LogId, RekorClient, RekorError};
pub use revocation::RevocationList;
pub use signer::{LocalSigner, Signer, signer_from_env};

#[cfg(feature = "kms")]
pub use signer::KmsSigner;
