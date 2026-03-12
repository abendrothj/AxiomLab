use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use proof_artifacts::manifest::RiskClass;
use proof_artifacts::policy::ExecutionContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct ApprovalPolicy {
    pub required_roles: Vec<String>,
}

impl ApprovalPolicy {
    pub fn default_high_risk() -> Self {
        Self {
            required_roles: vec!["operator".into(), "pi".into()],
        }
    }

    pub fn validate_action(
        &self,
        action: &str,
        risk_class: Option<RiskClass>,
        ctx: &ExecutionContext,
        params: &Value,
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

        let mut covered_roles = HashSet::new();
        let mut approver_ids = HashSet::new();
        let mut approval_ids = Vec::new();

        for signed in bundle {
            verify_signature(&signed)?;
            let s = &signed.statement;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalStatement {
    pub approval_id: String,
    pub action: String,
    pub approver_role: String,
    pub approver_id: String,
    pub git_commit: String,
    pub binary_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedApproval {
    pub statement: ApprovalStatement,
    pub public_key_b64: String,
    pub signature_b64: String,
}

fn requires_two_person_approval(risk_class: Option<RiskClass>) -> bool {
    matches!(risk_class, Some(RiskClass::Actuation | RiskClass::Destructive))
}

fn parse_bundle(params: &Value) -> Result<Vec<SignedApproval>, String> {
    let raw = params
        .get("approval_bundle")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    serde_json::from_value(raw).map_err(|e| format!("approval bundle parse failed: {e}"))
}

fn verify_signature(signed: &SignedApproval) -> Result<(), String> {
    let message = serde_json::to_vec(&signed.statement)
        .map_err(|e| format!("approval serialization failed: {e}"))?;

    let public_key_bytes = STANDARD
        .decode(signed.public_key_b64.trim())
        .map_err(|e| format!("approval public_key base64 decode failed: {e}"))?;
    let signature_bytes = STANDARD
        .decode(signed.signature_b64.trim())
        .map_err(|e| format!("approval signature base64 decode failed: {e}"))?;

    let public_key_arr: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| "approval public key must be 32 bytes".to_string())?;
    let signature_arr: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| "approval signature must be 64 bytes".to_string())?;

    let key = VerifyingKey::from_bytes(&public_key_arr)
        .map_err(|e| format!("approval verifying key invalid: {e}"))?;
    let sig = Signature::from_bytes(&signature_arr);
    key.verify(&message, &sig)
        .map_err(|e| format!("approval signature invalid: {e}"))
}

pub fn risk_class_for_action(
    action: &str,
    action_risks: &HashMap<String, RiskClass>,
) -> Option<RiskClass> {
    action_risks.get(action).cloned().or_else(|| match action {
        "move_arm" => Some(RiskClass::Actuation),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn sign_statement(statement: &ApprovalStatement, sk: &SigningKey) -> SignedApproval {
        let msg = serde_json::to_vec(statement).expect("serialize statement");
        let sig = sk.sign(&msg);
        SignedApproval {
            statement: statement.clone(),
            public_key_b64: STANDARD.encode(sk.verifying_key().to_bytes()),
            signature_b64: STANDARD.encode(sig.to_bytes()),
        }
    }

    #[test]
    fn high_risk_requires_operator_and_pi() {
        let policy = ApprovalPolicy::default_high_risk();
        let ctx = ExecutionContext {
            git_commit: "g".into(),
            binary_hash: "b".into(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        };

        let sk_operator = SigningKey::from_bytes(&[1u8; 32]);
        let sk_pi = SigningKey::from_bytes(&[2u8; 32]);

        let op = ApprovalStatement {
            approval_id: "a-op".into(),
            action: "move_arm".into(),
            approver_role: "operator".into(),
            approver_id: "u-op".into(),
            git_commit: "g".into(),
            binary_hash: "b".into(),
        };
        let pi = ApprovalStatement {
            approval_id: "a-pi".into(),
            action: "move_arm".into(),
            approver_role: "pi".into(),
            approver_id: "u-pi".into(),
            git_commit: "g".into(),
            binary_hash: "b".into(),
        };

        let params = serde_json::json!({
            "approval_bundle": [
                sign_statement(&op, &sk_operator),
                sign_statement(&pi, &sk_pi)
            ]
        });

        let ids = policy
            .validate_action("move_arm", Some(RiskClass::Actuation), &ctx, &params)
            .expect("approvals should validate");
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn invalid_signature_is_denied() {
        let policy = ApprovalPolicy::default_high_risk();
        let ctx = ExecutionContext {
            git_commit: "g".into(),
            binary_hash: "b".into(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        };

        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let mut signed = sign_statement(
            &ApprovalStatement {
                approval_id: "a-op".into(),
                action: "move_arm".into(),
                approver_role: "operator".into(),
                approver_id: "u-op".into(),
                git_commit: "g".into(),
                binary_hash: "b".into(),
            },
            &sk,
        );
        signed.signature_b64 = STANDARD.encode([0u8; 64]);

        let params = serde_json::json!({"approval_bundle": [signed]});
        assert!(policy
            .validate_action("move_arm", Some(RiskClass::Actuation), &ctx, &params)
            .is_err());
    }
}
