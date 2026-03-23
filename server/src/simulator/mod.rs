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
    experiment::Experiment,
    hypothesis::HypothesisManager,
    lab_state::LabState,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
};
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};

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
    slot:      usize,
    converged: bool,
}

// ── Single-experiment runner ───────────────────────────────────────────────────

async fn run_one_experiment(task: ExperimentTask) -> TaskResult {
    let llm = match OpenAiClient::from_env() {
        Ok(c)  => c,
        Err(e) => {
            tracing::error!("LLM init failed for slot {}: {e}", task.slot);
            return TaskResult { slot: task.slot, converged: false };
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

    TaskResult { slot: task.slot, converged }
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

    tracing::info!(
        slot_count = scheduler.slot_count,
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

    let mut iteration    = 0u32;
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

            if res.converged {
                let (n_findings, n_hyp) = {
                    let j = sink.journal.lock().unwrap();
                    (j.findings.len(), j.hypotheses.len())
                };
                tracing::info!(
                    findings   = n_findings,
                    hypotheses = n_hyp,
                    "All hypotheses settled — exploration converged. Pausing 60 s."
                );
                sleep(Duration::from_secs(60)).await;
            }
        }

        // ── Fill available slots ───────────────────────────────────────────────
        while scheduler.available_slots() > 0 && running.load(Ordering::SeqCst) {
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
                max_iterations: 20,
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

                if res.converged {
                    let (n_findings, n_hyp) = {
                        let j = sink.journal.lock().unwrap();
                        (j.findings.len(), j.hypotheses.len())
                    };
                    tracing::info!(
                        findings   = n_findings,
                        hypotheses = n_hyp,
                        "All hypotheses settled — converged. Pausing 60 s."
                    );
                    sleep(Duration::from_secs(60)).await;
                } else {
                    // Brief pause between sequential iterations (slot_count == 1 path).
                    if scheduler.slot_count == 1 {
                        sleep(Duration::from_secs(4)).await;
                    }
                }
            }
        }
    }

    // Drain remaining tasks on shutdown.
    join_set.abort_all();
    tracing::info!("Loop stopped after {iteration} iterations");
}
