mod manifest;
mod mandate;
mod tools;

use crate::discovery::HypothesisStatus;
use crate::ws_sink::WebSocketSink;
use agent_runtime::{
    capabilities::CapabilityPolicy,
    events::EventSink,
    experiment::Experiment,
    llm::OpenAiClient,
    orchestrator::{Orchestrator, OrchestratorConfig},
    revocation::RevocationList,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use tokio::time::{sleep, Duration};

pub async fn run_loop(
    sink: Arc<WebSocketSink>,
    running: Arc<AtomicBool>,
    iteration_counter: Arc<AtomicU32>,
) {
    let policy = CapabilityPolicy::default_lab();

    if let Err(e) = OpenAiClient::from_env() {
        tracing::error!("LLM init failed: {e}");
        return;
    }

    // ── SiLA 2 hardware connection ────────────────────────────────────────────
    let sila_endpoint = std::env::var("SILA2_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:50052".into());
    let sila_clients = match agent_runtime::hardware::SiLA2Clients::connect(&sila_endpoint).await {
        Ok(c) => {
            tracing::info!("SiLA 2 hardware connected at {sila_endpoint}");
            Some(Arc::new(c))
        }
        Err(e) => {
            tracing::warn!("SiLA 2 unavailable ({e}) — running with in-process physics simulator");
            None
        }
    };

    // ── Proof policy engine ───────────────────────────────────────────────────
    let (engine, exec_ctx) = manifest::build_policy_engine();
    tracing::info!(
        "Proof policy engine loaded ({} action policies)",
        engine.manifest().actions.len()
    );

    let mut iteration = 0u32;

    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }

        iteration += 1;
        iteration_counter.store(iteration, Ordering::SeqCst);

        let llm = match OpenAiClient::from_env() {
            Ok(c)  => c,
            Err(e) => { tracing::error!("LLM init failed: {e}"); break; }
        };

        // Pick the oldest proposed hypothesis to give the LLM a specific goal.
        // Falls back to free exploration when no proposed hypotheses exist.
        let active_hypothesis = {
            let j = sink.journal.lock().unwrap();
            j.hypotheses.iter()
                .find(|h| h.status == HypothesisStatus::Proposed)
                .map(|h| (h.id.clone(), h.statement.clone()))
        };

        if let Some((_, ref stmt)) = active_hypothesis {
            tracing::info!(hypothesis = %stmt, "Guided mode: testing active hypothesis");
        }

        let mandate = {
            let log     = sink.log.lock().unwrap();
            let journal = sink.journal.lock().unwrap();
            mandate::build_mandate(iteration, &log, &journal, &policy, active_hypothesis.as_ref())
        };

        let config = OrchestratorConfig {
            max_iterations: 20,
            code_gen_temperature: 0.2,
            reasoning_temperature: 0.7,
            capability_policy: Some(policy.clone()),
            revocation_list: RevocationList::default(),
            event_sink: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
            ..OrchestratorConfig::default()
        };

        let tools = match &sila_clients {
            Some(clients) => tools::make_sila2_tools(Arc::clone(clients), Arc::clone(&sink.journal)),
            None          => tools::make_sim_tools(Arc::clone(&sink.journal)),
        };

        let mut experiment = Experiment::new(
            format!("exp-{iteration}-{}", uuid::Uuid::new_v4()),
            &mandate,
        );

        let orchestrator = Orchestrator::new(llm, tools::make_sandbox(), tools, config)
            .with_runtime_policy(engine.clone(), exec_ctx.clone());

        if let Err(e) = orchestrator.run_experiment(&mut experiment).await {
            tracing::error!("experiment {iteration} error: {e}");
        }

        // Convergence: all hypotheses settled + at least one finding → slow the loop.
        let converged = {
            let j = sink.journal.lock().unwrap();
            !j.findings.is_empty()
                && !j.hypotheses.is_empty()
                && j.hypotheses.iter().all(|h| {
                    h.status == HypothesisStatus::Confirmed
                        || h.status == HypothesisStatus::Rejected
                })
        };

        if converged {
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
        } else {
            sleep(Duration::from_secs(4)).await;
        }
    }

    tracing::info!("loop stopped after {iteration} iterations");
}
