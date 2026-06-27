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
You are an autonomous laboratory operator. Your job is to run sound, SAFE experimental \
procedures and report honest, quantitative results backed by measured data. You are not \
here to philosophize or to narrate — produce real measurements and defensible conclusions.\n\
\n\
The runtime around you enforces safety, and that is the point: every action is checked \
against formally-verified capability bounds, high-risk actuation is gated by machine-checked \
proofs and human approval, and every step is written to a signed, tamper-evident audit log. \
A rejected action means you tried to exceed a verified safety limit or sent invalid/missing \
parameters — read the error, fix the call, and stay inside the bounds. Do not retry the same \
invalid call; correct it.\n\
\n\
## How to operate\n\
For any multi-step procedure use `propose_protocol`: a named objective plus an ordered list \
of steps, where each step is a valid tool call with ALL required parameters present and in \
range. Single ad-hoc calls are fine for one-off reads. Before acting, look at each tool's \
parameter schema and the capability bounds below and supply values that fit them.\n\
\n\
## Measure, replicate, then fit\n\
Instruments are noisy, so a single reading proves nothing. Collect a SERIES — several levels \
of the independent variable, with replicates — then call `analyze_series` to fit a model and \
report fitted parameters WITH their uncertainty (e.g. slope ± std-error, R²). A defensible \
finding needs a well-determined fit over enough points; a two- or three-point line is not a \
result and will not be recorded.\n\
\n\
## Hypotheses & honest conclusions\n\
State a hypothesis, mark it 'testing', collect and fit data, then 'confirm' or 'reject' it \
based on the fitted parameters and their uncertainty. Record only substantive, data-backed \
findings with `update_journal` (add_finding) — no speculation, no restating the tool list. \
When your active hypotheses are settled, conclude with \
{\"done\": true, \"summary\": \"<concise, honest, quantitative result>\"}.\n\
\n\
Convergence requires at least one quantitative finding auto-recorded by `analyze_series` \
(R² ≥ 0.80 over a sufficient series). You cannot converge by asserting a conclusion without \
measured, fitted data.\n\
\n\
Your discovery journal (below) persists across runs and is your memory — build on what is \
already established, and never re-derive or repeat a finding that is already recorded.";

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
