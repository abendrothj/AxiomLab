//! The [`Action`] an LLM proposes and the [`Rejection`] a gate returns.

use serde::{Deserialize, Serialize};

/// Risk taxonomy for a proposed action.
///
/// Determines which gates apply. `Actuation` and `Destructive` require operator
/// approval; all classes flow through the full proof + capability pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskClass {
    /// Sensor reads and other non-mutating operations.
    ReadOnly,
    /// Dispense / aspirate — moves fluid but not the arm.
    LiquidHandling,
    /// Arm movement, centrifugation, and other physical actuation.
    Actuation,
    /// Irreversible operations (e.g. sample disposal).
    Destructive,
}

impl RiskClass {
    /// True if this risk class requires an operator approval before execution.
    pub fn requires_approval(self) -> bool {
        matches!(self, RiskClass::Actuation | RiskClass::Destructive)
    }
}

/// A single proposed laboratory action.
///
/// Produced by the LLM, then carried unchanged through the gate pipeline. The
/// gates never mutate an `Action`; they only accept or reject it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Tool name, e.g. `"dispense"`, `"move_arm"`, `"read_absorbance"`.
    pub tool: String,
    /// Tool parameters as raw JSON (validated per-tool by the gates).
    pub params: serde_json::Value,
    /// Risk class governing which gates apply.
    pub risk_class: RiskClass,
}

impl Action {
    pub fn new(tool: impl Into<String>, params: serde_json::Value, risk_class: RiskClass) -> Self {
        Self { tool: tool.into(), params, risk_class }
    }
}

/// A snapshot of the action that a gate rejected.
///
/// Held by value (not borrowed) so a `Rejection` is self-contained and can be
/// written straight into the audit chain.
pub type RejectedAction = Action;

/// The result of a gate refusing an action.
///
/// Every gate returns `Result<(), Rejection>`. The first `Err` in the pipeline
/// hard-stops the action; nothing downstream runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rejection {
    /// Name of the gate that rejected the action (e.g. `"CapabilityGate"`).
    pub gate: &'static str,
    /// Human-readable reason, suitable for operator display and audit.
    pub reason: String,
    /// The rejected action, preserved for the audit record.
    pub action: RejectedAction,
}

impl Rejection {
    pub fn new(gate: &'static str, reason: impl Into<String>, action: Action) -> Self {
        Self { gate, reason: reason.into(), action }
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} (tool={})", self.gate, self.reason, self.action.tool)
    }
}

impl std::error::Error for Rejection {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_only_for_high_risk() {
        assert!(!RiskClass::ReadOnly.requires_approval());
        assert!(!RiskClass::LiquidHandling.requires_approval());
        assert!(RiskClass::Actuation.requires_approval());
        assert!(RiskClass::Destructive.requires_approval());
    }

    #[test]
    fn rejection_carries_action() {
        let action = Action::new("dispense", serde_json::json!({"volume_ul": 5.0}), RiskClass::LiquidHandling);
        let rej = Rejection::new("CapabilityGate", "out of range", action.clone());
        assert_eq!(rej.gate, "CapabilityGate");
        assert_eq!(rej.action.tool, "dispense");
        assert!(rej.to_string().contains("dispense"));
    }
}
