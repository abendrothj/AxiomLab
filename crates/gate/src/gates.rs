//! The seven concrete gates, in pipeline order.
//!
//! Each returns `Result<(), Rejection>`; the first `Err` hard-stops the action.
//! No gate logs-and-continues, softens, or skips.

use crate::Gate;
use crate::context::GateContext;
use async_trait::async_trait;
use axiom_audit::EntryData;
use axiom_chemistry::global as chemistry;
use axiom_proofs::{PredicateOutcome, evaluate_predicate};
use axiom_types::{Action, Rejection};
use serde_json::json;

fn reject(gate: &'static str, reason: impl Into<String>, action: &Action) -> Rejection {
    Rejection::new(gate, reason, action.clone())
}

// ── 1. CapabilityGate ──────────────────────────────────────────────────────

/// Verus-derived hardware bounds, per parameter. No LLM retry: reject and report.
pub struct CapabilityGate;

#[async_trait]
impl Gate for CapabilityGate {
    fn name(&self) -> &'static str {
        "CapabilityGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        ctx.capability
            .validate(&action.tool, &action.params)
            .map_err(|e| reject(self.name(), e, action))
    }
}

// ── 2. ChemistryGate ───────────────────────────────────────────────────────

/// Checks the proposed reagent against current vessel contents.
pub struct ChemistryGate;

#[async_trait]
impl Gate for ChemistryGate {
    fn name(&self) -> &'static str {
        "ChemistryGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        // Only adding a reagent to a vessel can create an incompatibility.
        if action.tool != "dispense" {
            return Ok(());
        }
        let params = &action.params;
        let Some(adding) = params
            .get("reagent")
            .or_else(|| params.get("source_reagent"))
            .and_then(|v| v.as_str())
        else {
            return Ok(()); // no reagent named — nothing to check
        };
        let Some(vessel) = params
            .get("vessel_id")
            .or_else(|| params.get("target_container"))
            .and_then(|v| v.as_str())
        else {
            return Ok(());
        };

        let (adding_name, existing) = {
            let lab = ctx.lab_state.lock().unwrap();
            // Resolve the added reagent's display name if it is a known id.
            let adding_name = lab
                .reagents
                .get(adding)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| adding.to_string());
            (adding_name, lab.vessel_reagent_names(vessel))
        };

        match chemistry().check_addition(&existing, &adding_name) {
            axiom_chemistry::HazardLevel::Safe => Ok(()),
            axiom_chemistry::HazardLevel::Dangerous(reason) => Err(reject(self.name(), reason, action)),
        }
    }
}

// ── 3. CalibrationGate ─────────────────────────────────────────────────────

/// Blocks measurement tools unless a non-expired calibration exists.
pub struct CalibrationGate;

#[async_trait]
impl Gate for CalibrationGate {
    fn name(&self) -> &'static str {
        "CalibrationGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        let Some(instrument) = crate::calibration::measurement_instrument(&action.tool) else {
            return Ok(()); // not a measurement tool
        };
        let valid_until = crate::calibration::latest_valid_until(&ctx.audit_chain, instrument)
            .map_err(|e| reject(self.name(), format!("audit chain read failed: {e}"), action))?;
        let now = now_secs();
        match valid_until {
            Some(vu) if vu > now => Ok(()),
            Some(_) => Err(reject(
                self.name(),
                format!("calibration for '{instrument}' has expired"),
                action,
            )),
            None => Err(reject(
                self.name(),
                format!("no valid calibration record for '{instrument}'"),
                action,
            )),
        }
    }
}

// ── 4. ProofGate ───────────────────────────────────────────────────────────

/// Two checks, both required: required artifacts present (and verified at load),
/// and the runtime predicate passes with the actual proposed parameters.
pub struct ProofGate;

#[async_trait]
impl Gate for ProofGate {
    fn name(&self) -> &'static str {
        "ProofGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        ctx.proofs
            .check_artifact(&action.tool)
            .map_err(|e| reject(self.name(), e, action))?;
        match evaluate_predicate(action) {
            PredicateOutcome::Pass | PredicateOutcome::NotApplicable => {}
            PredicateOutcome::Fail(reason) => return Err(reject(self.name(), reason, action)),
        }

        // Stateful, verified check: a dispense must keep the vessel's running
        // total within its capacity. Uses safe_add_volume (Verus-proven twin).
        if action.tool == "dispense" {
            let p = &action.params;
            let vessel = p
                .get("vessel_id")
                .or_else(|| p.get("target_container"))
                .and_then(|v| v.as_str());
            let add = p.get("volume_ul").and_then(|v| v.as_f64());
            if let (Some(vessel), Some(add)) = (vessel, add) {
                let lab = ctx.lab_state.lock().unwrap();
                if let Some(capacity) = lab.vessel_capacity(vessel) {
                    let current = lab.vessel_volume(vessel);
                    // µL is the verified envelope's unit; round to it.
                    let ok = axiom_proofs::predicates::safe_add_volume(
                        current.round() as u64,
                        add.round() as u64,
                        capacity.round() as u64,
                    )
                    .is_some();
                    if !ok {
                        return Err(reject(
                            self.name(),
                            format!(
                                "dispense of {add} µL would exceed verified capacity of '{vessel}' \
                                 ({current} + {add} > {capacity} µL)"
                            ),
                            action,
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

// ── 5. ApprovalGate ────────────────────────────────────────────────────────

/// Operator decision for Actuation/Destructive actions, scoped to action+params.
pub struct ApprovalGate;

#[async_trait]
impl Gate for ApprovalGate {
    fn name(&self) -> &'static str {
        "ApprovalGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        if !action.risk_class.requires_approval() {
            return Ok(());
        }
        crate::require_operator_approval(ctx, &action.tool, &action.params)
            .await
            .map(|_| ())
            .map_err(|e| reject(self.name(), e, action))
    }
}

// ── 6. ExecuteGate ─────────────────────────────────────────────────────────

/// Dispatches to the instrument backend; records the result for the AuditGate
/// and reflects liquid handling into LabState so chemistry stays accurate.
pub struct ExecuteGate;

#[async_trait]
impl Gate for ExecuteGate {
    fn name(&self) -> &'static str {
        "ExecuteGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        let result = ctx
            .clients
            .execute(action)
            .await
            .map_err(|e| reject(self.name(), format!("instrument error: {e}"), action))?;

        // Reflect liquid handling into LabState so the ChemistryGate stays
        // accurate and the ProofGate's cumulative-capacity check sees the running
        // total. Volume is tracked even when no reagent is named.
        if action.tool == "dispense" {
            if let (Some(vessel), Some(vol)) = (
                action.params.get("vessel_id").or_else(|| action.params.get("target_container")).and_then(|v| v.as_str()),
                action.params.get("volume_ul").and_then(|v| v.as_f64()),
            ) {
                let reagent = action
                    .params
                    .get("reagent")
                    .or_else(|| action.params.get("source_reagent"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unspecified)");
                let mut lab = ctx.lab_state.lock().unwrap();
                lab.add_to_vessel(vessel, reagent, vol);
            }
        }

        let snapshot = ctx.clients.vessel_snapshot().await;
        ctx.set_exec_result(result, snapshot);
        Ok(())
    }
}

// ── 7. AuditGate ───────────────────────────────────────────────────────────

/// Appends a signed entry recording the executed action and its result. Runs
/// after execution; a failure here never retroactively un-does the action.
pub struct AuditGate;

#[async_trait]
impl Gate for AuditGate {
    fn name(&self) -> &'static str {
        "AuditGate"
    }
    async fn check(&self, action: &Action, ctx: &GateContext) -> Result<(), Rejection> {
        let (result, snapshot) = ctx.take_exec();
        let reason = json!({
            "tool": action.tool,
            "params": action.params,
            "result": result,
            "vessel_snapshot": snapshot,
            "experiment_id": ctx.experiment_id,
            "iteration": ctx.iteration,
        })
        .to_string();
        let entry = EntryData::new(action.tool.clone(), "allow", reason, true);
        ctx.audit_chain
            .append(entry, ctx.signer.as_ref())
            .map_err(|e| reject(self.name(), format!("audit append failed: {e}"), action))?;
        Ok(())
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Record a rejection in the audit chain (called by the pipeline on any `Err`).
pub(crate) fn audit_rejection(ctx: &GateContext, rej: &Rejection) {
    let reason = json!({
        "gate": rej.gate,
        "reason": rej.reason,
        "tool": rej.action.tool,
        "params": rej.action.params,
        "experiment_id": ctx.experiment_id,
        "iteration": ctx.iteration,
    })
    .to_string();
    let entry = EntryData::new(rej.action.tool.clone(), "deny", reason, false);
    if let Err(e) = ctx.audit_chain.append(entry, ctx.signer.as_ref()) {
        tracing::error!(error = %e, "failed to audit a rejection");
    }
}
