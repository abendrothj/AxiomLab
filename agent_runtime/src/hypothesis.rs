//! Hypothesis state machine for the AxiomLab discovery journal.
//!
//! # Lifecycle
//!
//! ```text
//! Proposed ──► Active ──► Supported
//!                    └──► Refuted
//!                    └──► Revised { prior: _ } ──► (any of the above)
//! ```
//!
//! `HypothesisManager` persists every hypothesis in memory and exposes the
//! full set to the orchestrator's system-prompt so the LLM can reason about
//! accumulated evidence across experiments.
//!
//! # Evidence policy
//!
//! Auto-transitions fire when accumulated evidence crosses the configured
//! thresholds:
//!
//! - `confidence ≥ support_threshold` after ≥ `min_evidence` pieces
//!   → `Supported`
//! - `confidence ≤ 1 − refute_threshold` after ≥ `min_evidence` pieces
//!   → `Refuted`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

// ── Status ────────────────────────────────────────────────────────────────────

/// Current disposition of a hypothesis.
///
/// `Revised` wraps the status that was superseded, providing a complete audit
/// trail without a separate revisions table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum HypothesisStatus {
    Proposed,
    Active,
    Supported,
    Refuted,
    Revised {
        #[serde(rename = "prior")]
        prior: Box<HypothesisStatus>,
    },
}

impl HypothesisStatus {
    /// True when no further evidence can change the status.
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Supported | Self::Refuted)
    }

    /// One-word label for display / prompt injection.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Proposed       => "proposed",
            Self::Active         => "active",
            Self::Supported      => "supported",
            Self::Refuted        => "refuted",
            Self::Revised { .. } => "revised",
        }
    }
}

// ── Evidence ──────────────────────────────────────────────────────────────────

/// A key statistic extracted from an ANOVA / regression result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyStatistic {
    pub name:      String,
    pub value:     f64,
    pub threshold: Option<f64>,
}

/// One piece of experimental evidence linked to a hypothesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id:             String,
    pub experiment_id:  String,
    pub run_id:         Option<String>,
    /// `true` = the result is consistent with the hypothesis.
    pub supports:       bool,
    /// Human-readable conclusion from the LLM or statistical analysis.
    pub summary:        String,
    pub key_statistic:  Option<KeyStatistic>,
    pub recorded_at_utc: i64,
}

// ── Policy ────────────────────────────────────────────────────────────────────

/// Rules governing auto-transitions.
#[derive(Debug, Clone)]
pub struct EvidencePolicy {
    /// Minimum number of evidence records before auto-transition fires.
    pub min_evidence:      usize,
    /// Fraction of supporting evidence needed to auto-transition to `Supported`.
    /// Default: 1.0 (all evidence must support).
    pub support_threshold: f64,
    /// Fraction of *refuting* evidence needed to auto-transition to `Refuted`.
    /// Default: 1.0 (all evidence must refute).
    pub refute_threshold:  f64,
}

impl Default for EvidencePolicy {
    fn default() -> Self {
        Self {
            min_evidence:      3,
            support_threshold: 1.0,
            refute_threshold:  1.0,
        }
    }
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum HypothesisError {
    #[error("hypothesis '{0}' not found")]
    NotFound(String),
    #[error("hypothesis '{0}' is already settled and cannot receive new evidence")]
    AlreadySettled(String),
}

// ── Hypothesis ────────────────────────────────────────────────────────────────

/// A single hypothesis with its full evidence trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id:             String,
    pub statement:      String,
    pub status:         HypothesisStatus,
    pub evidence:       Vec<Evidence>,
    /// Fraction of supporting evidence: `supporting / total`.
    /// Initialised to 0.5 (neutral) when no evidence exists.
    pub confidence:     f64,
    pub created_at_utc: i64,
    pub updated_at_utc: i64,
}

impl Hypothesis {
    fn new(id: String, statement: String, now: i64) -> Self {
        Self {
            id,
            statement,
            status:         HypothesisStatus::Proposed,
            evidence:       Vec::new(),
            confidence:     0.5,
            created_at_utc: now,
            updated_at_utc: now,
        }
    }

    /// Append evidence and recalculate confidence; fire auto-transitions if
    /// thresholds are crossed.
    pub fn add_evidence(
        &mut self,
        ev:     Evidence,
        policy: &EvidencePolicy,
        now:    i64,
    ) {
        // Transition Proposed → Active on first evidence.
        if self.status == HypothesisStatus::Proposed {
            self.status = HypothesisStatus::Active;
        }

        self.evidence.push(ev);
        self.updated_at_utc = now;

        // Recompute confidence.
        let total = self.evidence.len();
        let supporting = self.evidence.iter().filter(|e| e.supports).count();
        self.confidence = supporting as f64 / total as f64;

        // Auto-transitions (only when minimum evidence threshold met).
        if total >= policy.min_evidence && self.status == HypothesisStatus::Active {
            if self.confidence >= policy.support_threshold {
                self.status = HypothesisStatus::Supported;
            } else if self.confidence <= 1.0 - policy.refute_threshold {
                self.status = HypothesisStatus::Refuted;
            }
        }
    }

    /// Mark the hypothesis as revised, preserving the prior status.
    pub fn revise(&mut self, now: i64) {
        let prior = std::mem::replace(&mut self.status, HypothesisStatus::Proposed);
        self.status = HypothesisStatus::Revised { prior: Box::new(prior) };
        self.updated_at_utc = now;
    }

    /// True when the hypothesis has reached a terminal status.
    pub fn is_settled(&self) -> bool {
        self.status.is_settled()
    }

    /// Structured plain-text block for LLM system-prompt injection.
    ///
    /// Example output:
    /// ```text
    /// ## Hypothesis hyp-001 [active, confidence 0.67]
    /// Temperature increase accelerates enzymatic degradation above 40 °C.
    /// Evidence (2 records):
    ///   [+] run-3 — F-stat 14.2 exceeds threshold — significant effect confirmed
    ///   [-] run-1 — inconclusive at 35 °C — no significant difference
    /// ```
    pub fn context_summary(&self) -> String {
        let mut buf = String::new();
        buf.push_str(&format!(
            "## Hypothesis {} [{}, confidence {:.2}]\n{}\n",
            self.id,
            self.status.label(),
            self.confidence,
            self.statement,
        ));
        if self.evidence.is_empty() {
            buf.push_str("Evidence: none yet.\n");
        } else {
            buf.push_str(&format!("Evidence ({} record(s)):\n", self.evidence.len()));
            for ev in &self.evidence {
                let sign = if ev.supports { "[+]" } else { "[-]" };
                let run = ev.run_id.as_deref().unwrap_or("—");
                let stat = ev.key_statistic.as_ref()
                    .map(|k| format!(" ({} = {:.4})", k.name, k.value))
                    .unwrap_or_default();
                buf.push_str(&format!("  {sign} {run}{stat} — {}\n", ev.summary));
            }
        }
        buf
    }
}

// ── Manager ───────────────────────────────────────────────────────────────────

/// In-memory store of all hypotheses for a session.
///
/// Insertion order is preserved so the system-prompt block is deterministic.
pub struct HypothesisManager {
    hypotheses:      HashMap<String, Hypothesis>,
    insertion_order: Vec<String>,
}

impl Default for HypothesisManager {
    fn default() -> Self {
        Self {
            hypotheses:      HashMap::new(),
            insertion_order: Vec::new(),
        }
    }
}

impl HypothesisManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new hypothesis in `Proposed` state.
    ///
    /// Returns the generated id.
    pub fn propose(&mut self, statement: impl Into<String>, now: i64) -> String {
        let id = format!("hyp-{}", uuid::Uuid::new_v4().simple());
        let h = Hypothesis::new(id.clone(), statement.into(), now);
        self.insertion_order.push(id.clone());
        self.hypotheses.insert(id.clone(), h);
        id
    }

    /// Append evidence to an existing hypothesis.
    ///
    /// Returns `Err` if the hypothesis is unknown or already settled.
    pub fn record_evidence(
        &mut self,
        id:     &str,
        ev:     Evidence,
        policy: &EvidencePolicy,
        now:    i64,
    ) -> Result<(), HypothesisError> {
        let h = self.hypotheses.get_mut(id)
            .ok_or_else(|| HypothesisError::NotFound(id.into()))?;
        if h.is_settled() {
            return Err(HypothesisError::AlreadySettled(id.into()));
        }
        h.add_evidence(ev, policy, now);
        Ok(())
    }

    /// Build the full hypothesis context block for the LLM system prompt.
    ///
    /// Active hypotheses appear first, followed by a brief settled summary.
    pub fn context_block(&self) -> String {
        let mut buf = String::new();
        buf.push_str("# Active Hypotheses\n\n");

        let active: Vec<&Hypothesis> = self.insertion_order.iter()
            .filter_map(|id| self.hypotheses.get(id))
            .filter(|h| !h.is_settled())
            .collect();

        if active.is_empty() {
            buf.push_str("None.\n");
        } else {
            for h in active {
                buf.push_str(&h.context_summary());
                buf.push('\n');
            }
        }

        let settled: Vec<&Hypothesis> = self.insertion_order.iter()
            .filter_map(|id| self.hypotheses.get(id))
            .filter(|h| h.is_settled())
            .collect();

        if !settled.is_empty() {
            buf.push_str("# Settled Hypotheses\n\n");
            for h in settled {
                buf.push_str(&format!(
                    "- {} [{}] — {}\n",
                    h.id,
                    h.status.label(),
                    h.statement,
                ));
            }
        }

        buf
    }

    /// All non-settled hypotheses, in insertion order.
    pub fn active(&self) -> Vec<&Hypothesis> {
        self.insertion_order.iter()
            .filter_map(|id| self.hypotheses.get(id))
            .filter(|h| !h.is_settled())
            .collect()
    }

    /// Find a hypothesis whose evidence list references the given experiment id.
    pub fn find_by_experiment(&self, experiment_id: &str) -> Option<&Hypothesis> {
        self.insertion_order.iter()
            .filter_map(|id| self.hypotheses.get(id))
            .find(|h| h.evidence.iter().any(|e| e.experiment_id == experiment_id))
    }

    /// Look up a hypothesis by id (read-only).
    pub fn get(&self, id: &str) -> Option<&Hypothesis> {
        self.hypotheses.get(id)
    }

    /// Look up a hypothesis by id (mutable).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Hypothesis> {
        self.hypotheses.get_mut(id)
    }

    /// Load a pre-existing hypothesis (used when rehydrating from the database).
    pub fn insert(&mut self, h: Hypothesis) {
        if !self.hypotheses.contains_key(&h.id) {
            self.insertion_order.push(h.id.clone());
        }
        self.hypotheses.insert(h.id.clone(), h);
    }

    /// Total number of hypotheses (active + settled).
    pub fn len(&self) -> usize {
        self.hypotheses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hypotheses.is_empty()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, exp: &str, supports: bool) -> Evidence {
        Evidence {
            id:              id.into(),
            experiment_id:   exp.into(),
            run_id:          Some(format!("run-{id}")),
            supports,
            summary:         format!("evidence {id}"),
            key_statistic:   None,
            recorded_at_utc: 0,
        }
    }

    fn policy_n(n: usize) -> EvidencePolicy {
        EvidencePolicy { min_evidence: n, ..EvidencePolicy::default() }
    }

    // ── Status label ─────────────────────────────────────────────────────────

    #[test]
    fn label_roundtrip() {
        assert_eq!(HypothesisStatus::Proposed.label(), "proposed");
        assert_eq!(HypothesisStatus::Active.label(), "active");
        assert_eq!(HypothesisStatus::Supported.label(), "supported");
        assert_eq!(HypothesisStatus::Refuted.label(), "refuted");
        assert_eq!(
            HypothesisStatus::Revised { prior: Box::new(HypothesisStatus::Active) }.label(),
            "revised"
        );
    }

    #[test]
    fn settled_flags() {
        assert!(!HypothesisStatus::Proposed.is_settled());
        assert!(!HypothesisStatus::Active.is_settled());
        assert!(HypothesisStatus::Supported.is_settled());
        assert!(HypothesisStatus::Refuted.is_settled());
    }

    // ── Hypothesis lifecycle ─────────────────────────────────────────────────

    #[test]
    fn proposed_to_active_on_first_evidence() {
        let mut h = Hypothesis::new("h1".into(), "stmt".into(), 0);
        assert_eq!(h.status, HypothesisStatus::Proposed);
        h.add_evidence(ev("e1", "exp-1", true), &policy_n(3), 1);
        assert_eq!(h.status, HypothesisStatus::Active);
    }

    #[test]
    fn auto_supported_at_threshold() {
        let mut h = Hypothesis::new("h1".into(), "stmt".into(), 0);
        let p = policy_n(3);
        h.add_evidence(ev("e1", "exp-1", true), &p, 1);
        h.add_evidence(ev("e2", "exp-2", true), &p, 2);
        assert_eq!(h.status, HypothesisStatus::Active); // < 3 yet
        h.add_evidence(ev("e3", "exp-3", true), &p, 3);
        assert_eq!(h.status, HypothesisStatus::Supported);
    }

    #[test]
    fn auto_refuted_at_threshold() {
        let mut h = Hypothesis::new("h1".into(), "stmt".into(), 0);
        let p = policy_n(3);
        h.add_evidence(ev("e1", "exp-1", false), &p, 1);
        h.add_evidence(ev("e2", "exp-2", false), &p, 2);
        h.add_evidence(ev("e3", "exp-3", false), &p, 3);
        assert_eq!(h.status, HypothesisStatus::Refuted);
    }

    #[test]
    fn mixed_evidence_stays_active() {
        let mut h = Hypothesis::new("h1".into(), "stmt".into(), 0);
        let p = policy_n(3);
        h.add_evidence(ev("e1", "exp-1", true), &p, 1);
        h.add_evidence(ev("e2", "exp-2", false), &p, 2);
        h.add_evidence(ev("e3", "exp-3", true), &p, 3);
        // confidence = 2/3 — neither threshold reached
        assert_eq!(h.status, HypothesisStatus::Active);
        assert!((h.confidence - 2.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn confidence_initialises_to_neutral() {
        let h = Hypothesis::new("h1".into(), "stmt".into(), 0);
        assert_eq!(h.confidence, 0.5);
    }

    #[test]
    fn revise_preserves_prior() {
        let mut h = Hypothesis::new("h1".into(), "stmt".into(), 0);
        h.status = HypothesisStatus::Active;
        h.revise(10);
        if let HypothesisStatus::Revised { prior } = &h.status {
            assert_eq!(**prior, HypothesisStatus::Active);
        } else {
            panic!("expected Revised");
        }
    }

    // ── Manager ──────────────────────────────────────────────────────────────

    #[test]
    fn propose_and_retrieve() {
        let mut mgr = HypothesisManager::new();
        let id = mgr.propose("test hypothesis", 0);
        let h = mgr.get(&id).expect("should exist");
        assert_eq!(h.statement, "test hypothesis");
        assert_eq!(h.status, HypothesisStatus::Proposed);
    }

    #[test]
    fn record_evidence_updates_status() {
        let mut mgr = HypothesisManager::new();
        let p = policy_n(1);
        let id = mgr.propose("test", 0);
        mgr.record_evidence(&id, ev("e1", "exp-1", true), &p, 1).unwrap();
        let h = mgr.get(&id).unwrap();
        assert_eq!(h.status, HypothesisStatus::Supported);
    }

    #[test]
    fn settled_hypothesis_rejects_evidence() {
        let mut mgr = HypothesisManager::new();
        let p = policy_n(1);
        let id = mgr.propose("test", 0);
        mgr.record_evidence(&id, ev("e1", "exp-1", true), &p, 1).unwrap();
        let result = mgr.record_evidence(&id, ev("e2", "exp-2", true), &p, 2);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HypothesisError::AlreadySettled(_)));
    }

    #[test]
    fn unknown_hypothesis_returns_error() {
        let mut mgr = HypothesisManager::new();
        let result = mgr.record_evidence("nonexistent", ev("e1", "exp-1", true), &EvidencePolicy::default(), 0);
        assert!(matches!(result.unwrap_err(), HypothesisError::NotFound(_)));
    }

    #[test]
    fn active_returns_only_unsettled() {
        let mut mgr = HypothesisManager::new();
        let p = policy_n(1);
        let a = mgr.propose("active one", 0);
        let b = mgr.propose("will be settled", 0);
        mgr.record_evidence(&b, ev("e1", "exp-1", true), &p, 1).unwrap();
        let active_ids: Vec<&str> = mgr.active().iter().map(|h| h.id.as_str()).collect();
        assert!(active_ids.contains(&a.as_str()));
        assert!(!active_ids.contains(&b.as_str()));
    }

    #[test]
    fn find_by_experiment_works() {
        let mut mgr = HypothesisManager::new();
        let p = EvidencePolicy::default();
        let id = mgr.propose("stmt", 0);
        mgr.record_evidence(&id, ev("e1", "exp-42", true), &p, 1).unwrap();
        assert!(mgr.find_by_experiment("exp-42").is_some());
        assert!(mgr.find_by_experiment("exp-99").is_none());
    }

    #[test]
    fn context_block_contains_statement() {
        let mut mgr = HypothesisManager::new();
        let id = mgr.propose("temperature affects rate", 0);
        let block = mgr.context_block();
        assert!(block.contains("temperature affects rate"), "block:\n{block}");
        assert!(block.contains(&id), "block:\n{block}");
    }

    #[test]
    fn insert_rehydrates_hypothesis() {
        let mut mgr = HypothesisManager::new();
        let h = Hypothesis {
            id:             "hyp-loaded".into(),
            statement:      "rehydrated".into(),
            status:         HypothesisStatus::Supported,
            evidence:       vec![],
            confidence:     1.0,
            created_at_utc: 0,
            updated_at_utc: 0,
        };
        mgr.insert(h);
        assert_eq!(mgr.len(), 1);
        assert!(mgr.get("hyp-loaded").is_some());
    }
}
