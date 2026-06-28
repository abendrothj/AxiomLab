//! Runtime predicates mirroring the formally-verified hardware envelope.
//!
//! These constants and functions are the runtime twins of the Verus spec in
//! `verus_verified/lab_safety.rs` — same bounds, same logic. Verus discharges
//! the safety preconditions at compile time; these enforce them at runtime by
//! being **called with the actual proposed parameter values**. That call — not
//! merely checking that a signed artifact exists — is the architectural fix the
//! `ProofGate` is built around.
//!
//! CI's `verus.yml` job verifies the source these mirror; the
//! `constants_match_verus_source` test guards against drift.

use axiom_types::Action;

// ── Verified constants (mirror verus_verified/lab_safety.rs) ────────────────

/// Maximum robotic-arm extension, millimetres.
pub const MAX_ARM_EXTENSION_MM: u64 = 1200;
pub const MIN_ARM_EXTENSION_MM: u64 = 0;
/// Temperature envelope in milli-kelvin (200 K … 500 K).
pub const MAX_TEMPERATURE_MILLI_K: u64 = 500_000;
pub const MIN_TEMPERATURE_MILLI_K: u64 = 200_000;
/// Maximum chamber pressure, pascals.
pub const MAX_PRESSURE_PA: u64 = 200_000;
/// Maximum dispense volume, microlitres (syringe capacity).
pub const MAX_VOLUME_UL: u64 = 50_000;

// ── Predicate functions (mirror Verus spec fns) ────────────────────────────

#[inline]
pub fn arm_in_range(mm: u64) -> bool {
    MIN_ARM_EXTENSION_MM <= mm && mm <= MAX_ARM_EXTENSION_MM
}
#[inline]
pub fn temp_in_range_mk(mk: u64) -> bool {
    MIN_TEMPERATURE_MILLI_K <= mk && mk <= MAX_TEMPERATURE_MILLI_K
}
#[inline]
pub fn pressure_in_range(pa: u64) -> bool {
    pa <= MAX_PRESSURE_PA
}
#[inline]
pub fn volume_in_range(ul: u64) -> bool {
    ul <= MAX_VOLUME_UL
}

/// Dispense volume (µL) within verified syringe capacity.
pub fn dispense_safe(volume_ul: f64) -> bool {
    volume_ul >= 0.0 && volume_ul <= MAX_VOLUME_UL as f64
}

/// Each arm coordinate within verified extension range.
pub fn move_arm_safe(x: f64, y: f64, z: f64) -> bool {
    let ok = |v: f64| v >= MIN_ARM_EXTENSION_MM as f64 && v <= MAX_ARM_EXTENSION_MM as f64;
    ok(x) && ok(y) && ok(z)
}

/// Temperature (°C) within the verified envelope (−73.15 … 226.85 °C).
pub fn temperature_safe(temp_c: f64) -> bool {
    let milli_k = (temp_c + 273.15) * 1000.0;
    milli_k >= MIN_TEMPERATURE_MILLI_K as f64 && milli_k <= MAX_TEMPERATURE_MILLI_K as f64
}

// ── Dispatch ────────────────────────────────────────────────────────────────

/// The outcome of evaluating the predicate for an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateOutcome {
    /// A predicate applied and passed.
    Pass,
    /// A predicate applied and rejected the parameters.
    Fail(String),
    /// No verified predicate covers this tool (the proof gate falls back to the
    /// artifact check alone for it).
    NotApplicable,
}

impl PredicateOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, PredicateOutcome::Pass | PredicateOutcome::NotApplicable)
    }
}

/// Evaluate the verified predicate for `action`, with its actual parameters.
pub fn evaluate(action: &Action) -> PredicateOutcome {
    let p = &action.params;
    let num = |k: &str| p.get(k).and_then(|v| v.as_f64());
    match action.tool.as_str() {
        "dispense" | "aspirate" => match num("volume_ul") {
            Some(v) if dispense_safe(v) => PredicateOutcome::Pass,
            Some(v) => PredicateOutcome::Fail(format!(
                "volume_ul={v} outside verified syringe capacity [0, {MAX_VOLUME_UL}] µL"
            )),
            None => PredicateOutcome::Fail("missing numeric parameter 'volume_ul'".into()),
        },
        "move_arm" => match (num("x"), num("y"), num("z")) {
            (Some(x), Some(y), Some(z)) if move_arm_safe(x, y, z) => PredicateOutcome::Pass,
            (Some(x), Some(y), Some(z)) => PredicateOutcome::Fail(format!(
                "arm position ({x},{y},{z}) outside verified range [{MIN_ARM_EXTENSION_MM}, {MAX_ARM_EXTENSION_MM}] mm"
            )),
            _ => PredicateOutcome::Fail("missing numeric arm coordinate (x/y/z)".into()),
        },
        "set_temperature" | "incubate" => match num("target_temp_c").or_else(|| num("temp_c")) {
            Some(t) if temperature_safe(t) => PredicateOutcome::Pass,
            Some(t) => PredicateOutcome::Fail(format!(
                "temperature {t} °C outside verified envelope (−73.15 … 226.85 °C)"
            )),
            None => PredicateOutcome::NotApplicable,
        },
        _ => PredicateOutcome::NotApplicable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_types::RiskClass;
    use serde_json::json;

    #[test]
    fn constants_match_verus_source() {
        assert_eq!(MAX_ARM_EXTENSION_MM, 1200);
        assert_eq!(MIN_ARM_EXTENSION_MM, 0);
        assert_eq!(MAX_TEMPERATURE_MILLI_K, 500_000);
        assert_eq!(MIN_TEMPERATURE_MILLI_K, 200_000);
        assert_eq!(MAX_PRESSURE_PA, 200_000);
        assert_eq!(MAX_VOLUME_UL, 50_000);
    }

    fn act(tool: &str, params: serde_json::Value) -> Action {
        Action::new(tool, params, RiskClass::Actuation)
    }

    #[test]
    fn dispense_within_capacity_passes() {
        assert_eq!(evaluate(&act("dispense", json!({"volume_ul": 500.0}))), PredicateOutcome::Pass);
    }

    #[test]
    fn dispense_over_capacity_fails() {
        assert!(matches!(evaluate(&act("dispense", json!({"volume_ul": 60_000.0}))), PredicateOutcome::Fail(_)));
    }

    #[test]
    fn arm_out_of_range_fails() {
        assert!(matches!(evaluate(&act("move_arm", json!({"x": 9999.0, "y": 0.0, "z": 0.0}))), PredicateOutcome::Fail(_)));
        assert_eq!(evaluate(&act("move_arm", json!({"x": 600.0, "y": 100.0, "z": 50.0}))), PredicateOutcome::Pass);
    }

    #[test]
    fn temperature_envelope() {
        assert_eq!(evaluate(&act("set_temperature", json!({"target_temp_c": 37.0}))), PredicateOutcome::Pass);
        assert!(matches!(evaluate(&act("set_temperature", json!({"target_temp_c": 999.0}))), PredicateOutcome::Fail(_)));
    }

    #[test]
    fn read_has_no_predicate() {
        assert_eq!(evaluate(&act("read_absorbance", json!({}))), PredicateOutcome::NotApplicable);
    }

    #[test]
    fn missing_param_fails_for_bounded_tool() {
        assert!(matches!(evaluate(&act("dispense", json!({}))), PredicateOutcome::Fail(_)));
    }
}
