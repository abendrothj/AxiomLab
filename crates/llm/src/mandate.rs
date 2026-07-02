//! The mandate — the full prompt, rebuilt fresh each iteration from the audit
//! chain and lab state. No hypothesis tracking, no finding counts, no journal.

use axiom_gate::{GateContext, latest_valid_until};

const INSTRUMENTS: &[&str] = &["spectrophotometer", "ph_meter", "thermal_controller"];

/// Build the mandate for this iteration.
///
/// `last_rejection`, when present, is the reason the previous proposal was
/// rejected — surfaced prominently so the model can correct course.
pub fn build_mandate(directive: &str, ctx: &GateContext, last_rejection: Option<&str>) -> String {
    let now = now_secs();
    let mut m = String::new();

    m.push_str(
        "You operate an autonomous laboratory by proposing instrument actions. A fail-closed \
         safety pipeline (capability, chemistry, calibration, proof, approval) checks every action \
         before any hardware moves. If an action is rejected you are told why and may revise; \
         repeated rejections abort the run. Propose only safe, in-bounds steps.\n\n",
    );

    m.push_str("# Directive\n");
    m.push_str(directive);
    m.push_str("\n\n");

    if let Some(reason) = last_rejection {
        m.push_str("# ⚠ Your previous proposal was REJECTED\n");
        m.push_str(reason);
        m.push_str("\nDo not repeat it. Propose a different, compliant action.\n\n");
    }

    m.push_str("# Operating rules\n");
    m.push_str(
        "1. Reply with EXACTLY ONE JSON object — no prose, no markdown fences.\n\
         2. Keep every parameter within the capability bounds below.\n\
         3. A measurement tool (read_absorbance/read_ph/read_temperature) is blocked until its \
            instrument has a valid calibration. Calibrate first if needed.\n\
         4. To calibrate, run analyze_series over CERTIFIED REFERENCE STANDARDS: pass \
            reference_material_ids (one registered standard per x value, ≥5 distinct levels). \
            Calibration also requires operator approval.\n\
         5. When the directive is satisfied, reply with the done tool.\n\n",
    );

    m.push_str("# Tools (reply with one of these shapes)\n");
    m.push_str(
        "propose_protocol — run a sequence of actions:\n\
         {\"tool\":\"propose_protocol\",\"steps\":[\
         {\"tool\":\"dispense\",\"params\":{\"vessel_id\":\"tube_1\",\"reagent\":\"water\",\"volume_ul\":50}}]}\n\
         analyze_series — fit data / calibrate an instrument:\n\
         {\"tool\":\"analyze_series\",\"x\":[1,2,3,4,5],\"y\":[0.1,0.2,0.3,0.4,0.5],\"model\":\"linear\",\
         \"instrument\":\"spectrophotometer\",\"reference_material_ids\":[\"std-1\",\"std-2\",\"std-3\",\"std-4\",\"std-5\"]}\n\
         done — finish:\n\
         {\"tool\":\"done\",\"summary\":\"what was accomplished\"}\n\n",
    );

    m.push_str("# Valid action tools (use EXACTLY these names inside propose_protocol steps)\n");
    m.push_str("- dispense (params: vessel_id, reagent, volume_ul)\n");
    m.push_str("- aspirate (params: vessel_id, volume_ul)\n");
    m.push_str("- read_absorbance (params: vessel_id, wavelength_nm)\n");
    m.push_str("- read_ph (params: vessel_id)\n");
    m.push_str("- read_temperature (params: device_id)\n");
    m.push_str("- set_temperature (params: device_id, target_temp_c)\n");
    m.push_str("- incubate (params: device_id, temp_c, duration_s)\n");
    m.push_str("- centrifuge (params: rpm, duration_s)\n");
    m.push_str("- move_arm (params: x, y, z)\n");
    m.push_str("- calibrate (params: instrument)\n\n");

    m.push_str("# Capability bounds (operational)\n");
    m.push_str(&ctx.capability.describe());
    m.push('\n');

    m.push_str("# Calibration status\n");
    for inst in INSTRUMENTS {
        let status = match latest_valid_until(&ctx.audit_chain, inst) {
            Ok(Some(vu)) if vu > now => format!("valid for {}s", vu - now),
            Ok(Some(_)) => "EXPIRED — recalibrate before measuring".to_string(),
            Ok(None) => "none — measurement blocked until calibrated".to_string(),
            Err(_) => "unknown".to_string(),
        };
        m.push_str(&format!("- {inst}: {status}\n"));
    }
    m.push('\n');

    m.push_str("# Registered reference standards (use these IDs to calibrate)\n");
    let standards = ctx.lab_state.lock().unwrap().registered_reference_materials();
    if standards.is_empty() {
        m.push_str("- (none registered — calibration is not possible)\n");
    } else {
        let mut ids: Vec<String> = standards.into_iter().collect();
        ids.sort();
        m.push_str(&format!("- {}\n", ids.join(", ")));
    }
    m.push('\n');

    m.push_str("# Recent activity (last 5 audit entries)\n");
    match ctx.audit_chain.entries() {
        Ok(entries) => {
            if entries.is_empty() {
                m.push_str("- (none yet)\n");
            }
            for e in entries.iter().rev().take(5).rev() {
                m.push_str(&format!("- [{}] {} ({})\n", e.decision, e.action, short(&e.reason)));
            }
        }
        Err(e) => m.push_str(&format!("- (audit unavailable: {e})\n")),
    }
    m
}

fn short(s: &str) -> String {
    s.chars().take(120).collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}
