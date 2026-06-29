//! Per-action, per-parameter capability bounds (operational policy).
//!
//! These are the lab's *operational* limits — narrower than the formally
//! verified hardware envelope the `ProofGate` enforces. Both apply; an action
//! must satisfy operational policy *and* the verified bounds.

use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct NumericRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone)]
pub struct ActionCapability {
    pub action: String,
    pub numeric_limits: HashMap<String, NumericRange>,
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityPolicy {
    actions: HashMap<String, ActionCapability>,
}

impl CapabilityPolicy {
    pub fn from_actions(actions: Vec<ActionCapability>) -> Self {
        Self { actions: actions.into_iter().map(|a| (a.action.clone(), a)).collect() }
    }

    /// The default lab policy: arm travel and dispense volume bounds.
    pub fn default_lab() -> Self {
        let range = |min, max| NumericRange { min, max };
        let arm = HashMap::from([
            ("x".to_string(), range(0.0, 300.0)),
            ("y".to_string(), range(0.0, 300.0)),
            ("z".to_string(), range(0.0, 250.0)),
        ]);
        let dispense = HashMap::from([("volume_ul".to_string(), range(0.5, 1000.0))]);
        let aspirate = HashMap::from([("volume_ul".to_string(), range(0.5, 1000.0))]);
        Self::from_actions(vec![
            ActionCapability { action: "move_arm".into(), numeric_limits: arm },
            ActionCapability { action: "dispense".into(), numeric_limits: dispense },
            ActionCapability { action: "aspirate".into(), numeric_limits: aspirate },
        ])
    }

    /// A human-readable summary of the configured bounds, for the LLM mandate.
    pub fn describe(&self) -> String {
        let mut actions: Vec<&ActionCapability> = self.actions.values().collect();
        actions.sort_by(|a, b| a.action.cmp(&b.action));
        let mut out = String::new();
        for cap in actions {
            let mut limits: Vec<String> = cap
                .numeric_limits
                .iter()
                .map(|(k, r)| format!("{k}∈[{}, {}]", r.min, r.max))
                .collect();
            limits.sort();
            out.push_str(&format!("- {}: {}\n", cap.action, limits.join(", ")));
        }
        out
    }

    /// Validate `params` for `action`. Unknown actions pass (no configured limit);
    /// out-of-range or missing numeric parameters are rejected.
    pub fn validate(&self, action: &str, params: &Value) -> Result<(), String> {
        let Some(cap) = self.actions.get(action) else {
            return Ok(());
        };
        for (key, range) in &cap.numeric_limits {
            let v = params.get(key).and_then(|x| x.as_f64()).ok_or_else(|| {
                format!("capability violation: missing numeric parameter '{key}' for '{action}'")
            })?;
            if v < range.min || v > range.max {
                return Err(format!(
                    "capability violation: '{action}' parameter '{key}'={v} outside [{}, {}]",
                    range.min, range.max
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_out_of_bounds_arm() {
        let p = CapabilityPolicy::default_lab();
        assert!(p.validate("move_arm", &json!({"x": 999.0, "y": 10.0, "z": 10.0})).is_err());
        assert!(p.validate("move_arm", &json!({"x": 100.0, "y": 120.0, "z": 80.0})).is_ok());
    }

    #[test]
    fn rejects_over_volume() {
        let p = CapabilityPolicy::default_lab();
        assert!(p.validate("dispense", &json!({"volume_ul": 5000.0})).is_err());
        assert!(p.validate("dispense", &json!({"volume_ul": 50.0})).is_ok());
    }

    #[test]
    fn missing_param_rejected() {
        let p = CapabilityPolicy::default_lab();
        assert!(p.validate("dispense", &json!({})).is_err());
    }

    #[test]
    fn unknown_action_passes() {
        let p = CapabilityPolicy::default_lab();
        assert!(p.validate("read_absorbance", &json!({"vessel_id": "x"})).is_ok());
    }
}
