use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
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
        let mut by_action = HashMap::new();
        for action in actions {
            by_action.insert(action.action.clone(), action);
        }
        Self { actions: by_action }
    }

    pub fn default_lab() -> Self {
        let mut move_arm_limits = HashMap::new();
        move_arm_limits.insert(
            "x".into(),
            NumericRange {
                min: 0.0,
                max: 300.0,
            },
        );
        move_arm_limits.insert(
            "y".into(),
            NumericRange {
                min: 0.0,
                max: 300.0,
            },
        );
        move_arm_limits.insert(
            "z".into(),
            NumericRange {
                min: 0.0,
                max: 250.0,
            },
        );

        let mut dispense_limits = HashMap::new();
        dispense_limits.insert(
            "volume_ul".into(),
            NumericRange {
                min: 0.5,
                max: 1000.0,
            },
        );

        Self::from_actions(vec![
            ActionCapability {
                action: "move_arm".into(),
                numeric_limits: move_arm_limits,
            },
            ActionCapability {
                action: "dispense".into(),
                numeric_limits: dispense_limits,
            },
        ])
    }

    /// Return the maximum allowed value for `param` under `action`, if configured.
    pub fn max_for(&self, action: &str, param: &str) -> Option<f64> {
        self.actions.get(action)?.numeric_limits.get(param).map(|r| r.max)
    }

    pub fn validate(&self, action: &str, params: &Value) -> Result<(), String> {
        let Some(cap) = self.actions.get(action) else {
            return Ok(());
        };

        for (key, range) in &cap.numeric_limits {
            let Some(v) = params.get(key).and_then(|x| x.as_f64()) else {
                return Err(format!("capability violation: missing numeric parameter '{key}' for action '{action}'"));
            };
            if v < range.min || v > range.max {
                return Err(format!(
                    "capability violation: action '{}' parameter '{}'={} outside [{}, {}]",
                    action, key, v, range.min, range.max
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_denies_out_of_bounds_move_arm() {
        let p = CapabilityPolicy::default_lab();
        let bad = serde_json::json!({"x": 999.0, "y": 10.0, "z": 10.0});
        assert!(p.validate("move_arm", &bad).is_err());
    }

    #[test]
    fn default_policy_allows_in_bounds_move_arm() {
        let p = CapabilityPolicy::default_lab();
        let ok = serde_json::json!({"x": 100.0, "y": 120.0, "z": 80.0});
        assert!(p.validate("move_arm", &ok).is_ok());
    }
}
