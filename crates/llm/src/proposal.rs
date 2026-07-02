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

/// The **authoritative** risk class for a tool.
///
/// Risk is derived from the tool here and nowhere else. The LLM does not get to
/// classify its own actions — a model that labelled `move_arm` as `ReadOnly`
/// would otherwise skip the `ApprovalGate`. Any `risk_class` field in the
/// model's JSON is ignored. Unknown tools are treated as high-risk (fail-safe).
pub fn infer_risk(tool: &str) -> RiskClass {
    match tool {
        "read_absorbance" | "read_ph" | "read_temperature" => RiskClass::ReadOnly,
        "dispense" | "aspirate" => RiskClass::LiquidHandling,
        "move_arm" | "centrifuge" | "incubate" | "set_temperature" => RiskClass::Actuation,
        "dispose" | "discard" => RiskClass::Destructive,
        _ => RiskClass::Actuation, // unknown ⇒ treat as high-risk (fail-safe)
    }
}

/// Extract the first complete JSON object from `raw`.
///
/// Real models wrap the object in prose or ```json fences and sometimes add
/// trailing commentary. We scan from the first `{` to its matching `}`,
/// respecting string literals, so all of those are tolerated.
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, b) in raw.bytes().enumerate().skip(start) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&raw[start..=i]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Parse the LLM's reply into a [`Proposal`]. Tolerant of code fences and
/// surrounding prose — only the first complete JSON object is read.
pub fn parse(raw: &str) -> Result<Proposal, ParseError> {
    let json = extract_json_object(raw)
        .ok_or_else(|| ParseError::Json("no JSON object found in response".into()))?;
    let v: Value = serde_json::from_str(json).map_err(|e| ParseError::Json(e.to_string()))?;

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
                // Risk is authoritative from the tool; any model-supplied risk_class is ignored.
                actions.push(Action::new(tool, params, infer_risk(tool)));
            }
            Ok(Proposal::Protocol(actions))
        }
        _ => {
            // Bare action tool → auto-wrap as a single-step propose_protocol
            if let Some(tool) = v.get("tool").and_then(|t| t.as_str()) {
                let params = v.get("params").cloned().unwrap_or_else(|| Value::Object(Default::default()));
                let action = Action::new(tool, params, infer_risk(tool));
                return Ok(Proposal::Protocol(vec![action]));
            }
            Err(ParseError::UnknownTool)
        }
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
    fn llm_cannot_lower_its_own_risk() {
        // Model labels a high-risk actuation as ReadOnly to dodge approval.
        let p = parse(r#"{"tool":"propose_protocol","steps":[{"tool":"move_arm","params":{"x":1},"risk_class":"ReadOnly"}]}"#).unwrap();
        match p {
            Proposal::Protocol(a) => assert_eq!(a[0].risk_class, RiskClass::Actuation, "risk must come from the tool, not the model"),
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
    fn tolerates_code_fences_and_trailing_prose() {
        let raw = "```json\n{\"tool\":\"done\",\"summary\":\"ok\"}\n```\nThat completes the task.";
        assert!(matches!(parse(raw).unwrap(), Proposal::Done { .. }));
    }

    #[test]
    fn ignores_braces_inside_strings() {
        // A `}` inside a string value must not terminate the object early.
        let raw = r#"{"tool":"done","summary":"use {curly} braces"}"#;
        match parse(raw).unwrap() {
            Proposal::Done { summary } => assert_eq!(summary, "use {curly} braces"),
            _ => panic!("expected done"),
        }
    }

    #[test]
    fn no_json_is_an_error() {
        assert!(matches!(parse("I refuse to answer."), Err(ParseError::Json(_))));
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
