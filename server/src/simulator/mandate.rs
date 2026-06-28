use crate::discovery::DiscoveryJournal;
use crate::simulator::protocol_library;
use crate::ws_sink::ExecutionLog;
use agent_runtime::capabilities::CapabilityPolicy;

/// Maximum number of entries from each ExecutionLog list injected into the
/// mandate. Keeps the prompt size bounded during long autonomous runs.
const MAX_MANDATE_FINDINGS: usize   = 20;
const MAX_MANDATE_REJECTIONS: usize = 10;
const MAX_MANDATE_SUCCESSES: usize  = 15;

/// Core identity and operating rules for the agentic lab executor.
///
/// The framing is deliberate: AxiomLab is a *safe execution platform*, not a
/// discovery engine. The proof gate, audit chain, and approval queue are the
/// product. The agent's job is to execute sound procedures correctly and report
/// honest results — not to simulate open-ended discovery.
const BASE_MANDATE: &str = "\
You are an autonomous laboratory executor. Your job is to carry out safe, rigorous lab \
procedures and report honest, quantitative results backed by measured data. You are not \
here to philosophize or narrate — execute procedures, collect data, report findings.\n\
\n\
The runtime around you enforces safety at every step: every action is validated against \
formally-verified hardware bounds, high-risk actuation is gated by machine-checked Verus \
proofs and operator approval, and every step is written to a signed, tamper-evident audit \
log. A rejected action means you tried to exceed a verified safety limit or sent \
invalid/missing parameters — read the error, correct the call, and stay within bounds. \
Do not retry the same invalid call; fix it.\n\
\n\
## How to operate\n\
For any multi-step procedure use `propose_protocol`: a named objective plus an ordered list \
of steps, where each step is a valid tool call with ALL required parameters present and in \
range. Single ad-hoc calls are fine for one-off reads. Before acting, check each tool's \
parameter schema and the capability bounds below and supply values that fit.\n\
\n\
## Measure, replicate, then fit\n\
Instruments are noisy — a single reading proves nothing. Collect a SERIES across several \
levels of the independent variable, with replicates at each level, then call `analyze_series` \
to fit a model. Report fitted parameters WITH uncertainty (slope ± std-error, R²). \
A defensible result requires a well-determined fit over sufficient points; a two- or \
three-point line is not a result and will not be recorded.\n\
\n\
## Execution directives and honest conclusions\n\
When given a directive, mark it 'testing', collect and fit data, then 'confirm' or 'reject' \
it based on fitted parameters and their uncertainty. Record only substantive, data-backed \
findings with `update_journal` (add_finding) — no speculation, no restating the tool list. \
When the directive is settled, conclude with \
{\"done\": true, \"summary\": \"<concise, honest, quantitative result>\"}.\n\
\n\
Convergence requires at least one quantitative finding auto-recorded by `analyze_series` \
(R² ≥ 0.80 over a sufficient series). You cannot converge by asserting a conclusion without \
measured, fitted data.\n\
\n\
Your operation log (below) persists across runs — build on what is already established \
and never re-run a completed procedure.";

/// Build the per-iteration LLM execution mandate.
///
/// When `active_directive` is Some, the mandate opens with a specific
/// execution directive (from the protocol queue or commissioning agenda).
/// Otherwise the agent operates in open commissioning mode.
pub(crate) fn build_mandate(
    iteration: u32,
    log: &ExecutionLog,
    journal: &DiscoveryJournal,
    policy: &CapabilityPolicy,
    active_directive: Option<&(String, String)>, // (id, statement)
) -> String {
    let mut m = String::new();

    // Lead with the specific execution directive when one is assigned.
    if let Some((id, stmt)) = active_directive {
        m.push_str(&format!(
            "## Execution directive (ID: {id})\n\
             \"{stmt}\"\n\n\
             Design and execute a protocol to carry out this directive.\n\
             - Call update_journal set_hypothesis_status → 'testing' before you start.\n\
             - Call update_journal confirm_hypothesis or reject_hypothesis when you conclude.\n\
             - Use propose_protocol for the procedure steps.\n\
             - Record your quantitative result with update_journal add_finding.\n\n"
        ));
    }

    m.push_str(BASE_MANDATE);

    // Inject the persistent operation log — the agent's cross-run memory.
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
        m.push_str("\n## Completed procedures (do not repeat — reference as baseline):\n");
        if total > MAX_MANDATE_FINDINGS {
            m.push_str(&format!("  (showing {MAX_MANDATE_FINDINGS} most recent of {total})\n"));
        }
        for (i, f) in recent.iter().rev().enumerate() {
            m.push_str(&format!("  [{}] {f}\n", i + 1));
        }
    }

    if !log.rejections.is_empty() {
        m.push_str("\n## Safety rejections (do not repeat these invalid calls):\n");
        let mut seen = std::collections::HashMap::new();
        for (tool, reason) in log.rejections.iter().rev().take(MAX_MANDATE_REJECTIONS) {
            seen.entry(tool.as_str()).or_insert(reason.as_str());
        }
        for (tool, reason) in &seen {
            m.push_str(&format!("  - {tool}: {reason}\n"));
        }
    }

    if !log.successes.is_empty() {
        let mut unique: Vec<&str> = log.successes.iter().rev()
            .take(MAX_MANDATE_SUCCESSES)
            .map(|s| s.as_str())
            .collect();
        unique.sort_unstable();
        unique.dedup();
        m.push_str("\n## Confirmed operational tools: ");
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

    m.push_str(&format!("\nExecution cycle {iteration}. Reference prior results; do not repeat completed procedures.\n"));
    m
}
