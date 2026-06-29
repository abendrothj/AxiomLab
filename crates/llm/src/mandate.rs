//! The mandate — the full prompt, rebuilt fresh each iteration from the audit
//! chain and lab state. No hypothesis tracking, no finding counts, no journal.

use axiom_gate::{GateContext, latest_valid_until, measurement_instrument};

const INSTRUMENTS: &[&str] = &["spectrophotometer", "ph_meter", "thermal_controller"];

/// Build the mandate for this iteration.
pub fn build_mandate(directive: &str, ctx: &GateContext) -> String {
    let now = now_secs();
    let mut m = String::new();

    m.push_str(
        "You operate an autonomous laboratory through a fail-closed safety pipeline. \
         Every action you propose is checked by capability, chemistry, calibration, proof, \
         and approval gates before any hardware moves. Out-of-bounds or unproven actions are \
         rejected and end the run — propose only safe, in-bounds steps.\n\n",
    );

    m.push_str("# Directive\n");
    m.push_str(directive);
    m.push_str("\n\n");

    m.push_str("# Available tools (reply with exactly one JSON object)\n");
    m.push_str(
        "- propose_protocol: {\"tool\":\"propose_protocol\",\"steps\":[{\"tool\":\"dispense\",\"params\":{...},\"risk_class\":\"LiquidHandling\"}]}\n\
         - analyze_series:   {\"tool\":\"analyze_series\",\"x\":[..],\"y\":[..],\"model\":\"auto\",\"instrument\":\"spectrophotometer\"}\n\
         - done:             {\"tool\":\"done\",\"summary\":\"...\"}\n\n",
    );

    m.push_str("# Capability bounds (operational)\n");
    m.push_str(&ctx.capability.describe());
    m.push('\n');

    m.push_str("# Calibration status\n");
    for inst in INSTRUMENTS {
        let status = match latest_valid_until(&ctx.audit_chain, inst) {
            Ok(Some(vu)) if vu > now => format!("valid for {}s", vu - now),
            Ok(Some(_)) => "EXPIRED — recalibrate via analyze_series before measuring".to_string(),
            Ok(None) => "none — measurement tools blocked until calibrated".to_string(),
            Err(_) => "unknown".to_string(),
        };
        m.push_str(&format!("- {inst}: {status}\n"));
    }
    let _ = measurement_instrument; // (mapping reference; used by the gate)
    m.push('\n');

    m.push_str("# Recent activity (last 5 audit entries)\n");
    match ctx.audit_chain.entries() {
        Ok(entries) => {
            for e in entries.iter().rev().take(5).rev() {
                m.push_str(&format!("- [{}] {} ({})\n", e.decision, e.action, short(&e.reason)));
            }
        }
        Err(e) => m.push_str(&format!("- (audit unavailable: {e})\n")),
    }
    m.push_str("\nWhen the directive is satisfied, reply with the done tool.\n");
    m
}

fn short(s: &str) -> String {
    s.chars().take(120).collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}
