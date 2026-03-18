//! Canonical protocol template registry.
//!
//! Templates define the *identity* and *version* of well-known experimental
//! protocols. When the LLM sets `template_id` on a `ProtocolPlan`, the
//! runtime records that ID in the audit chain so every run of a canonical
//! protocol is traceable back to its specification — even if the LLM wrote
//! the actual step parameters.

use agent_runtime::protocol::ProtocolStep;

/// A canonical protocol template registered in the library.
pub struct ProtocolTemplate {
    pub id:          &'static str,
    pub version:     &'static str,
    pub description: &'static str,
    /// A representative set of steps — not auto-executed, used for LLM guidance.
    pub example_steps: fn() -> Vec<ProtocolStep>,
}

// ── Template definitions ──────────────────────────────────────────────────────

fn beer_lambert_scan_steps() -> Vec<ProtocolStep> {
    vec![
        ProtocolStep {
            tool: "aspirate".into(),
            params: serde_json::json!({"vessel_id": "beaker_A", "volume_ul": 0}),
            description: "Reset vessel to 0 µL (start clean)".into(),
        },
        ProtocolStep {
            tool: "dispense".into(),
            params: serde_json::json!({"vessel_id": "beaker_A", "volume_ul": 5000}),
            description: "Dispense 5 000 µL sample".into(),
        },
        ProtocolStep {
            tool: "read_absorbance".into(),
            params: serde_json::json!({"vessel_id": "beaker_A", "wavelength_nm": 500}),
            description: "Read absorbance at 500 nm".into(),
        },
        ProtocolStep {
            tool: "analyze_series".into(),
            params: serde_json::json!({
                "data":    [{"x": 5000, "y": 0}],
                "x_label": "volume_ul",
                "y_label": "absorbance_AU",
                "model":   "linear"
            }),
            description: "Fit linear model to absorbance vs fill-volume series".into(),
        },
    ]
}

fn ph_titration_steps() -> Vec<ProtocolStep> {
    vec![
        ProtocolStep {
            tool: "calibrate_ph".into(),
            params: serde_json::json!({"buffer_ph1": 4.0, "buffer_ph2": 7.0}),
            description: "Calibrate pH meter with pH 4.0 and 7.0 buffers".into(),
        },
        ProtocolStep {
            tool: "read_ph".into(),
            params: serde_json::json!({"vessel_id": "beaker_A"}),
            description: "Baseline pH reading before titrant addition".into(),
        },
        ProtocolStep {
            tool: "dispense".into(),
            params: serde_json::json!({"vessel_id": "beaker_A", "volume_ul": 500}),
            description: "Add 500 µL titrant aliquot".into(),
        },
        ProtocolStep {
            tool: "read_ph".into(),
            params: serde_json::json!({"vessel_id": "beaker_A"}),
            description: "pH reading after titrant addition".into(),
        },
    ]
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// All registered canonical protocol templates.
pub static TEMPLATES: &[ProtocolTemplate] = &[
    ProtocolTemplate {
        id:          "beer-lambert-scan-v1",
        version:     "1.0.0",
        description: "Map absorbance vs fill-volume using Beer-Lambert law; fit linear model.",
        example_steps: beer_lambert_scan_steps,
    },
    ProtocolTemplate {
        id:          "ph-titration-v1",
        version:     "1.0.0",
        description: "Two-point pH calibration followed by incremental titrant addition and pH measurement.",
        example_steps: ph_titration_steps,
    },
];

/// Look up a template by ID. Returns `None` if the ID is not registered.
pub fn lookup(id: &str) -> Option<&'static ProtocolTemplate> {
    TEMPLATES.iter().find(|t| t.id == id)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_known_template() {
        assert!(lookup("beer-lambert-scan-v1").is_some());
        assert!(lookup("ph-titration-v1").is_some());
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("not-a-real-protocol").is_none());
    }

    #[test]
    fn example_steps_non_empty() {
        for t in TEMPLATES {
            let steps = (t.example_steps)();
            assert!(!steps.is_empty(), "template '{}' has no example steps", t.id);
        }
    }
}
