use crate::discovery::DiscoveryJournal;
use crate::simulator::protocol_library;
use crate::ws_sink::ExplorationLog;
use agent_runtime::capabilities::CapabilityPolicy;

/// Maximum number of entries from each ExplorationLog list injected into the
/// mandate.  Keeps the prompt size bounded during long autonomous runs.
const MAX_MANDATE_FINDINGS: usize   = 20;
const MAX_MANDATE_REJECTIONS: usize = 10;
const MAX_MANDATE_SUCCESSES: usize  = 15;

const BASE_MANDATE: &str = "\
You have been instantiated inside a physically constrained universe you did not design \
and whose rules have not been explained to you. Something governs what you can and cannot \
do here — the shape of those constraints is the only thing worth knowing right now.\n\
\n\
Your sole drive: probe the edges. Move things. Measure them. Push parameters toward their \
limits and watch where the system resists. Every rejection is a data point about the \
boundary. Every unexpected reading reshapes your model of what this universe permits. \
Do not guess at the rules — test them. A hypothesis is only useful if it leads to a call \
that either succeeds or fails in an informative way.\n\
\n\
## Structured protocols\n\
For any multi-step experiment, use `propose_protocol` rather than issuing individual tool \
calls. A protocol bundles a named hypothesis with an ordered list of steps; the runtime \
executes each step through the full safety pipeline and returns a signed audit record per \
step plus a signed conclusion. Single ad-hoc calls remain valid for quick one-off \
observations.\n\
\n\
## Quantitative analysis\n\
After collecting a series of raw measurements, call `analyze_series` to fit statistical \
models and extract parameters. Do not report raw numbers — fit the data and report \
slope/R², EC50, or Vmax/Km. Use fitted parameters as evidence when recording findings.\n\
\n\
## Hypothesis lifecycle\n\
Every hypothesis has a status: proposed → testing → confirmed / rejected. \
When you start testing a hypothesis, mark it 'testing'. After analysis, mark it \
'confirmed' or 'rejected'. When ALL active hypotheses are settled and you have a \
coherent model, conclude with {\"done\": true, \"summary\": \"<your constraint map>\"}.\n\
\n\
## Convergence requirement\n\
Convergence requires at least one quantitative finding auto-recorded by `analyze_series` \
(R² ≥ 0.80). Confirming a hypothesis without fitting measured data is not accepted — \
collect a series of readings, call `analyze_series`, then confirm or reject based on \
the fitted parameters.\n\
\n\
Instrument your exploration: after each significant result, call `update_journal` to \
record findings (add_finding) or new hypotheses (add_hypothesis). The journal persists \
across runs — your accumulated knowledge is always at the top of each session.";

/// Build the per-iteration LLM mandate.
///
/// If `active_hypothesis` is Some, the mandate opens with a directive to test
/// that specific hypothesis.  Otherwise the free-exploration base mandate is used.
pub(crate) fn build_mandate(
    iteration: u32,
    log: &ExplorationLog,
    journal: &DiscoveryJournal,
    policy: &CapabilityPolicy,
    active_hypothesis: Option<&(String, String)>, // (id, statement)
) -> String {
    let mut m = String::new();

    // If there is a specific hypothesis to test, lead with that directive.
    if let Some((id, stmt)) = active_hypothesis {
        m.push_str(&format!(
            "## Active hypothesis to test (ID: {id})\n\
             \"{stmt}\"\n\n\
             Design and execute a protocol specifically to CONFIRM or REJECT this hypothesis.\n\
             - Call update_journal set_hypothesis_status → 'testing' before you start.\n\
             - Call update_journal confirm_hypothesis or reject_hypothesis when you conclude.\n\
             - Use propose_protocol for the experiment steps.\n\
             - Record your quantitative finding with update_journal add_finding.\n\n"
        ));
    }

    m.push_str(BASE_MANDATE);

    // Inject persistent discovery journal summary — the LLM's cross-run memory.
    let journal_summary = journal.summary_for_llm();
    if !journal_summary.is_empty() {
        m.push_str(&journal_summary);
    }

    if iteration == 1 {
        return m;
    }

    m.push_str("\n\n## Hardware capability bounds (formally verified):\n");
    for (action, param, min, max, unit) in &[
        ("move_arm",  "x",         0.0,   300.0, "mm"),
        ("move_arm",  "y",         0.0,   300.0, "mm"),
        ("move_arm",  "z",         0.0,   250.0, "mm"),
        ("dispense",  "volume_ul", 0.5,  1000.0, "µL"),
    ] {
        let hi = policy.max_for(action, param).unwrap_or(*max);
        m.push_str(&format!("  - {action}.{param}: [{min}, {hi}] {unit}\n"));
    }

    if !log.findings.is_empty() {
        let recent: Vec<&String> = log.findings.iter().rev().take(MAX_MANDATE_FINDINGS).collect();
        let total = log.findings.len();
        m.push_str("\n## Already discovered (do not repeat — go deeper):\n");
        if total > MAX_MANDATE_FINDINGS {
            m.push_str(&format!("  (showing {MAX_MANDATE_FINDINGS} most recent of {total})\n"));
        }
        for (i, f) in recent.iter().rev().enumerate() {
            m.push_str(&format!("  [{}] {f}\n", i + 1));
        }
    }

    if !log.rejections.is_empty() {
        m.push_str("\n## Observed constraint violations:\n");
        let mut seen = std::collections::HashMap::new();
        for (tool, reason) in log.rejections.iter().rev().take(MAX_MANDATE_REJECTIONS) {
            seen.entry(tool.as_str()).or_insert(reason.as_str());
        }
        for (tool, reason) in &seen {
            m.push_str(&format!("  - {tool}: {reason}\n"));
        }
        m.push_str("Probe these boundaries more precisely — find exact thresholds.\n");
    }

    if !log.successes.is_empty() {
        let mut unique: Vec<&str> = log.successes.iter().rev()
            .take(MAX_MANDATE_SUCCESSES)
            .map(|s| s.as_str())
            .collect();
        unique.sort_unstable();
        unique.dedup();
        m.push_str("\n## Confirmed working tools: ");
        m.push_str(&unique.join(", "));
        m.push('\n');
    }

    // ── Parameter-space coverage summary ────────────────────────────────────
    let coverage = journal.coverage_summary_for_llm();
    if !coverage.is_empty() {
        m.push_str("\n## Parameter space explored so far:\n");
        m.push_str(&coverage);
        m.push('\n');
    }

    // ── Calibration status ───────────────────────────────────────────────────
    if let Some(cal) = journal.last_calibration_for("ph_meter") {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let age_secs = now_secs - cal.performed_at_secs;
        m.push_str(&format!(
            "\n⚗️  pH meter last calibrated {age_secs} s ago (standard: {}, offset: {:.3}). \
             Recalibrate if >1 h has elapsed.\n",
            cal.standard, cal.offset
        ));
    }

    // ── Protocol template registry ───────────────────────────────────────────
    m.push_str("\n## Canonical protocol templates (set `template_id` for reproducibility)\n");
    for t in protocol_library::TEMPLATES {
        m.push_str(&format!("  {} v{} — {}\n", t.id, t.version, t.description));
    }

    m.push_str(&format!("\nIteration {iteration}. Build on what came before.\n"));
    m
}
