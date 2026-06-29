//! Background worker: drains the protocol queue, running each directive through
//! the orchestrator + gate pipeline.

use crate::queue::QueueStatus;
use crate::state::AppState;
use axiom_gate::{GateContext, Pipeline};
use axiom_llm::{HttpLlmClient, Orchestrator};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub async fn run(state: AppState) {
    let pipeline = Arc::new(Pipeline::standard());
    tracing::info!(gates = ?pipeline.gate_names(), "Worker started");

    loop {
        let Some((id, directive)) = state.protocol_queue.claim_next() else {
            state.protocol_queue.wait().await;
            continue;
        };

        let iteration = state.iteration.fetch_add(1, Ordering::Relaxed);
        state.running.store(true, Ordering::Relaxed);
        state.broadcast(json!({ "event": "run_started", "id": id, "directive": directive }));

        let ctx = GateContext::new(
            id.clone(),
            iteration,
            state.lab_state.clone(),
            state.audit_chain.clone(),
            state.signer.clone(),
            state.clients.clone(),
            state.proofs.clone(),
            state.capability.clone(),
            state.approval_queue.clone(),
            state.revocations.clone(),
            None,
        );

        let llm = Arc::new(HttpLlmClient::from_env());
        let orchestrator = Orchestrator::new(llm, pipeline.clone());

        match orchestrator.run(&directive, &ctx).await {
            Ok(summary) => {
                state.protocol_queue.finish(&id, QueueStatus::Completed, Some(summary.clone()));
                state.broadcast(json!({ "event": "run_completed", "id": id, "summary": summary }));
            }
            Err(e) => {
                let msg = e.to_string();
                state.protocol_queue.finish(&id, QueueStatus::Failed, Some(msg.clone()));
                state.broadcast(json!({ "event": "run_failed", "id": id, "error": msg }));
            }
        }

        // Persist lab state after each run.
        if let Err(e) = state.lab_state.lock().unwrap().save() {
            tracing::warn!(error = %e, "failed to persist lab state");
        }
        state.running.store(false, Ordering::Relaxed);
    }
}
