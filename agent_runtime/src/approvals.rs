use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use proof_artifacts::manifest::RiskClass;
use proof_artifacts::policy::ExecutionContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::revocation::RevocationList;

/// SECURITY: Key lifecycle is critical. See OPERATOR_GUIDE.md section 2.5.

// ── Trusted key registry ──────────────────────────────────────────

/// Maps approver_id → raw 32-byte Ed25519 public key.
///
/// This is the ground truth of who may sign approvals.  An approval bundle
/// whose `public_key_b64` does not match the registry entry for the claimed
/// `approver_id` is rejected even if the signature itself is valid — preventing
/// self-signed approvals from arbitrary keys.
///
/// Load via `KeyRegistry::from_env()` which reads `AXIOMLAB_TRUSTED_KEYS`
/// (a JSON object mapping approver_id → base64-encoded 32-byte public key).
#[derive(Debug, Clone, Default)]
pub struct KeyRegistry {
    keys: HashMap<String, Vec<u8>>,
}

impl KeyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a trusted key.
    pub fn register(&mut self, approver_id: impl Into<String>, public_key_bytes: Vec<u8>) {
        self.keys.insert(approver_id.into(), public_key_bytes);
    }

    /// Load from `AXIOMLAB_TRUSTED_KEYS` environment variable.
    ///
    /// Expected format: JSON object `{"approver_id": "base64_pubkey_32bytes", ...}`
    pub fn from_env() -> Self {
        let Ok(raw) = std::env::var("AXIOMLAB_TRUSTED_KEYS") else {
            return Self::default();
        };
        let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&raw) else {
            tracing::warn!("AXIOMLAB_TRUSTED_KEYS is not valid JSON — no keys loaded");
            return Self::default();
        };
        let mut registry = Self::default();
        for (id, b64) in map {
            match STANDARD.decode(b64.trim()) {
                Ok(bytes) if bytes.len() == 32 => {
                    registry.keys.insert(id, bytes);
                }
                _ => {
                    tracing::warn!(approver_id = %id, "skipping malformed key in AXIOMLAB_TRUSTED_KEYS");
                }
            }
        }
        registry
    }

    /// Look up the trusted public key for an approver.
    pub fn get(&self, approver_id: &str) -> Option<&[u8]> {
        self.keys.get(approver_id).map(|v| v.as_slice())
    }

    /// True if the registry has at least one entry (non-empty means enforcement is active).
    pub fn is_active(&self) -> bool {
        !self.keys.is_empty()
    }
}

// ── Approval policy ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ApprovalPolicy {
    pub required_roles: Vec<String>,
    /// Maximum age (in seconds) a valid approval may be before it is rejected.
    /// Default: 3600 (1 hour).
    pub max_approval_age_secs: u64,
    /// Trusted key registry.  When non-empty, the public key in every signed
    /// approval must match the registry entry for the claimed approver_id.
    pub key_registry: KeyRegistry,
    /// Revocation list.  Approvals whose approval_id or approver_id appears
    /// here are rejected even if the signature is valid.
    pub revocation_list: RevocationList,
}

impl ApprovalPolicy {
    pub fn default_high_risk() -> Self {
        Self {
            required_roles: vec!["operator".into(), "pi".into()],
            max_approval_age_secs: 3600,
            key_registry: KeyRegistry::from_env(),
            revocation_list: RevocationList::from_env(),
        }
    }

    /// Validate a tool call against this policy.
    ///
    /// Returns the list of `approval_id`s that were validated, or an error
    /// string describing the first policy violation.
    ///
    /// `session_nonce` — if `Some`, every `ApprovalStatement` in the bundle
    /// must carry the same nonce, preventing cross-session replay of signed
    /// bundles.
    pub fn validate_action(
        &self,
        action: &str,
        risk_class: Option<RiskClass>,
        ctx: &ExecutionContext,
        params: &Value,
        session_nonce: Option<&str>,
    ) -> Result<Vec<String>, String> {
        if !requires_two_person_approval(risk_class) {
            return Ok(Vec::new());
        }

        let bundle = parse_bundle(params)?;
        if bundle.is_empty() {
            return Err(format!(
                "approval violation: action '{action}' requires signed approvals from roles {}",
                self.required_roles.join(",")
            ));
        }

        let now = unix_now_secs();
        let mut covered_roles = HashSet::new();
        let mut approver_ids = HashSet::new();
        let mut approval_ids = Vec::new();

        for signed in &bundle {
            // Verify Ed25519 signature, optionally cross-checking the key against the registry.
            verify_signature(signed, &self.key_registry)?;

            let s = &signed.statement;

            // ── Revocation check ──────────────────────────────────────────
            self.revocation_list
                .check_approval(&s.approver_id, &s.approval_id)?;

            // ── Expiry check ──────────────────────────────────────
            if now < s.issued_at_unix_secs {
                return Err(format!(
                    "approval violation: approval {} issued in the future (clock skew?)",
                    s.approval_id
                ));
            }
            if now > s.expires_at_unix_secs {
                return Err(format!(
                    "approval violation: approval {} expired at unix={} (now={})",
                    s.approval_id, s.expires_at_unix_secs, now
                ));
            }
            let age = now.saturating_sub(s.issued_at_unix_secs);
            if age > self.max_approval_age_secs {
                return Err(format!(
                    "approval violation: approval {} is {} seconds old (max={})",
                    s.approval_id, age, self.max_approval_age_secs
                ));
            }

            // ── Session nonce check (replay prevention) ───────────
            if let Some(expected_nonce) = session_nonce {
                if s.session_nonce.as_deref() != Some(expected_nonce) {
                    return Err(format!(
                        "approval violation: approval {} session_nonce mismatch \
                         (expected '{}', got '{:?}')",
                        s.approval_id, expected_nonce, s.session_nonce
                    ));
                }
            }

            // ── Action + build identity match ─────────────────────
            if s.action != action {
                return Err(format!(
                    "approval violation: approval {} action mismatch (expected '{}', got '{}')",
                    s.approval_id, action, s.action
                ));
            }
            if s.git_commit != ctx.git_commit || s.binary_hash != ctx.binary_hash {
                return Err(format!(
                    "approval violation: approval {} build identity mismatch",
                    s.approval_id
                ));
            }

            // ── Role checks ───────────────────────────────────────
            if !self.required_roles.iter().any(|r| r == &s.approver_role) {
                return Err(format!(
                    "approval violation: approval {} has unsupported role '{}'",
                    s.approval_id, s.approver_role
                ));
            }
            if !approver_ids.insert(s.approver_id.clone()) {
                return Err(format!(
                    "approval violation: duplicate approver identity '{}'",
                    s.approver_id
                ));
            }

            covered_roles.insert(s.approver_role.clone());
            approval_ids.push(s.approval_id.clone());
        }

        let missing_roles: Vec<String> = self
            .required_roles
            .iter()
            .filter(|r| !covered_roles.contains(*r))
            .cloned()
            .collect();
        if !missing_roles.is_empty() {
            return Err(format!(
                "approval violation: missing required roles {}",
                missing_roles.join(",")
            ));
        }

        Ok(approval_ids)
    }
}

// ── Data structures ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalStatement {
    pub approval_id: String,
    pub action: String,
    pub approver_role: String,
    pub approver_id: String,
    pub git_commit: String,
    pub binary_hash: String,
    /// Unix timestamp when this approval was created.
    pub issued_at_unix_secs: u64,
    /// Unix timestamp after which this approval must be rejected.
    pub expires_at_unix_secs: u64,
    /// Session nonce issued by the orchestrator for this execution session.
    /// Prevents replay of approval bundles across different sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedApproval {
    pub statement: ApprovalStatement,
    pub public_key_b64: String,
    pub signature_b64: String,
}

// ── Internal helpers ──────────────────────────────────────────────

pub fn requires_two_person_approval(risk_class: Option<RiskClass>) -> bool {
    matches!(risk_class, Some(RiskClass::Actuation | RiskClass::Destructive))
}

fn parse_bundle(params: &Value) -> Result<Vec<SignedApproval>, String> {
    let raw = params
        .get("approval_bundle")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    serde_json::from_value(raw).map_err(|e| format!("approval bundle parse failed: {e}"))
}

/// Verify the Ed25519 signature on a signed approval.
///
/// When `registry` is active (non-empty), the `public_key_b64` in the bundle
/// must match the registered key for the claimed `approver_id`.  This prevents
/// self-signed approvals from arbitrary keys.
fn verify_signature(signed: &SignedApproval, registry: &KeyRegistry) -> Result<(), String> {
    let public_key_bytes = STANDARD
        .decode(signed.public_key_b64.trim())
        .map_err(|e| format!("approval public_key base64 decode failed: {e}"))?;

    // ── Registry enforcement ──────────────────────────────────────
    if registry.is_active() {
        let trusted = registry
            .get(&signed.statement.approver_id)
            .ok_or_else(|| {
                format!(
                    "approval violation: approver '{}' is not in the trusted key registry",
                    signed.statement.approver_id
                )
            })?;
        if trusted != public_key_bytes.as_slice() {
            return Err(format!(
                "approval violation: public key for '{}' does not match registry",
                signed.statement.approver_id
            ));
        }
    }

    let signature_bytes = STANDARD
        .decode(signed.signature_b64.trim())
        .map_err(|e| format!("approval signature base64 decode failed: {e}"))?;

    let public_key_arr: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| "approval public key must be 32 bytes".to_string())?;
    let signature_arr: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| "approval signature must be 64 bytes".to_string())?;

    let message = serde_json::to_vec(&signed.statement)
        .map_err(|e| format!("approval serialization failed: {e}"))?;

    let key = VerifyingKey::from_bytes(&public_key_arr)
        .map_err(|e| format!("approval verifying key invalid: {e}"))?;
    let sig = Signature::from_bytes(&signature_arr);
    key.verify(&message, &sig)
        .map_err(|e| format!("approval signature invalid: {e}"))
}

/// Resolve the risk class for a tool call from the manifest-derived action index.
///
/// The manifest is the single source of truth — there is no hardcoded fallback.
/// Any action not present in the index is treated as having an unknown risk class
/// and will be denied by the fail-closed policy in the orchestrator.
pub fn risk_class_for_action(
    action: &str,
    action_risks: &HashMap<String, RiskClass>,
) -> Option<RiskClass> {
    action_risks.get(action).cloned()
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn make_statement(action: &str, role: &str, id: &str, git: &str, bin: &str) -> ApprovalStatement {
        let now = unix_now_secs();
        ApprovalStatement {
            approval_id: format!("a-{id}"),
            action: action.into(),
            approver_role: role.into(),
            approver_id: id.into(),
            git_commit: git.into(),
            binary_hash: bin.into(),
            issued_at_unix_secs: now,
            expires_at_unix_secs: now + 3600,
            session_nonce: None,
        }
    }

    fn sign_statement(statement: &ApprovalStatement, sk: &SigningKey) -> SignedApproval {
        let msg = serde_json::to_vec(statement).expect("serialize statement");
        let sig = sk.sign(&msg);
        SignedApproval {
            statement: statement.clone(),
            public_key_b64: STANDARD.encode(sk.verifying_key().to_bytes()),
            signature_b64: STANDARD.encode(sig.to_bytes()),
        }
    }

    fn ctx(git: &str, bin: &str) -> ExecutionContext {
        ExecutionContext {
            git_commit: git.into(),
            binary_hash: bin.into(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        }
    }

    #[test]
    fn high_risk_requires_operator_and_pi() {
        let sk_op = SigningKey::from_bytes(&[1u8; 32]);
        let sk_pi = SigningKey::from_bytes(&[2u8; 32]);

        let mut policy = ApprovalPolicy::default_high_risk();
        // Register trusted keys so registry enforcement is active.
        policy.key_registry.register("u-op", sk_op.verifying_key().to_bytes().to_vec());
        policy.key_registry.register("u-pi", sk_pi.verifying_key().to_bytes().to_vec());

        let op = make_statement("move_arm", "operator", "u-op", "g", "b");
        let pi = make_statement("move_arm", "pi", "u-pi", "g", "b");
        let params = serde_json::json!({
            "approval_bundle": [sign_statement(&op, &sk_op), sign_statement(&pi, &sk_pi)]
        });

        let ids = policy
            .validate_action("move_arm", Some(RiskClass::Actuation), &ctx("g", "b"), &params, None)
            .expect("approvals should validate");
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn invalid_signature_is_denied() {
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let policy = ApprovalPolicy::default_high_risk(); // empty registry = no registry enforcement
        let stmt = make_statement("move_arm", "operator", "u-op", "g", "b");
        let mut signed = sign_statement(&stmt, &sk);
        signed.signature_b64 = STANDARD.encode([0u8; 64]); // corrupt signature

        let params = serde_json::json!({"approval_bundle": [signed]});
        assert!(policy
            .validate_action("move_arm", Some(RiskClass::Actuation), &ctx("g", "b"), &params, None)
            .is_err());
    }

    #[test]
    fn expired_approval_is_denied() {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let policy = ApprovalPolicy::default_high_risk();
        let now = unix_now_secs();
        let stmt = ApprovalStatement {
            approval_id: "a-exp".into(),
            action: "move_arm".into(),
            approver_role: "operator".into(),
            approver_id: "u-op".into(),
            git_commit: "g".into(),
            binary_hash: "b".into(),
            issued_at_unix_secs: now - 7200,
            expires_at_unix_secs: now - 3600, // already expired
            session_nonce: None,
        };
        let signed = sign_statement(&stmt, &sk);
        let params = serde_json::json!({"approval_bundle": [signed]});
        let err = policy
            .validate_action("move_arm", Some(RiskClass::Actuation), &ctx("g", "b"), &params, None)
            .unwrap_err();
        assert!(err.contains("expired"), "expected expiry error, got: {err}");
    }

    #[test]
    fn session_nonce_mismatch_is_denied() {
        let sk_op = SigningKey::from_bytes(&[4u8; 32]);
        let sk_pi = SigningKey::from_bytes(&[5u8; 32]);
        let policy = ApprovalPolicy::default_high_risk();

        let mut op = make_statement("move_arm", "operator", "u-op", "g", "b");
        op.session_nonce = Some("wrong-nonce".into());
        let mut pi = make_statement("move_arm", "pi", "u-pi", "g", "b");
        pi.session_nonce = Some("wrong-nonce".into());

        let params = serde_json::json!({
            "approval_bundle": [sign_statement(&op, &sk_op), sign_statement(&pi, &sk_pi)]
        });
        let err = policy
            .validate_action(
                "move_arm",
                Some(RiskClass::Actuation),
                &ctx("g", "b"),
                &params,
                Some("correct-nonce"),
            )
            .unwrap_err();
        assert!(err.contains("session_nonce"), "expected nonce error, got: {err}");
    }

    #[test]
    fn unregistered_key_is_denied_when_registry_active() {
        let _sk_op = SigningKey::from_bytes(&[6u8; 32]);
        let sk_pi = SigningKey::from_bytes(&[7u8; 32]);
        let attacker_key = SigningKey::from_bytes(&[99u8; 32]);

        let mut policy = ApprovalPolicy::default_high_risk();
        // Only register the PI key — attacker is not registered.
        policy.key_registry.register("u-pi", sk_pi.verifying_key().to_bytes().to_vec());

        let op = make_statement("move_arm", "operator", "u-op", "g", "b");
        let pi = make_statement("move_arm", "pi", "u-pi", "g", "b");
        let params = serde_json::json!({
            "approval_bundle": [
                sign_statement(&op, &attacker_key), // signed by attacker, not u-op's real key
                sign_statement(&pi, &sk_pi)
            ]
        });
        assert!(policy
            .validate_action("move_arm", Some(RiskClass::Actuation), &ctx("g", "b"), &params, None)
            .is_err());
    }

    #[test]
    fn readonly_action_skips_approval() {
        let policy = ApprovalPolicy::default_high_risk();
        let params = serde_json::json!({});
        let ids = policy
            .validate_action(
                "read_sensor",
                Some(RiskClass::ReadOnly),
                &ctx("g", "b"),
                &params,
                None,
            )
            .expect("read-only should not require approval");
        assert!(ids.is_empty());
    }

    #[test]
    fn unknown_action_in_manifest_has_no_hardcoded_fallback() {
        // After fix #7 the function must not return a risk class for actions
        // not present in the manifest index.
        let index: HashMap<String, RiskClass> = HashMap::new();
        assert!(
            risk_class_for_action("move_arm", &index).is_none(),
            "move_arm must not have a hardcoded fallback — it must come from the manifest"
        );
    }
}
