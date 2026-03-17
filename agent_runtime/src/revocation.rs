//! Revocation list for approval IDs and signing key IDs.
//!
//! Once an operator key is compromised or an approval bundle is known to have
//! been issued under duress, operators add the relevant ID to the revocation
//! list.  The orchestrator and approval validator check this list before
//! accepting any signed approval.
//!
//! # Loading
//!
//! Set `AXIOMLAB_REVOCATION_LIST` to a JSON object:
//! ```json
//! {
//!   "key_ids":      ["key-id-1", "key-id-2"],
//!   "approval_ids": ["approval-uuid-abc", "approval-uuid-def"]
//! }
//! ```
//!
//! In production, this should be served from a configuration management
//! system (Vault, AWS SSM Parameter Store, etc.) and refreshed periodically.

use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct RevocationList {
    /// Revoked signing key IDs (the `key_id` field in `ManifestSignature` /
    /// the `approver_id` in `ApprovalStatement`).
    revoked_key_ids: HashSet<String>,
    /// Revoked individual approval IDs.
    revoked_approval_ids: HashSet<String>,
}

#[derive(Deserialize)]
struct RevocationPayload {
    #[serde(default)]
    key_ids: Vec<String>,
    #[serde(default)]
    approval_ids: Vec<String>,
}

impl RevocationList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from `AXIOMLAB_REVOCATION_LIST` environment variable.
    pub fn from_env() -> Self {
        let Ok(raw) = std::env::var("AXIOMLAB_REVOCATION_LIST") else {
            return Self::default();
        };
        match serde_json::from_str::<RevocationPayload>(&raw) {
            Ok(payload) => Self {
                revoked_key_ids: payload.key_ids.into_iter().collect(),
                revoked_approval_ids: payload.approval_ids.into_iter().collect(),
            },
            Err(e) => {
                tracing::warn!("AXIOMLAB_REVOCATION_LIST parse error — no revocations loaded: {e}");
                Self::default()
            }
        }
    }

    /// Revoke a signing key ID (affects all approvals signed with that key).
    pub fn revoke_key(&mut self, key_id: impl Into<String>) {
        self.revoked_key_ids.insert(key_id.into());
    }

    /// Revoke a specific approval ID.
    pub fn revoke_approval(&mut self, approval_id: impl Into<String>) {
        self.revoked_approval_ids.insert(approval_id.into());
    }

    /// Returns `true` if the given key ID has been revoked.
    pub fn is_key_revoked(&self, key_id: &str) -> bool {
        self.revoked_key_ids.contains(key_id)
    }

    /// Returns `true` if the given approval ID has been revoked.
    pub fn is_approval_revoked(&self, approval_id: &str) -> bool {
        self.revoked_approval_ids.contains(approval_id)
    }

    /// Returns `true` if neither the approver_id (key ID) nor the approval_id
    /// is revoked.  This is the single call-site check for the approval loop.
    pub fn check_approval(
        &self,
        approver_id: &str,
        approval_id: &str,
    ) -> Result<(), String> {
        if self.is_key_revoked(approver_id) {
            return Err(format!(
                "revocation: approver '{approver_id}' key has been revoked"
            ));
        }
        if self.is_approval_revoked(approval_id) {
            return Err(format!(
                "revocation: approval '{approval_id}' has been revoked"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_revokes_nothing() {
        let rl = RevocationList::new();
        assert!(!rl.is_key_revoked("key-1"));
        assert!(!rl.is_approval_revoked("appr-1"));
        assert!(rl.check_approval("key-1", "appr-1").is_ok());
    }

    #[test]
    fn revoked_key_blocks_approval() {
        let mut rl = RevocationList::new();
        rl.revoke_key("compromised-operator");
        assert!(rl.is_key_revoked("compromised-operator"));
        assert!(rl.check_approval("compromised-operator", "appr-1").is_err());
    }

    #[test]
    fn revoked_approval_id_blocks_specific_approval() {
        let mut rl = RevocationList::new();
        rl.revoke_approval("appr-bad");
        assert!(rl.check_approval("good-key", "appr-bad").is_err());
        assert!(rl.check_approval("good-key", "appr-ok").is_ok());
    }

    #[test]
    fn from_env_parses_json() {
        let json = r#"{"key_ids": ["k1", "k2"], "approval_ids": ["a1"]}"#;
        // SAFETY: single-threaded test, no other threads reading this env var.
        unsafe { std::env::set_var("AXIOMLAB_REVOCATION_LIST", json); }
        let rl = RevocationList::from_env();
        unsafe { std::env::remove_var("AXIOMLAB_REVOCATION_LIST"); }
        assert!(rl.is_key_revoked("k1"));
        assert!(rl.is_key_revoked("k2"));
        assert!(rl.is_approval_revoked("a1"));
        assert!(!rl.is_key_revoked("k3"));
    }
}
