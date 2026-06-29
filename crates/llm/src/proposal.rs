//! What the LLM is allowed to propose, and how its JSON reply is parsed.
//!
//! The tool surface is deliberately tiny: `propose_protocol` and `analyze_series`
//! only. There is no `update_journal`, no `design_experiment`, no hypothesis or
//! finding machinery. A tool absent from this surface cannot be proposed.

use axiom_gate::AnalyzeRequest;
use axiom_types::{Action, RiskClass};
use serde_json::Value;

/// A single decoded LLM turn.
pub enum Proposal {
    /// An ordered list of actions to run through the pipeline.
    Protocol(Vec<Action>),
    /// A curve-fit request (records calibration on a good fit).
    Analyze(AnalyzeRequest),
    /// The LLM declares the directive complete.
    Done { summary: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("response was not valid JSON: {0}")]
    Json(String),
    #[error("missing or unrecognised 'tool' field (expected propose_protocol | analyze_series | done)")]
    UnknownTool,
    #[error("invalid {tool}: {detail}")]
    InvalidArgs { tool: &'static str, detail: String },
}

/// Default risk class for a tool when the model omits one.
pub fn infer_risk(tool: &str) -> RiskClass {
    match tool {
        "read_absorbance" | "read_ph" | "read_temperature" => RiskClass::ReadOnly,
        "dispense" | "aspirate" => RiskClass::LiquidHandling,
        "move_arm" | "centrifuge" | "incubate" | "set_temperature" => RiskClass::Actuation,
        "dispose" | "discard" => RiskClass::Destructive,
        _ => RiskClass::Actuation, // unknown ⇒ treat as high-risk (fail-safe)
    }
}

fn parse_risk(v: Option<&Value>, tool: &str) -> RiskClass {
    match v.and_then(|x| x.as_str()) {
        Some("ReadOnly") => RiskClass::ReadOnly,
        Some("LiquidHandling") => RiskClass::LiquidHandling,
        Some("Actuation") => RiskClass::Actuation,
        Some("Destructive") => RiskClass::Destructive,
        _ => infer_risk(tool),
    }
}

/// Parse the LLM's reply. The model is instructed to emit a single JSON object;
/// any prose preceding the first `{` is tolerated and discarded.
pub fn parse(raw: &str) -> Result<Proposal, ParseError> {
    let json_start = raw.find('{').unwrap_or(0);
    let v: Value = serde_json::from_str(raw[json_start..].trim()).map_err(|e| ParseError::Json(e.to_string()))?;

    match v.get("tool").and_then(|t| t.as_str()) {
        Some("done") => Ok(Proposal::Done {
            summary: v.get("summary").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        }),
        Some("analyze_series") => {
            let req: AnalyzeRequest = serde_json::from_value(v.clone())
                .map_err(|e| ParseError::InvalidArgs { tool: "analyze_series", detail: e.to_string() })?;
            Ok(Proposal::Analyze(req))
        }
        Some("propose_protocol") => {
            let steps = v
                .get("steps")
                .and_then(|s| s.as_array())
                .ok_or(ParseError::InvalidArgs { tool: "propose_protocol", detail: "missing 'steps' array".into() })?;
            let mut actions = Vec::with_capacity(steps.len());
            for step in steps {
                let tool = step
                    .get("tool")
                    .and_then(|t| t.as_str())
                    .ok_or(ParseError::InvalidArgs { tool: "propose_protocol", detail: "step missing 'tool'".into() })?;
                let params = step.get("params").cloned().unwrap_or_else(|| Value::Object(Default::default()));
                let risk = parse_risk(step.get("risk_class"), tool);
                actions.push(Action::new(tool, params, risk));
            }
            Ok(Proposal::Protocol(actions))
        }
        _ => Err(ParseError::UnknownTool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_done() {
        let p = parse(r#"{"tool":"done","summary":"all set"}"#).unwrap();
        assert!(matches!(p, Proposal::Done { summary } if summary == "all set"));
    }

    #[test]
    fn parses_protocol_with_inferred_risk() {
        let p = parse(r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"volume_ul":5}}]}"#).unwrap();
        match p {
            Proposal::Protocol(a) => {
                assert_eq!(a[0].tool, "dispense");
                assert_eq!(a[0].risk_class, RiskClass::LiquidHandling);
            }
            _ => panic!("expected protocol"),
        }
    }

    #[test]
    fn parses_analyze() {
        let p = parse(r#"{"tool":"analyze_series","x":[1,2],"y":[2,4],"instrument":"spectrophotometer"}"#).unwrap();
        assert!(matches!(p, Proposal::Analyze(_)));
    }

    #[test]
    fn tolerates_leading_prose() {
        let p = parse("Here is my plan: {\"tool\":\"done\",\"summary\":\"x\"}").unwrap();
        assert!(matches!(p, Proposal::Done { .. }));
    }

    #[test]
    fn unknown_tool_errors() {
        assert!(matches!(parse(r#"{"tool":"update_journal"}"#), Err(ParseError::UnknownTool)));
    }

    #[test]
    fn unknown_action_defaults_high_risk() {
        assert_eq!(infer_risk("frobnicate"), RiskClass::Actuation);
    }
}
