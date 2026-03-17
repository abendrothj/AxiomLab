//! Persistent discovery journal — cross-run scientific memory.
//!
//! Every protocol conclusion and LLM-recorded finding is stored here.
//! On the next run the journal summary is injected into the LLM mandate,
//! turning isolated experiments into an accumulating knowledge base.
//!
//! # Storage
//! The journal is written to `.artifacts/discovery/journal.json` (relative to
//! the server working directory, which is the workspace root).  The file is
//! created on first write; reads succeed silently on a missing file.

use agent_runtime::audit::data_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HypothesisStatus {
    Proposed,
    Testing,
    Confirmed,
    Rejected,
}

impl std::fmt::Display for HypothesisStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Proposed  => write!(f, "Proposed"),
            Self::Testing   => write!(f, "Testing"),
            Self::Confirmed => write!(f, "Confirmed"),
            Self::Rejected  => write!(f, "Rejected"),
        }
    }
}

/// A confirmed or candidate scientific finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    /// One-sentence scientific statement of the finding.
    pub statement: String,
    /// Evidence strings (e.g. "run X: OD600=0.45 @ 25µM").
    pub evidence: Vec<String>,
    pub first_observed_secs: i64,
}

/// A scientific hypothesis and its current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub status: HypothesisStatus,
    pub created_secs: i64,
    pub updated_secs: i64,
}

/// One completed protocol run, summarised for the journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub protocol_name: String,
    pub hypothesis: String,
    pub conclusion: String,
    pub steps_succeeded: usize,
    pub steps_total: usize,
    pub timestamp_secs: i64,
}

/// The persistent discovery journal.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoveryJournal {
    pub schema_version: u32,
    pub findings: Vec<Finding>,
    pub hypotheses: Vec<Hypothesis>,
    pub runs: Vec<RunSummary>,
}

// ── I/O ───────────────────────────────────────────────────────────────────────

pub fn journal_path() -> PathBuf {
    data_dir().join("discovery").join("journal.json")
}

impl DiscoveryJournal {
    /// Load from disk; returns an empty journal if the file doesn't exist.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("Discovery journal parse error ({e}) — starting fresh");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persist to disk, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}

// ── Mutations ─────────────────────────────────────────────────────────────────

impl DiscoveryJournal {
    /// Record the conclusion of a completed protocol run.
    pub fn record_run(
        &mut self,
        run_id: &str,
        protocol_name: &str,
        hypothesis: &str,
        conclusion: &str,
        steps_succeeded: usize,
        steps_total: usize,
    ) {
        self.runs.push(RunSummary {
            run_id: run_id.to_string(),
            protocol_name: protocol_name.to_string(),
            hypothesis: hypothesis.to_string(),
            conclusion: conclusion.to_string(),
            steps_succeeded,
            steps_total,
            timestamp_secs: now_secs(),
        });
        // Keep last 100 runs; older history stays available on disk but not in memory summary.
        if self.runs.len() > 100 {
            self.runs.remove(0);
        }
    }

    /// Add a confirmed finding. Returns the new finding's id.
    pub fn add_finding(&mut self, statement: String, evidence: Vec<String>) -> String {
        let id = Uuid::new_v4().to_string();
        self.findings.push(Finding {
            id: id.clone(),
            statement,
            evidence,
            first_observed_secs: now_secs(),
        });
        id
    }

    /// Add a new hypothesis. Returns the new hypothesis's id.
    pub fn add_hypothesis(&mut self, statement: String) -> String {
        let id = Uuid::new_v4().to_string();
        self.hypotheses.push(Hypothesis {
            id: id.clone(),
            statement,
            status: HypothesisStatus::Proposed,
            created_secs: now_secs(),
            updated_secs: now_secs(),
        });
        id
    }

    /// Update a hypothesis's status by id. Returns false if the id wasn't found.
    pub fn update_hypothesis_status(&mut self, id: &str, status: HypothesisStatus) -> bool {
        if let Some(h) = self.hypotheses.iter_mut().find(|h| h.id == id) {
            h.status = status;
            h.updated_secs = now_secs();
            true
        } else {
            false
        }
    }
}

// ── LLM summary ───────────────────────────────────────────────────────────────

impl DiscoveryJournal {
    /// Render a compact summary block for injection into the LLM mandate.
    ///
    /// Kept under ~600 tokens: all findings (capped at 10), active hypotheses,
    /// and the 5 most recent runs.
    pub fn summary_for_llm(&self) -> String {
        if self.runs.is_empty() && self.findings.is_empty() && self.hypotheses.is_empty() {
            return String::new();
        }

        let active_hyps: Vec<&Hypothesis> = self
            .hypotheses
            .iter()
            .filter(|h| {
                h.status == HypothesisStatus::Proposed || h.status == HypothesisStatus::Testing
            })
            .collect();

        let mut out = format!(
            "\n## Discovery journal ({} runs · {} findings · {} active hypotheses)\n",
            self.runs.len(),
            self.findings.len(),
            active_hyps.len(),
        );

        if !self.findings.is_empty() {
            out.push_str("\n### Confirmed findings:\n");
            for (i, f) in self.findings.iter().take(10).enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, f.statement));
                for ev in f.evidence.iter().take(2) {
                    out.push_str(&format!("   evidence: {ev}\n"));
                }
            }
        }

        if !active_hyps.is_empty() {
            out.push_str("\n### Active hypotheses:\n");
            for h in &active_hyps {
                out.push_str(&format!("• [{}] {} (id: {})\n", h.status, h.statement, h.id));
            }
        }

        if !self.runs.is_empty() {
            out.push_str("\n### Recent runs (newest first):\n");
            for run in self.runs.iter().rev().take(5) {
                let frac = format!("{}/{}", run.steps_succeeded, run.steps_total);
                out.push_str(&format!(
                    "• \"{}\": {frac} steps — {}\n",
                    run.protocol_name,
                    truncate(&run.conclusion, 120),
                ));
            }
        }

        out.push_str(
            "\n(Call update_journal to record a finding or manage a hypothesis. \
             Build on what is known — don't repeat completed experiments.)\n",
        );
        out
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        s
    } else {
        // truncate at a char boundary
        let mut idx = max_chars;
        while !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}
