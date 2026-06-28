mod manifest;
mod mandate;
pub mod protocol_library;
mod tools;

use crate::discovery::HypothesisStatus;
use crate::protocol_queue::ProtocolQueue;
use crate::ws_sink::WebSocketSink;
use agent_runtime::{
    approval_queue::PendingApprovalQueue,
    capabilities::CapabilityPolicy,
    events::EventSink,
    experiment::{Experiment, Stage},
    hypothesis::HypothesisManager,
    lab_state::LabState,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
};
use crate::discovery::DiscoveryJournal;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};

// ── Execution loop pacing ─────────────────────────────────────────────────────

/// Tunable execution-loop pacing (all overridable via env).
struct LoopConfig {
    max_iterations:        u32,
    inter_run_pause_secs:  u64,
    idle_pause_secs:       u64,
    backoff_base_secs:     u64,
    backoff_max_secs:      u64,
}

impl LoopConfig {
    fn from_env() -> Self {
        Self {
            max_iterations: env_u32("AXIOMLAB_MAX_ITERATIONS", 10).clamp(3, 30),
            inter_run_pause_secs: env_u64("AXIOMLAB_EXPERIMENT_PAUSE_SECS", 120).clamp(10, 3600),
            idle_pause_secs: env_u64("AXIOMLAB_IDLE_PAUSE_SECS", 300).clamp(60, 3600),
            backoff_base_secs: env_u64("AXIOMLAB_EXHAUST_BACKOFF_BASE_SECS", 60).clamp(10, 600),
            backoff_max_secs: env_u64("AXIOMLAB_EXHAUST_BACKOFF_MAX_SECS", 300).clamp(60, 3600),
        }
    }

    /// Exponential backoff after consecutive max-iteration exhaustions (60 → 120 → 240 → 300 cap).
    fn backoff_secs(&self, consecutive_exhaustions: u32) -> u64 {
        if consecutive_exhaustions == 0 {
            return self.inter_run_pause_secs;
        }
        let exp = consecutive_exhaustions.saturating_sub(1).min(4);
        let scaled = self.backoff_base_secs.saturating_mul(1u64 << exp);
        scaled.min(self.backoff_max_secs)
    }
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// True when the journal already has findings but nothing is actively being tested.
fn should_idle_exploration(journal: &DiscoveryJournal) -> bool {
    if journal.findings.is_empty() {
        return false;
    }
    !journal.hypotheses.iter().any(|h| {
        h.status == HypothesisStatus::Proposed || h.status == HypothesisStatus::Testing
    })
}

fn experiment_exhausted_iterations(experiment: &Experiment) -> bool {
    experiment.stage == Stage::Failed
        && experiment
            .error
            .as_deref()
            .is_some_and(|e| e.contains("max orchestrator iterations"))
}

/// Deterministic science agenda.  Each entry is (dedup_key, hypothesis_statement).
/// `dedup_key` must appear verbatim in the statement so the existence check works.
const SCIENCE_AGENDA: &[(&str, &str)] = &[
    (
        "ph-titration-capacity",
        "pH titration — buffer capacity [ph-titration-capacity]: run calibrate_ph, \
         then add NaOH in ≥6 equal increments (50–300 µL into a water baseline) and \
         measure pH after each addition. Fit pH vs cumulative NaOH volume (linear model). \
         Report slope ± std-error and R².",
    ),
    (
        "beer-lambert-upper-range",
        "Beer-Lambert extended range [beer-lambert-upper-range]: scan absorbance at ≥8 \
         fill volumes across [100, 1000] µL (evenly spaced). Fit the linear model and \
         compare the slope to the previously established ~2.38×10⁻⁵ AU/µL baseline. \
         Report updated slope ± std-error and R².",
    ),
    (
        "incubator-temperature-linearity",
        "Incubator setpoint accuracy [incubator-temperature-linearity]: use \
         read_temperature at 5 setpoints — trigger each with a dispense step, then \
         read. Setpoints: 25, 30, 35, 37, 40 °C. Fit measured vs nominal temperature; \
         report offset, slope, and R².",
    ),
    (
        "ph-absorbance-coupling",
        "pH–absorbance coupling [ph-absorbance-coupling]: prepare 5 solutions at NaOH \
         volumes 50, 100, 150, 200, 250 µL. Measure both pH and absorbance at each level. \
         Fit absorbance vs pH and report the correlation coefficient with R².",
    ),
    (
        "arm-workspace-boundary",
        "Arm workspace boundary [arm-workspace-boundary]: map the outer boundary of the \
         safe arm workspace by issuing move_arm calls at x ∈ {0, 75, 150, 225, 300} mm \
         and y ∈ {0, 75, 150, 225, 300} mm (z fixed at 100 mm). Record which succeed; \
         report the confirmed reachable area as a fraction of the declared 300×300 mm envelope.",
    ),
];

/// Propose the next untried hypothesis from the science agenda.
/// Returns true when a hypothesis was added (and the journal saved).
fn seed_follow_up_hypothesis(journal: &mut crate::discovery::DiscoveryJournal) -> bool {
    for (key, stmt) in SCIENCE_AGENDA {
        let already_exists = journal.hypotheses.iter().any(|h| h.statement.contains(key));
        if !already_exists {
            journal.add_hypothesis(stmt.to_string());
            tracing::info!(key = key, "Seeded follow-up hypothesis from science agenda");
            if let Err(e) = journal.save(&crate::discovery::journal_path()) {
                tracing::warn!("Failed to persist journal after hypothesis seed: {e}");
            }
            return true;
        }
    }
    tracing::warn!("Science agenda exhausted — all planned hypotheses have been proposed");
    false
}

// ── Slot manager ──────────────────────────────────────────────────────────────

struct SlotManager {
    slot_count: usize,
    available:  std::sync::atomic::AtomicUsize,
}

impl SlotManager {
    fn from_env() -> Self {
        let count = std::env::var("AXIOMLAB_EXPERIMENT_SLOTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
            .clamp(1, 4);
        Self { slot_count: count, available: std::sync::atomic::AtomicUsize::new(count) }
    }

    fn available_slots(&self) -> usize {
        self.available.load(Ordering::SeqCst)
    }

    fn try_acquire(&self, _exp_id: &str, _instruments: &[&str]) -> Option<usize> {
        self.available.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
            if v > 0 { Some(v - 1) } else { None }
        }).ok().map(|prev| self.slot_count - prev)
    }

    fn release(&self, _slot: usize, _instruments: &[&str]) {
        self.available.fetch_add(1, Ordering::SeqCst);
    }
}

// ── Per-experiment task state ─────────────────────────────────────────────────

/// All state needed to run a single experiment in a task.
/// Every field is either `Arc<…>` (shared) or owned (cloned per-task).
struct ExperimentTask {
    slot:               usize,
    experiment_id:      String,
    mandate:            String,
    iteration:          u32,
    config:             OrchestratorConfig,
    sink:               Arc<WebSocketSink>,
    sila_clients:       Option<Arc<agent_runtime::hardware::SiLA2Clients>>,
    engine:             proof_artifacts::policy::RuntimePolicyEngine,
    exec_ctx:           proof_artifacts::policy::ExecutionContext,
    db:                 Arc<crate::db::Db>,
    approval_queue:     Arc<PendingApprovalQueue>,
    lab_state:          Arc<Mutex<LabState>>,
    hypothesis_manager: Arc<Mutex<HypothesisManager>>,
}

/// Result returned from a completed experiment task.
struct TaskResult {
    slot:                  usize,
    experiment_id:         String,
    converged:             bool,
    exhausted_iterations:  bool,
}

// ── Single-experiment runner ───────────────────────────────────────────────────

async fn run_one_experiment(task: ExperimentTask) -> TaskResult {
    let llm = match OpenAiClient::from_env() {
        Ok(c)  => c,
        Err(e) => {
            tracing::error!("LLM init failed for slot {}: {e}", task.slot);
            return TaskResult {
                slot: task.slot,
                experiment_id: task.experiment_id.clone(),
                converged: false,
                exhausted_iterations: false,
            };
        }
    };

    let tools = match &task.sila_clients {
        Some(clients) => tools::make_sila2_tools(
            Arc::clone(clients),
            Arc::clone(&task.sink.journal),
            Arc::clone(&task.db),
            Arc::clone(&task.lab_state),
        ),
        None => tools::make_sim_tools(
            Arc::clone(&task.sink.journal),
            Arc::clone(&task.db),
            Arc::clone(&task.lab_state),
        ),
    };

    let mut experiment = Experiment::new(task.experiment_id.clone(), &task.mandate);
    let orchestrator   = Orchestrator::new(llm, tools::make_sandbox(), tools, task.config)
        .with_runtime_policy(task.engine.clone(), task.exec_ctx.clone());

    if let Err(e) = orchestrator.run_experiment(&mut experiment).await {
        tracing::error!("Slot {} experiment {} error: {e}", task.slot, task.experiment_id);
    }

    // Convergence check — all hypotheses settled + at least one system-generated finding.
    // A "system" finding is auto-recorded by `analyze_series` (R² ≥ 0.80); the LLM
    // cannot fake convergence by calling `confirm_hypothesis` without measured data.
    let converged = {
        let j = task.sink.journal.lock().unwrap();
        let has_system_finding = j.findings.iter()
            .any(|f| f.source == "system");
        has_system_finding
            && !j.hypotheses.is_empty()
            && j.hypotheses.iter().all(|h| {
                h.status == HypothesisStatus::Confirmed
                    || h.status == HypothesisStatus::Rejected
            })
    };

    TaskResult {
        slot: task.slot,
        experiment_id: task.experiment_id.clone(),
        converged,
        exhausted_iterations: experiment_exhausted_iterations(&experiment),
    }
}

async fn pause_after_run(
    res: &TaskResult,
    sink: &WebSocketSink,
    loop_cfg: &LoopConfig,
    consecutive_exhaustions: &mut u32,
    slot_count: usize,
) {
    if slot_count != 1 {
        return;
    }

    if res.converged {
        *consecutive_exhaustions = 0;
        tracing::info!("Execution cycle converged — all directives settled. Pausing 60 s.");
        sink.set_loop_status(
            "converged",
            "All directives settled — pausing before next cycle",
            60,
        );
        sleep(Duration::from_secs(60)).await;
        return;
    }

    if res.exhausted_iterations {
        *consecutive_exhaustions = consecutive_exhaustions.saturating_add(1);
        let pause = loop_cfg.backoff_secs(*consecutive_exhaustions);
        tracing::info!(
            consecutive = *consecutive_exhaustions,
            pause_secs = pause,
            "Experiment hit max iterations — backing off before next run"
        );
        sink.set_loop_status(
            "backoff",
            format!(
                "Hit max iterations ({}) — backoff #{} before retry",
                loop_cfg.max_iterations, *consecutive_exhaustions
            ),
            pause,
        );
        sleep(Duration::from_secs(pause)).await;
        return;
    }

    if should_idle_exploration(&sink.journal.lock().unwrap()) {
        *consecutive_exhaustions = 0;
        let seeded = seed_follow_up_hypothesis(&mut sink.journal.lock().unwrap());
        if seeded {
            sink.set_loop_status("exploring", "New hypothesis seeded — queuing next experiment", 0);
            return;
        }
        let pause = loop_cfg.idle_pause_secs;
        tracing::info!(
            pause_secs = pause,
            "Science agenda exhausted — all planned experiments complete, extended idle"
        );
        sink.set_loop_status(
            "idle",
            "All planned experiments complete — idling for new questions",
            pause,
        );
        sleep(Duration::from_secs(pause)).await;
        return;
    }

    *consecutive_exhaustions = 0;
    let pause = loop_cfg.inter_run_pause_secs;
    sink.set_loop_status(
        "paused",
        format!("Cool-down between experiments ({pause}s configured)"),
        pause,
    );
    sleep(Duration::from_secs(pause)).await;
}

// ── Execution loop ────────────────────────────────────────────────────────────
//
// Priority order for each cycle:
//   1. Pending items in the operator protocol queue (highest priority)
//   2. Proposed directives in the journal (from the commissioning agenda)
//   3. Seed the next commissioning procedure from the science agenda
//
// The queue is the primary interface: operators push work here and the loop
// executes it. The science agenda is fallback commissioning that keeps the
// system characterizing instruments when no operator work is queued.

pub async fn run_loop(
    sink:               Arc<WebSocketSink>,
    running:            Arc<AtomicBool>,
    iteration_counter:  Arc<AtomicU32>,
    approval_queue:     Arc<PendingApprovalQueue>,
    db:                 Arc<crate::db::Db>,
    sila_clients:       Option<Arc<agent_runtime::hardware::SiLA2Clients>>,
    lab_state:          Arc<Mutex<LabState>>,
    hypothesis_manager: Arc<Mutex<HypothesisManager>>,
    protocol_queue:     Arc<Mutex<ProtocolQueue>>,
) {
    let policy     = CapabilityPolicy::default_lab();
    let scheduler  = SlotManager::from_env();
    let loop_cfg   = LoopConfig::from_env();

    tracing::info!(
        slot_count = scheduler.slot_count,
        max_iterations = loop_cfg.max_iterations,
        inter_run_pause_secs = loop_cfg.inter_run_pause_secs,
        idle_pause_secs = loop_cfg.idle_pause_secs,
        "Execution loop starting ({} concurrent experiment slot(s))",
        scheduler.slot_count,
    );

    if let Err(e) = OpenAiClient::from_env() {
        tracing::error!("LLM init failed: {e}");
        return;
    }

    // ── Proof policy engine ────────────────────────────────────────────────────
    let (engine, exec_ctx) = manifest::build_policy_engine();
    tracing::info!(
        "Proof policy engine loaded ({} action policies)",
        engine.manifest().actions.len()
    );

    let mut iteration               = 0u32;
    let mut consecutive_exhaustions = 0u32;
    let mut join_set: JoinSet<TaskResult> = JoinSet::new();
    // Maps experiment_id → queued-item-id for items that came from the protocol queue.
    let mut queued_experiment_map: HashMap<String, String> = HashMap::new();

    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }

        // ── Collect any finished tasks ─────────────────────────────────────────
        while let Some(outcome) = join_set.try_join_next() {
            let res = match outcome {
                Ok(r)  => r,
                Err(e) => {
                    tracing::error!("Experiment task panicked: {e}");
                    continue;
                }
            };
            // Update the protocol queue if this experiment served a queued item.
            if let Some(queue_id) = queued_experiment_map.remove(&res.experiment_id) {
                let mut q = protocol_queue.lock().unwrap();
                if res.converged {
                    q.mark_completed(&queue_id, "Execution converged — quantitative finding recorded".into());
                } else if res.exhausted_iterations {
                    q.mark_failed(&queue_id, "Max iterations reached without converging".into());
                } else {
                    q.mark_completed(&queue_id, "Execution cycle finished".into());
                }
            }
            scheduler.release(res.slot, &[]);
            tracing::debug!(slot = res.slot, converged = res.converged, "Slot freed");
            pause_after_run(
                &res,
                &sink,
                &loop_cfg,
                &mut consecutive_exhaustions,
                scheduler.slot_count,
            )
            .await;
        }

        // ── Fill available slots ───────────────────────────────────────────────
        while scheduler.available_slots() > 0 && running.load(Ordering::SeqCst) {
            // Priority 1: operator-queued protocols take precedence over everything.
            let queued_directive = {
                let q = protocol_queue.lock().unwrap();
                q.next_pending().map(|item| (item.id.clone(), item.statement.clone()))
            };

            // Priority 2: proposed journal directives (from the commissioning agenda).
            let journal_directive = if queued_directive.is_none() {
                let j = sink.journal.lock().unwrap();
                j.hypotheses.iter()
                    .find(|h| h.status == HypothesisStatus::Proposed)
                    .map(|h| (h.id.clone(), h.statement.clone()))
            } else {
                None
            };

            // Priority 3: seed the next commissioning procedure when nothing is queued.
            let active_directive = queued_directive.or(journal_directive);
            if active_directive.is_none() {
                // Nothing from queue or journal — check if we need to seed a new procedure.
                if should_idle_exploration(&sink.journal.lock().unwrap()) {
                    let seeded = seed_follow_up_hypothesis(&mut sink.journal.lock().unwrap());
                    if seeded {
                        sink.set_loop_status(
                            "commissioning",
                            "Commissioning procedure seeded — starting execution",
                            0,
                        );
                        continue;
                    }
                    tracing::info!(
                        pause_secs = loop_cfg.idle_pause_secs,
                        "Commissioning agenda complete — idling until new operator directives arrive"
                    );
                    sink.set_loop_status(
                        "idle",
                        "Commissioning complete — awaiting operator directives via /api/queue",
                        loop_cfg.idle_pause_secs,
                    );
                    sleep(Duration::from_secs(loop_cfg.idle_pause_secs)).await;
                    break;
                }
            }

            iteration += 1;
            iteration_counter.store(iteration, Ordering::SeqCst);

            // Instrument pool: in simulator mode no physical contention → no locks.
            let instruments: &[&str] = &[];
            let exp_id = format!("exp-{iteration}-{}", uuid::Uuid::new_v4());

            // If this experiment serves a queued item, record the mapping.
            if let Some((ref qid, _)) = active_directive {
                // Only queue items have UUIDs without the "hyp-" prefix used by journal hypotheses.
                let is_queue_item = !qid.starts_with("hyp-");
                if is_queue_item {
                    protocol_queue.lock().unwrap().mark_running(qid, &exp_id);
                    queued_experiment_map.insert(exp_id.clone(), qid.clone());
                    tracing::info!(queue_id = %qid, exp_id = %exp_id, "Executing operator-queued protocol");
                } else {
                    tracing::info!(directive = %qid, slot = 0, "Executing commissioning directive");
                }
            }

            let slot = match scheduler.try_acquire(&exp_id, instruments) {
                Some(s) => s,
                None    => break, // all slots busy (shouldn't happen here, but guard)
            };

            let (mandate, journal_summary, findings_at_start, calibration_status) = {
                let log     = sink.log.lock().unwrap();
                let journal = sink.journal.lock().unwrap();
                let m = mandate::build_mandate(iteration, &log, &journal, &policy, active_directive.as_ref());
                let s = journal.summary_for_llm();
                let f = journal.findings.len() as u32;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                let cal: std::collections::HashMap<String, (bool, bool)> = [
                    ("read_ph",          "ph_meter"),
                    ("read_absorbance",  "spectrophotometer"),
                    ("read_temperature", "incubator"),
                ].iter().map(|(tool, inst)| {
                    let rec = journal.last_calibration_for(inst);
                    let calibrated = rec.is_some();
                    let valid = rec.map(|r| r.is_valid_at(now)).unwrap_or(false);
                    (tool.to_string(), (calibrated, valid))
                }).collect();
                (m, s, f, cal)
            };

            let config = OrchestratorConfig {
                max_iterations: loop_cfg.max_iterations,
                code_gen_temperature: 0.2,
                reasoning_temperature: 0.7,
                capability_policy: Some(policy.clone()),
                revocation_list: RevocationList::default(),
                event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
                approval_queue: Some(Arc::clone(&approval_queue)),
                approval_timeout_secs: 300,
                journal_summary,
                findings_at_start,
                calibration_status,
                require_system_finding_for_completion: true,
                hypothesis_manager: Some(Arc::clone(&hypothesis_manager)),
                ..OrchestratorConfig::default()
            };

            let task = ExperimentTask {
                slot,
                experiment_id:      exp_id,
                mandate,
                iteration,
                config,
                sink:               Arc::clone(&sink),
                sila_clients:       sila_clients.clone(),
                engine:             engine.clone(),
                exec_ctx:           exec_ctx.clone(),
                db:                 Arc::clone(&db),
                approval_queue:     Arc::clone(&approval_queue),
                lab_state:          Arc::clone(&lab_state),
                hypothesis_manager: Arc::clone(&hypothesis_manager),
            };

            tracing::info!(
                slot,
                experiment_id = %task.experiment_id,
                "Spawning experiment in slot {slot}"
            );
            sink.set_loop_status(
                "running",
                format!(
                    "Experiment {iteration} — up to {} LLM iterations",
                    loop_cfg.max_iterations
                ),
                0,
            );
            join_set.spawn(run_one_experiment(task));

            // With slot_count == 1, break immediately — don't spin-fill.
            if scheduler.slot_count == 1 {
                break;
            }
        }

        // ── Wait strategy ──────────────────────────────────────────────────────
        if join_set.is_empty() {
            // No running tasks — brief pause before re-checking.
            sleep(Duration::from_millis(200)).await;
        } else {
            // Wait for any running task to complete, then loop back to fill slots.
            if let Some(outcome) = join_set.join_next().await {
                let res = match outcome {
                    Ok(r)  => r,
                    Err(e) => {
                        tracing::error!("Experiment task panicked: {e}");
                        continue;
                    }
                };
                scheduler.release(res.slot, &[]);
                tracing::debug!(slot = res.slot, "Slot freed after join_next");
                pause_after_run(
                    &res,
                    &sink,
                    &loop_cfg,
                    &mut consecutive_exhaustions,
                    scheduler.slot_count,
                )
                .await;
            }
        }
    }

    // Drain remaining tasks on shutdown.
    join_set.abort_all();
    tracing::info!("Loop stopped after {iteration} iterations");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{DiscoveryJournal, Finding, Hypothesis, HypothesisStatus};

    #[test]
    fn idle_when_findings_exist_but_no_active_hypotheses() {
        let mut j = DiscoveryJournal::default();
        j.findings.push(Finding {
            id: "f1".into(),
            statement: "s".into(),
            evidence: vec![],
            measurements: vec![],
            experiment_id: None,
            source: "system".into(),
            first_observed_secs: 0,
        });
        assert!(should_idle_exploration(&j));

        j.hypotheses.push(Hypothesis {
            id: "h1".into(),
            statement: "test".into(),
            status: HypothesisStatus::Proposed,
            created_secs: 0,
            updated_secs: 0,
        });
        assert!(!should_idle_exploration(&j));
    }

    #[test]
    fn backoff_scales_with_consecutive_exhaustions() {
        let cfg = LoopConfig {
            max_iterations: 10,
            inter_run_pause_secs: 120,
            idle_pause_secs: 300,
            backoff_base_secs: 60,
            backoff_max_secs: 300,
        };
        assert_eq!(cfg.backoff_secs(1), 60);
        assert_eq!(cfg.backoff_secs(2), 120);
        assert_eq!(cfg.backoff_secs(3), 240);
        assert_eq!(cfg.backoff_secs(4), 300);
        assert_eq!(cfg.backoff_secs(10), 300);
    }
}
