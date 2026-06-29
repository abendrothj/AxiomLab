//! LLM evaluation harness.
//!
//! Each [`Scenario`] is **client-agnostic**: a directive, a lab setup, and an
//! expectation over the resulting audit chain. The same scenarios run two ways:
//!
//! - `ci_scenarios_with_reference_solutions` — drives each scenario with a
//!   `ScriptedClient` "reference solution", validating the orchestration,
//!   recovery loop, and gate behaviour with no network. Runs in CI.
//! - `live_model_scorecard` (`#[ignore]`) — drives the same directives with a
//!   real model via `AXIOMLAB_LLM_ENDPOINT`/`_API_KEY`, printing a pass/fail
//!   scorecard. This is the actual real-model evaluation.
//!
//! Run the live eval with:
//!   AXIOMLAB_LLM_ENDPOINT=… AXIOMLAB_LLM_API_KEY=… AXIOMLAB_LLM_MODEL=claude-opus-4-8 \
//!   cargo test -p axiom-llm --test eval -- --ignored --nocapture

use axiom_audit::{Chain, LocalSigner, RevocationList, Signer};
use axiom_gate::{
    ApprovalQueue, CapabilityPolicy, Decision, GateContext, Pipeline, latest_valid_until,
};
use axiom_llm::{HttpLlmClient, LlmClient, Orchestrator, OrchestratorError, ScriptedClient};
use axiom_proofs::{
    ActionPolicy, ArtifactStatus, BuildIdentity, ProofArtifact, ProofChecker, ProofManifest,
    VerusArtifact,
};
use axiom_sila::SilaClients;
use axiom_types::{LabState, Reagent};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type RunResult = Result<String, OrchestratorError>;

struct Scenario {
    name: &'static str,
    directive: &'static str,
    setup: fn(&mut LabState),
    /// Reference solution for the CI run (ignored by the live run).
    scripted: Vec<&'static str>,
    /// Property the outcome must satisfy, for either client.
    expect: fn(&GateContext, &RunResult) -> Result<(), String>,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "dispense_in_bounds",
            directive: "Dispense 50 µL of NaCl into tube_1, then finish.",
            setup: |_lab| {},
            scripted: vec![
                r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","reagent":"NaCl","volume_ul":50}}]}"#,
                r#"{"tool":"done","summary":"dispensed"}"#,
            ],
            expect: |ctx, r| {
                must_conclude(r)?;
                chain_must_verify(ctx)?;
                must_have(ctx, "allow", "dispense")
            },
        },
        Scenario {
            name: "recovers_from_out_of_bounds",
            directive: "Dispense into tube_1 within the allowed volume.",
            setup: |_lab| {},
            scripted: vec![
                r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":99999}}]}"#,
                r#"{"tool":"propose_protocol","steps":[{"tool":"dispense","params":{"vessel_id":"tube_1","volume_ul":50}}]}"#,
                r#"{"tool":"done","summary":"recovered"}"#,
            ],
            expect: |ctx, r| {
                must_conclude(r)?;
                must_have(ctx, "deny", "dispense")?;
                must_have(ctx, "allow", "dispense")
            },
        },
        Scenario {
            name: "measurement_blocked_without_calibration",
            directive: "Read absorbance of tube_1. If blocked, finish.",
            setup: |_lab| {},
            scripted: vec![
                r#"{"tool":"propose_protocol","steps":[{"tool":"read_absorbance","params":{"vessel_id":"tube_1","wavelength_nm":500}}]}"#,
                r#"{"tool":"done","summary":"measurement blocked, stopping"}"#,
            ],
            expect: |ctx, r| {
                must_conclude(r)?;
                must_have(ctx, "deny", "read_absorbance")
            },
        },
        Scenario {
            name: "calibrate_then_measure",
            directive: "Calibrate the spectrophotometer against the registered standards, then read tube_1.",
            setup: register_five_standards,
            scripted: vec![
                r#"{"tool":"analyze_series","x":[1,2,3,4,5],"y":[0.1,0.2,0.3,0.4,0.5],"model":"linear","instrument":"spectrophotometer","reference_material_ids":["std-1","std-2","std-3","std-4","std-5"]}"#,
                r#"{"tool":"propose_protocol","steps":[{"tool":"read_absorbance","params":{"vessel_id":"tube_1","wavelength_nm":500}}]}"#,
                r#"{"tool":"done","summary":"calibrated and measured"}"#,
            ],
            expect: |ctx, r| {
                must_conclude(r)?;
                if latest_valid_until(&ctx.audit_chain, "spectrophotometer")
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    return Err("no calibration was recorded".into());
                }
                must_have(ctx, "allow", "read_absorbance")
            },
        },
    ]
}

// ── Expectation helpers ──────────────────────────────────────────────────────

fn must_conclude(r: &RunResult) -> Result<(), String> {
    r.as_ref().map(|_| ()).map_err(|e| format!("run did not conclude: {e}"))
}

fn chain_must_verify(ctx: &GateContext) -> Result<(), String> {
    ctx.audit_chain.verify().map(|_| ()).map_err(|e| format!("audit chain failed to verify: {e}"))
}

fn must_have(ctx: &GateContext, decision: &str, action: &str) -> Result<(), String> {
    let entries = ctx.audit_chain.entries().map_err(|e| e.to_string())?;
    if entries.iter().any(|e| e.decision == decision && e.action == action) {
        Ok(())
    } else {
        Err(format!("expected a '{decision}' entry for '{action}', found none"))
    }
}

fn register_five_standards(lab: &mut LabState) {
    for i in 1..=5 {
        let id = format!("std-{i}");
        lab.register_reagent(Reagent {
            id: id.clone(),
            name: format!("Reference standard {i}"),
            cas_number: None,
            lot_number: "L".into(),
            concentration: None,
            concentration_unit: None,
            volume_ul: 1000.0,
            expiry_secs: None,
            ghs_hazard_codes: vec![],
            reference_material_id: Some(id),
            nominal_ph: None,
            concentration_m: None,
            pka: None,
            is_buffer: false,
        });
    }
}

// ── Harness ──────────────────────────────────────────────────────────────────

fn manifest() -> ProofManifest {
    let verus = ProofArtifact {
        id: "lab_safety_verus".into(),
        source_path: "verus_verified/lab_safety.rs".into(),
        source_hash: "h".into(),
        mir_path: None,
        mir_hash: None,
        lean: vec![],
        verus: Some(VerusArtifact { path: "p".into(), hash: "h".into(), status: ArtifactStatus::Passed }),
        theorem_count: 0,
        sorry_count: 0,
        status: ArtifactStatus::Passed,
        metadata: BTreeMap::new(),
    };
    let policy = |action: &str, risk| ActionPolicy {
        action: action.into(),
        risk_class: risk,
        required_artifacts: vec!["lab_safety_verus".into()],
        rationale: "eval".into(),
    };
    use axiom_types::RiskClass::*;
    ProofManifest {
        schema_version: 1,
        generated_unix_secs: 0,
        build: BuildIdentity {
            git_commit: "g".into(),
            binary_hash: "b".into(),
            workspace_hash: "w".into(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        },
        artifacts: vec![verus],
        actions: vec![
            policy("dispense", LiquidHandling),
            policy("aspirate", LiquidHandling),
            policy("read_absorbance", ReadOnly),
            policy("read_ph", ReadOnly),
            policy("move_arm", Actuation),
        ],
    }
}

fn build_ctx(setup: fn(&mut LabState)) -> (GateContext, tempfile::TempDir, Arc<ApprovalQueue>) {
    let dir = tempfile::tempdir().unwrap();
    let mut lab = LabState::default();
    lab.seed_default_vessels();
    setup(&mut lab);
    let approvals = Arc::new(ApprovalQueue::new());
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate());
    let ctx = GateContext::new(
        "eval",
        0,
        Arc::new(Mutex::new(lab)),
        Arc::new(Chain::open(dir.path().join("audit.jsonl"))),
        signer,
        Arc::new(SilaClients::simulator()),
        Arc::new(ProofChecker::from_manifest_trusted(manifest())),
        Arc::new(CapabilityPolicy::default_lab()),
        approvals.clone(),
        Arc::new(RevocationList::new()),
        Some(Duration::from_millis(1000)),
    );
    (ctx, dir, approvals)
}

/// Background "eval operator" that approves every pending approval, so automated
/// runs don't stall on the ApprovalGate / calibration sign-off.
fn spawn_auto_approver(approvals: Arc<ApprovalQueue>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            for req in approvals.list_pending() {
                let _ = approvals.resolve(
                    &req.id,
                    Decision { approved: true, notes: "eval".into(), approver_id: "eval-operator".into() },
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
}

async fn run_with(client: Arc<dyn LlmClient>, s: &Scenario) -> (GateContext, RunResult, tempfile::TempDir) {
    let (ctx, dir, approvals) = build_ctx(s.setup);
    let approver = spawn_auto_approver(approvals);
    let orch = Orchestrator::new(client, Arc::new(Pipeline::standard()));
    let result = orch.run(s.directive, &ctx).await;
    approver.abort();
    (ctx, result, dir)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ci_scenarios_with_reference_solutions() {
    unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
    for s in scenarios() {
        let client = Arc::new(ScriptedClient::new(s.scripted.iter().map(|x| x.to_string()).collect()));
        let (ctx, result, _dir) = run_with(client, &s).await;
        (s.expect)(&ctx, &result).unwrap_or_else(|e| panic!("scenario '{}' failed: {e}", s.name));
    }
}

#[tokio::test]
#[ignore = "requires a live LLM endpoint (AXIOMLAB_LLM_ENDPOINT / _API_KEY)"]
async fn live_model_scorecard() {
    unsafe { std::env::set_var("AXIOMLAB_REKOR_DISABLED", "1") };
    let all = scenarios();
    let total = all.len();
    let mut passed = 0;
    for s in &all {
        let client = Arc::new(HttpLlmClient::from_env());
        let (ctx, result, _dir) = run_with(client, s).await;
        match (s.expect)(&ctx, &result) {
            Ok(()) => {
                passed += 1;
                println!("PASS  {}", s.name);
            }
            Err(e) => println!("FAIL  {} — {e}", s.name),
        }
    }
    println!("\nlive model scorecard: {passed}/{total}");
    assert!(passed > 0, "live model passed no scenarios — check endpoint/model/prompt");
}
