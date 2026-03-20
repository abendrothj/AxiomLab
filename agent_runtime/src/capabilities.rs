use serde_json::Value;
use std::collections::HashMap;
use crate::units;

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

    /// Validate `params` for `action` against the configured capability limits.
    ///
    /// `parameter_units` (from [`crate::tools::ToolSpec::parameter_units`]) declares what
    /// unit each parameter value is expressed in.  When a unit is declared, the value is
    /// converted to the canonical base unit (µL for volumes, mm for lengths) before being
    /// compared against the stored bounds — so a limit of `[0.5, 1000] µL` correctly
    /// rejects `{"volume_ul": 2.0}` when the tool declares `"volume_ul": "mL"` (2 mL =
    /// 2000 µL, which exceeds the 1000 µL maximum).
    pub fn validate(
        &self,
        action: &str,
        params: &Value,
        parameter_units: Option<&HashMap<String, String>>,
    ) -> Result<(), String> {
        let Some(cap) = self.actions.get(action) else {
            return Ok(());
        };

        for (key, range) in &cap.numeric_limits {
            let Some(raw) = params.get(key).and_then(|x| x.as_f64()) else {
                return Err(format!(
                    "capability violation: missing numeric parameter '{key}' for action '{action}'"
                ));
            };

            // Convert to canonical base unit when a unit declaration is available.
            let v = if let Some(unit_map) = parameter_units {
                if let Some(declared_unit) = unit_map.get(key) {
                    units::to_canonical(raw, declared_unit, key)
                } else {
                    raw
                }
            } else {
                raw
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
        assert!(p.validate("move_arm", &bad, None).is_err());
    }

    #[test]
    fn default_policy_allows_in_bounds_move_arm() {
        let p = CapabilityPolicy::default_lab();
        let ok = serde_json::json!({"x": 100.0, "y": 120.0, "z": 80.0});
        assert!(p.validate("move_arm", &ok, None).is_ok());
    }

    #[test]
    fn unit_aware_volume_conversion_denies_over_limit() {
        // Policy: volume_ul ∈ [0.5, 1000] µL.
        // LLM passes 2.0 with unit "mL" → 2000 µL → exceeds 1000 µL limit.
        let p = CapabilityPolicy::default_lab();
        let params = serde_json::json!({"volume_ul": 2.0});
        let units = HashMap::from([("volume_ul".to_string(), "mL".to_string())]);
        assert!(p.validate("dispense", &params, Some(&units)).is_err());
    }

    #[test]
    fn unit_aware_volume_conversion_allows_in_range() {
        // 0.5 mL = 500 µL — within [0.5, 1000] µL.
        let p = CapabilityPolicy::default_lab();
        let params = serde_json::json!({"volume_ul": 0.5});
        let units = HashMap::from([("volume_ul".to_string(), "mL".to_string())]);
        assert!(p.validate("dispense", &params, Some(&units)).is_ok());
    }
}
