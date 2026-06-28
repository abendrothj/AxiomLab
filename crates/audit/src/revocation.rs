//! Revocation list for compromised signing keys and individual approvals.
//!
//! Loaded from `AXIOMLAB_REVOCATION_LIST` (a JSON object with `key_ids` and
//! `approval_ids` arrays). The `AuditGate` and approval path check this list
//! before honouring any signed approval — wiring it to an actual revoked-key
//! store rather than always-empty default.

use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct RevocationList {
    revoked_key_ids: HashSet<String>,
    revoked_approval_ids: HashSet<String>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    key_ids: Vec<String>,
    #[serde(default)]
    approval_ids: Vec<String>,
}

impl RevocationList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from the `AXIOMLAB_REVOCATION_LIST` environment variable.
    pub fn from_env() -> Self {
        let Ok(raw) = std::env::var("AXIOMLAB_REVOCATION_LIST") else {
            return Self::default();
        };
        Self::from_json(&raw).unwrap_or_else(|e| {
            tracing::warn!("AXIOMLAB_REVOCATION_LIST parse error — no revocations loaded: {e}");
            Self::default()
        })
    }

    /// Parse a revocation payload from a JSON string.
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        let payload: Payload = serde_json::from_str(raw)?;
        Ok(Self {
            revoked_key_ids: payload.key_ids.into_iter().collect(),
            revoked_approval_ids: payload.approval_ids.into_iter().collect(),
        })
    }

    pub fn revoke_key(&mut self, key_id: impl Into<String>) {
        self.revoked_key_ids.insert(key_id.into());
    }
    pub fn revoke_approval(&mut self, approval_id: impl Into<String>) {
        self.revoked_approval_ids.insert(approval_id.into());
    }
    pub fn is_key_revoked(&self, key_id: &str) -> bool {
        self.revoked_key_ids.contains(key_id)
    }
    pub fn is_approval_revoked(&self, approval_id: &str) -> bool {
        self.revoked_approval_ids.contains(approval_id)
    }

    /// Single call-site check: neither the signing key nor the approval is revoked.
    pub fn check_approval(&self, key_id: &str, approval_id: &str) -> Result<(), String> {
        if self.is_key_revoked(key_id) {
            return Err(format!("revocation: signing key '{key_id}' has been revoked"));
        }
        if self.is_approval_revoked(approval_id) {
            return Err(format!("revocation: approval '{approval_id}' has been revoked"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_revokes_nothing() {
        let rl = RevocationList::new();
        assert!(rl.check_approval("k", "a").is_ok());
    }

    #[test]
    fn revoked_key_blocks() {
        let mut rl = RevocationList::new();
        rl.revoke_key("bad");
        assert!(rl.check_approval("bad", "a").is_err());
    }

    #[test]
    fn revoked_approval_blocks_specific() {
        let mut rl = RevocationList::new();
        rl.revoke_approval("a-bad");
        assert!(rl.check_approval("k", "a-bad").is_err());
        assert!(rl.check_approval("k", "a-ok").is_ok());
    }

    #[test]
    fn from_json_parses() {
        let rl = RevocationList::from_json(r#"{"key_ids":["k1"],"approval_ids":["a1"]}"#).unwrap();
        assert!(rl.is_key_revoked("k1"));
        assert!(rl.is_approval_revoked("a1"));
        assert!(!rl.is_key_revoked("k2"));
    }
}
