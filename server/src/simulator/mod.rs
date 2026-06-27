mod manifest;
mod mandate;
pub mod protocol_library;
mod tools;

use crate::discovery::HypothesisStatus;
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
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};

// ── Loop pacing ───────────────────────────────────────────────────────────────

/// Tunable exploration-loop pacing (all overridable via env).
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
        tracing::info!("All hypotheses settled — exploration converged. Pausing 60 s.");
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
        sleep(Duration::from_secs(pause)).await;
        return;
    }

    if should_idle_exploration(&sink.journal.lock().unwrap()) {
        *consecutive_exhaustions = 0;
        let pause = loop_cfg.idle_pause_secs;
        tracing::info!(
            pause_secs = pause,
            "Journal has findings and no active hypotheses — extended idle"
        );
        sleep(Duration::from_secs(pause)).await;
        return;
    }

    *consecutive_exhaustions = 0;
    sleep(Duration::from_secs(loop_cfg.inter_run_pause_secs)).await;
}

// ── Exploration loop ───────────────────────────────────────────────────────────

pub async fn run_loop(
    sink:               Arc<WebSocketSink>,
    running:            Arc<AtomicBool>,
    iteration_counter:  Arc<AtomicU32>,
    approval_queue:     Arc<PendingApprovalQueue>,
    db:                 Arc<crate::db::Db>,
    sila_clients:       Option<Arc<agent_runtime::hardware::SiLA2Clients>>,
    lab_state:          Arc<Mutex<LabState>>,
    hypothesis_manager: Arc<Mutex<HypothesisManager>>,
) {
    let policy     = CapabilityPolicy::default_lab();
    let scheduler  = SlotManager::from_env();
    let loop_cfg   = LoopConfig::from_env();

    tracing::info!(
        slot_count = scheduler.slot_count,
        max_iterations = loop_cfg.max_iterations,
        inter_run_pause_secs = loop_cfg.inter_run_pause_secs,
        idle_pause_secs = loop_cfg.idle_pause_secs,
        "Exploration loop starting ({} concurrent experiment slot(s))",
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
            if should_idle_exploration(&sink.journal.lock().unwrap()) {
                tracing::info!(
                    pause_secs = loop_cfg.idle_pause_secs,
                    "Skipping new experiment — findings recorded, no active hypotheses"
                );
                sleep(Duration::from_secs(loop_cfg.idle_pause_secs)).await;
                break;
            }

            iteration += 1;
            iteration_counter.store(iteration, Ordering::SeqCst);

            // Instrument pool: in simulator mode no physical contention → no locks.
            let instruments: &[&str] = &[];
            let exp_id = format!("exp-{iteration}-{}", uuid::Uuid::new_v4());

            let slot = match scheduler.try_acquire(&exp_id, instruments) {
                Some(s) => s,
                None    => break, // all slots busy (shouldn't happen here, but guard)
            };

            // Pick the oldest proposed hypothesis for guided mode.
            let active_hypothesis = {
                let j = sink.journal.lock().unwrap();
                j.hypotheses.iter()
                    .find(|h| h.status == HypothesisStatus::Proposed)
                    .map(|h| (h.id.clone(), h.statement.clone()))
            };

            if let Some((_, ref stmt)) = active_hypothesis {
                tracing::info!(hypothesis = %stmt, slot, "Slot {slot}: guided mode");
            }

            let (mandate, journal_summary, findings_at_start, calibration_status) = {
                let log     = sink.log.lock().unwrap();
                let journal = sink.journal.lock().unwrap();
                let m = mandate::build_mandate(iteration, &log, &journal, &policy, active_hypothesis.as_ref());
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
