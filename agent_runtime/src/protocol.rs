//! Structured experimental protocol types.
//!
//! A [`Protocol`] is a named sequence of [`ProtocolStep`]s, each mapping to a
//! single tool call.  The LLM proposes a [`ProtocolPlan`] as JSON; the runtime
//! validates it into a typed [`Protocol`] before any step is executed.
//!
//! Every step passes through the full 5-stage orchestrator validation pipeline
//! (sandbox → approval → capability → proof policy → dispatch).  The LLM sees
//! the result of each step before the next is attempted, so it can adapt its
//! plan mid-run.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum number of steps allowed in a single protocol.
pub const MAX_PROTOCOL_STEPS: usize = 20;

// ── Core types ────────────────────────────────────────────────────────────────

/// One step in a protocol — maps 1:1 to a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolStep {
    /// Tool name (must be in the sandbox allowlist).
    pub tool: String,
    /// Tool parameters as a JSON object.
    pub params: serde_json::Value,
    /// Human-readable description of what this step does and why.
    pub description: String,
}

/// A structured experimental protocol produced by the runtime from a [`ProtocolPlan`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Protocol {
    /// Unique run identifier assigned at creation time.
    pub id: Uuid,
    /// Short name for this protocol (e.g. "Dilution Series A").
    pub name: String,
    /// The LLM's scientific hypothesis this protocol is designed to test.
    pub hypothesis: String,
    /// Ordered list of tool calls to execute.
    pub steps: Vec<ProtocolStep>,
    /// Unix timestamp (seconds) when this protocol was created.
    pub created_at_utc: i64,
    /// Number of times to run the full step sequence (≥ 1).
    pub replicate_count: u32,
    /// Optional canonical protocol template ID (e.g. "beer-lambert-scan-v1").
    /// `None` for fully custom, ad-hoc protocols.
    pub template_id: Option<String>,
}

/// The JSON shape the LLM emits when calling `propose_protocol`.
///
/// Validated and converted into a [`Protocol`] before execution begins.
/// This is the LLM boundary — only typed, validated data crosses it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolPlan {
    /// Short protocol name.
    pub name: String,
    /// Scientific hypothesis being tested.
    pub hypothesis: String,
    /// Ordered steps to execute (max [`MAX_PROTOCOL_STEPS`]).
    pub steps: Vec<ProtocolStep>,
    /// Number of times to run the full step sequence for replication (default 1, max 10).
    ///
    /// Use > 1 to get defensible statistics (mean ± SD across replicates).
    /// Each replicate re-runs all steps in order; include a vessel-reset step
    /// (aspirate back to 0) at the start when the vessel must start clean.
    #[serde(default = "default_replicate_count")]
    pub replicate_count: u32,
    /// Optional canonical protocol template ID for reproducibility tracking.
    #[serde(default)]
    pub template_id: Option<String>,
}

fn default_replicate_count() -> u32 { 1 }

impl ProtocolPlan {
    /// Validate the plan. Returns `Err` with a human-readable reason on failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("protocol name must be non-empty".into());
        }
        if self.hypothesis.is_empty() {
            return Err("protocol hypothesis must be non-empty".into());
        }
        if self.steps.is_empty() {
            return Err("protocol must have at least one step".into());
        }
        if self.steps.len() > MAX_PROTOCOL_STEPS {
            return Err(format!(
                "protocol has {} steps; maximum is {}",
                self.steps.len(),
                MAX_PROTOCOL_STEPS
            ));
        }
        if self.replicate_count < 1 || self.replicate_count > 10 {
            return Err(format!(
                "replicate_count must be between 1 and 10, got {}",
                self.replicate_count
            ));
        }
        for (i, step) in self.steps.iter().enumerate() {
            if step.tool.is_empty() {
                return Err(format!("step {i}: tool name must be non-empty"));
            }
            if !step.tool.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err(format!(
                    "step {i}: tool name '{}' contains invalid characters (allowed: [a-zA-Z0-9_])",
                    step.tool
                ));
            }
            if !step.params.is_object() {
                return Err(format!("step {i}: params must be a JSON object"));
            }
        }
        Ok(())
    }

    /// Convert into a [`Protocol`], assigning a new UUID and current timestamp.
    ///
    /// Call [`validate`] first — this method does not re-validate.
    pub fn into_protocol(self) -> Protocol {
        Protocol {
            id: Uuid::new_v4(),
            name: self.name,
            hypothesis: self.hypothesis,
            steps: self.steps,
            created_at_utc: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            replicate_count: self.replicate_count,
            template_id: self.template_id,
        }
    }
}

// ── Execution results ─────────────────────────────────────────────────────────

/// The outcome of a single step in a protocol run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepOutcome {
    pub step_index: usize,
    pub replicate_index: usize,
    pub tool: String,
    pub description: String,
    /// `true` if the 5-stage pipeline allowed the action and the tool succeeded.
    pub allowed: bool,
    /// Tool output, present when allowed.
    pub result: Option<serde_json::Value>,
    /// Human-readable rejection reason, present when not allowed.
    pub rejection_reason: Option<String>,
}

/// Per-replicate and aggregate statistics for multi-replicate protocol runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicateAggregate {
    /// Steps succeeded per replicate (same order as replicate index).
    pub steps_succeeded_counts: Vec<usize>,
    pub mean_steps_succeeded: f64,
    pub sd_steps_succeeded: f64,
}

impl ReplicateAggregate {
    pub fn from_counts(counts: &[usize]) -> Self {
        let n = counts.len() as f64;
        let mean = counts.iter().sum::<usize>() as f64 / n.max(1.0);
        let variance = counts.iter()
            .map(|&c| (c as f64 - mean).powi(2))
            .sum::<f64>()
            / n.max(1.0);
        Self {
            steps_succeeded_counts: counts.to_vec(),
            mean_steps_succeeded: mean,
            sd_steps_succeeded: variance.sqrt(),
        }
    }
}

/// Status of the ZK audit proof generation and Base L2 submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ZkProofStatus {
    /// Proof task spawned — result not yet available (async background task).
    Pending,
    /// Proof generated and submitted to Base; `tx_hash` links to basescan.org.
    Verified { tx_hash: String },
    /// Proof generation or submission failed.
    Failed { reason: String },
    /// ZK proving is disabled (`AXIOMLAB_BASE_RPC_URL` not set or
    /// crate built without `prove`/`onchain` features).
    Disabled,
}

/// Whether the protocol conclusion was successfully anchored to the Sigstore
/// Rekor transparency log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RekorStatus {
    /// Conclusion hash anchored; `uuid` links to the Rekor log entry.
    Anchored { uuid: String },
    /// All Rekor attempts failed.  Local audit chain is still intact.
    Failed { reason: String },
    /// No audit signer configured — Rekor submission skipped.
    Skipped,
}

/// One component in a GUM-compliant uncertainty budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncertaintyComponent {
    /// Human-readable source label (e.g., "pH probe repeatability", "calibration").
    pub source: String,
    /// Standard uncertainty u_i for this component.
    pub u_i: f64,
    /// Sensitivity coefficient c_i (partial derivative of output w.r.t. input).
    pub sensitivity_coeff: f64,
    /// Contribution to combined variance: `(c_i * u_i)^2`.
    pub contribution: f64,
}

/// GUM-compliant combined uncertainty budget for a measured parameter.
///
/// Built at protocol conclusion from all sensor-reading step outcomes.
/// Reported as expanded uncertainty U = k × u_combined at 95% confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncertaintyBudget {
    /// Name of the measured parameter (e.g., "pH", "absorbance_600nm").
    pub parameter: String,
    /// Physical unit.
    pub unit: String,
    /// Type A standard uncertainty (repeatability, from statistics).
    pub u_type_a: f64,
    /// Type B standard uncertainty (systematic, from calibration/spec).
    pub u_type_b: f64,
    /// Combined standard uncertainty: sqrt(u_a² + u_b²).
    pub u_combined: f64,
    /// Effective degrees of freedom (Welch-Satterthwaite).
    pub effective_dof: f64,
    /// Coverage factor k from t-distribution at `confidence_level`.
    pub coverage_factor_k: f64,
    /// Expanded uncertainty U = k × u_combined.
    pub expanded_u: f64,
    /// Confidence level (0.95 for 95%).
    pub confidence_level: f64,
    /// Per-source breakdown.
    pub budget_entries: Vec<UncertaintyComponent>,
}

impl UncertaintyBudget {
    /// Build a budget from a list of (source, u_i, sensitivity_coeff) tuples.
    ///
    /// `u_type_a_values` comes from repeated measurements; `u_type_b_abs` from specs.
    pub fn from_instrument_uncertainty(
        parameter: impl Into<String>,
        unit: impl Into<String>,
        reading: f64,
        u_type_a_fraction: f64,
        u_type_b_abs: f64,
    ) -> Self {
        let parameter = parameter.into();
        let unit = unit.into();

        let u_a = reading.abs() * u_type_a_fraction;
        let u_b = u_type_b_abs;
        let u_c = (u_a * u_a + u_b * u_b).sqrt();

        // Welch-Satterthwaite with large DoF assumption (> 30 → k ≈ 2.0 for 95%).
        let eff_dof = if u_c > 0.0 {
            let numerator = (u_a * u_a + u_b * u_b).powi(2);
            let denominator = (u_a.powi(4) / 30.0) + (u_b.powi(4) / 50.0);
            if denominator > 0.0 { numerator / denominator } else { 100.0 }
        } else {
            100.0
        };

        // Coverage factor from t-table at 95%: approximate via effective DoF.
        let k = if eff_dof >= 30.0 { 2.0 } else { t_95_coverage(eff_dof) };
        let expanded_u = k * u_c;

        UncertaintyBudget {
            parameter: parameter.clone(),
            unit: unit.clone(),
            u_type_a: u_a,
            u_type_b: u_b,
            u_combined: u_c,
            effective_dof: eff_dof,
            coverage_factor_k: k,
            expanded_u,
            confidence_level: 0.95,
            budget_entries: vec![
                UncertaintyComponent {
                    source: format!("{parameter} repeatability (Type A)"),
                    u_i: u_a,
                    sensitivity_coeff: 1.0,
                    contribution: u_a * u_a,
                },
                UncertaintyComponent {
                    source: format!("{parameter} systematic / calibration (Type B)"),
                    u_i: u_b,
                    sensitivity_coeff: 1.0,
                    contribution: u_b * u_b,
                },
            ],
        }
    }
}

/// Approximate t-distribution 95% coverage factor for small degrees of freedom.
/// Values from ISO GUM Table G.2.
fn t_95_coverage(dof: f64) -> f64 {
    // Piecewise linear approximation over the standard table.
    let table: &[(f64, f64)] = &[
        (1.0, 12.71), (2.0, 4.30), (3.0, 3.18), (4.0, 2.78),
        (5.0, 2.57),  (6.0, 2.45), (7.0, 2.36), (8.0, 2.31),
        (10.0, 2.23), (12.0, 2.18), (15.0, 2.13), (20.0, 2.09),
        (25.0, 2.06), (30.0, 2.04),
    ];
    if dof <= 1.0 { return 12.71; }
    if dof >= 30.0 { return 2.0; }
    for i in 0..table.len() - 1 {
        let (d0, k0) = table[i];
        let (d1, k1) = table[i + 1];
        if dof <= d1 {
            let t = (dof - d0) / (d1 - d0);
            return k0 + t * (k1 - k0);
        }
    }
    2.0
}

/// The complete result of running a protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRunResult {
    pub protocol_id: Uuid,
    pub run_id: Uuid,
    pub protocol_name: String,
    pub steps_total: usize,
    pub steps_succeeded: usize,
    /// The LLM's scientific conclusion after observing all step results.
    pub conclusion: String,
    pub step_results: Vec<StepOutcome>,
    /// Number of replicates executed.
    pub replicate_count: u32,
    /// Aggregate statistics across replicates; `None` for single-replicate runs.
    pub aggregate: Option<ReplicateAggregate>,
    /// Rekor transparency-log anchoring status for this conclusion.
    pub rekor_status: RekorStatus,
    /// ZK audit proof status; `Pending` until the background task completes.
    pub zk_proof_status: ZkProofStatus,
    /// Per-parameter uncertainty budgets built from all sensor readings in the run.
    /// One entry per unique measured parameter (e.g., "pH", "absorbance_600nm").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncertainty_budgets: Vec<UncertaintyBudget>,
}

// ── Protocol recovery types ───────────────────────────────────────────────────

/// Partial run state reconstructed from the audit log.
///
/// Used by [`scan_for_protocol_state`][crate::audit::scan_for_protocol_state]
/// to find the last confirmed step so that `resume_protocol` can continue from
/// the next step without re-executing already-dispatched actions.
#[derive(Debug, Clone)]
pub struct ProtocolRecoveryState {
    pub protocol_id: Uuid,
    pub run_id: Uuid,
    /// 0-based index of the last step that was allowed and dispatched.
    pub last_completed_step: usize,
    pub replicate_index: usize,
    /// Raw JSON output values from completed steps, in order.
    pub step_results: Vec<serde_json::Value>,
}

/// Result of scanning the audit log for a protocol's execution state.
#[derive(Debug)]
pub enum ProtocolScanResult {
    /// A `protocol_conclusion` entry exists — the run completed normally.
    Complete,
    /// Steps were started but no conclusion was written — can resume.
    Interrupted(ProtocolRecoveryState),
    /// The audit hash chain failed verification — unsafe to trust the log.
    ChainInvalid(String),
    /// No `protocol_step` entries were found for this protocol_id.
    NotFound,
}

// ── JSON schema for propose_protocol ─────────────────────────────────────────

/// Returns the JSON schema for the `propose_protocol` tool parameter.
///
/// Used by [`ToolSpec`] when registering `propose_protocol` in the orchestrator.
pub fn propose_protocol_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Short name for this protocol (e.g. 'Dilution Series A')"
            },
            "hypothesis": {
                "type": "string",
                "description": "The scientific hypothesis this protocol is designed to test"
            },
            "steps": {
                "type": "array",
                "maxItems": MAX_PROTOCOL_STEPS,
                "description": "Ordered list of tool calls to execute",
                "items": {
                    "type": "object",
                    "properties": {
                        "tool": {
                            "type": "string",
                            "description": "Tool name (must be in the allowed tool list)"
                        },
                        "params": {
                            "type": "object",
                            "description": "Tool parameters"
                        },
                        "description": {
                            "type": "string",
                            "description": "Why this step is being performed"
                        }
                    },
                    "required": ["tool", "params", "description"]
                }
            },
            "replicate_count": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10,
                "default": 1,
                "description": "Number of times to run the full step sequence. Use >1 for defensible statistics (mean ± SD). Include a vessel-reset step when needed."
            },
            "template_id": {
                "type": "string",
                "description": "Optional: reference a canonical protocol template by ID (e.g. 'beer-lambert-scan-v1') for reproducibility tracking. Leave null for fully custom protocols."
            }
        },
        "required": ["name", "hypothesis", "steps"]
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_plan() -> ProtocolPlan {
        ProtocolPlan {
            name: "Test Protocol".into(),
            hypothesis: "Dispense increases volume".into(),
            steps: vec![ProtocolStep {
                tool: "dispense".into(),
                params: serde_json::json!({"pump_id": "beaker_A", "volume_ul": 100.0}),
                description: "Dispense 100 µL into beaker A".into(),
            }],
            replicate_count: 1,
            template_id: None,
        }
    }

    #[test]
    fn valid_plan_validates_ok() {
        assert!(minimal_plan().validate().is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        let mut p = minimal_plan();
        p.name = String::new();
        assert!(p.validate().is_err());
    }

    #[test]
    fn empty_steps_rejected() {
        let mut p = minimal_plan();
        p.steps.clear();
        assert!(p.validate().is_err());
    }

    #[test]
    fn too_many_steps_rejected() {
        let mut p = minimal_plan();
        let step = p.steps[0].clone();
        p.steps = vec![step; MAX_PROTOCOL_STEPS + 1];
        assert!(p.validate().is_err());
    }

    #[test]
    fn invalid_tool_name_chars_rejected() {
        let mut p = minimal_plan();
        p.steps[0].tool = "rm -rf /".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn non_object_params_rejected() {
        let mut p = minimal_plan();
        p.steps[0].params = serde_json::json!([1, 2, 3]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn into_protocol_assigns_uuid_and_timestamp() {
        let plan = minimal_plan();
        let proto = plan.into_protocol();
        assert!(!proto.id.to_string().is_empty());
        assert!(proto.created_at_utc > 0);
        assert_eq!(proto.steps.len(), 1);
        assert_eq!(proto.replicate_count, 1);
        assert!(proto.template_id.is_none());
    }

    #[test]
    fn replicate_count_zero_rejected() {
        let mut p = minimal_plan();
        p.replicate_count = 0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn replicate_count_eleven_rejected() {
        let mut p = minimal_plan();
        p.replicate_count = 11;
        assert!(p.validate().is_err());
    }

    #[test]
    fn replicate_count_ten_accepted() {
        let mut p = minimal_plan();
        p.replicate_count = 10;
        assert!(p.validate().is_ok());
    }

    #[test]
    fn replicate_aggregate_uniform_counts() {
        let agg = ReplicateAggregate::from_counts(&[2, 2, 2]);
        assert!((agg.mean_steps_succeeded - 2.0).abs() < 1e-9);
        assert!(agg.sd_steps_succeeded.abs() < 1e-9);
    }

    #[test]
    fn replicate_aggregate_varied_counts() {
        let agg = ReplicateAggregate::from_counts(&[1, 3]);
        assert!((agg.mean_steps_succeeded - 2.0).abs() < 1e-9);
        assert!(agg.sd_steps_succeeded > 0.0);
    }

    #[test]
    fn template_id_round_trips() {
        let mut p = minimal_plan();
        p.template_id = Some("beer-lambert-scan-v1".into());
        let proto = p.into_protocol();
        assert_eq!(proto.template_id.as_deref(), Some("beer-lambert-scan-v1"));
    }
}
