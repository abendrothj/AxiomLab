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
use std::collections::HashMap;
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

/// A single numeric measurement extracted from a fit or sensor reading.
/// Sourced from system code (fitting algorithms, sensor drivers), not from LLM output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurement {
    /// Parameter name, e.g. "ec50", "slope", "r_squared", "hill_n".
    pub parameter: String,
    pub value: f64,
    /// Physical unit, e.g. "µM", "AU/µL", "" (dimensionless).
    pub unit: String,
    /// Standard error of the parameter estimate, or `None` if not available.
    pub uncertainty: Option<f64>,
}

/// A confirmed or candidate scientific finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    /// One-sentence scientific statement of the finding.
    pub statement: String,
    /// Evidence strings (e.g. "run X: OD600=0.45 @ 25µM").
    pub evidence: Vec<String>,
    /// Typed numeric measurements extracted from curve fits or sensor readings.
    /// Empty for LLM-narrated findings that carry no structured quantities.
    #[serde(default)]
    pub measurements: Vec<Measurement>,
    /// Experiment that produced this finding, if known.
    #[serde(default)]
    pub experiment_id: Option<String>,
    /// "system" = auto-recorded by fitting/analysis code; "llm" = LLM-curated.
    #[serde(default = "default_llm_source")]
    pub source: String,
    pub first_observed_secs: i64,
}

fn default_llm_source() -> String { "llm".into() }

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

/// Instrument calibration record — created automatically when a calibration
/// tool is called, not by LLM output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationRecord {
    pub id: String,
    /// Instrument identifier, e.g. "ph_meter", "spectrophotometer".
    pub instrument: String,
    /// Human-readable description of the calibration standards used.
    pub standard: String,
    /// Measured drift correction (offset) applied after calibration.
    pub offset: f64,
    pub performed_at_secs: i64,
    /// Unix timestamp after which recalibration is recommended, or `None`.
    pub valid_until_secs: Option<i64>,
}

/// A single parameter value probed during an experiment — used to track
/// coverage of the experimental parameter space across runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterProbe {
    pub tool: String,
    pub parameter: String,
    pub value: f64,
    pub experiment_id: String,
    pub observed_at_secs: i64,
}

/// The persistent discovery journal.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoveryJournal {
    pub schema_version: u32,
    pub findings: Vec<Finding>,
    pub hypotheses: Vec<Hypothesis>,
    pub runs: Vec<RunSummary>,
    /// Instrument calibration history (auto-recorded by tool handlers).
    #[serde(default)]
    pub calibrations: Vec<CalibrationRecord>,
    /// Parameter space probed across experiments (capped at 500 entries).
    #[serde(default)]
    pub coverage: Vec<ParameterProbe>,
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
    ///
    /// - `measurements`: typed numeric results from fitting/analysis code (`source="system"`)
    ///   or empty for LLM-narrated findings (`source="llm"`).
    /// - `source`: `"system"` for auto-recorded findings, `"llm"` for LLM-curated ones.
    pub fn add_finding(
        &mut self,
        statement: String,
        evidence: Vec<String>,
        measurements: Vec<Measurement>,
        experiment_id: Option<String>,
        source: &str,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        self.findings.push(Finding {
            id: id.clone(),
            statement,
            evidence,
            measurements,
            experiment_id,
            source: source.to_owned(),
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

    /// Record an instrument calibration. Returns the new calibration record's id.
    pub fn record_calibration(
        &mut self,
        instrument: &str,
        standard: &str,
        offset: f64,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        self.calibrations.push(CalibrationRecord {
            id: id.clone(),
            instrument: instrument.to_owned(),
            standard: standard.to_owned(),
            offset,
            performed_at_secs: now_secs(),
            valid_until_secs: None,
        });
        id
    }

    /// Return the most recent calibration record for an instrument, or `None`.
    pub fn last_calibration_for(&self, instrument: &str) -> Option<&CalibrationRecord> {
        self.calibrations
            .iter()
            .filter(|c| c.instrument == instrument)
            .max_by_key(|c| c.performed_at_secs)
    }

    /// Record a parameter value observed during an experiment.
    /// Caps the coverage list at 500 entries (trims oldest).
    pub fn record_coverage(&mut self, probe: ParameterProbe) {
        self.coverage.push(probe);
        if self.coverage.len() > 500 {
            let trim = self.coverage.len() - 500;
            self.coverage.drain(0..trim);
        }
    }

    /// Compact coverage summary for the LLM mandate.
    ///
    /// Groups probes by `(tool, parameter)` and reports explored range + count.
    pub fn coverage_summary_for_llm(&self) -> String {
        if self.coverage.is_empty() {
            return String::new();
        }

        // Group: (tool, parameter) -> list of values
        let mut groups: HashMap<(&str, &str), Vec<f64>> = HashMap::new();
        for probe in &self.coverage {
            groups
                .entry((&probe.tool, &probe.parameter))
                .or_default()
                .push(probe.value);
        }

        let mut lines = vec!["## Parameter coverage".to_string()];
        let mut keys: Vec<(&str, &str)> = groups.keys().copied().collect();
        keys.sort_unstable();
        for (tool, param) in keys {
            let vals = &groups[&(tool, param)];
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            lines.push(format!(
                "  {tool}.{param}: [{min:.1}, {max:.1}] · {} values",
                vals.len()
            ));
        }
        lines.join("\n")
    }
}

// ── LLM summary ───────────────────────────────────────────────────────────────

impl DiscoveryJournal {
    /// Render a compact summary block for injection into the LLM mandate.
    ///
    /// Kept under ~600 tokens: all findings (capped at 10), active hypotheses,
    /// and the 5 most recent runs.  Structured measurements are shown inline.
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
                let src_tag = if f.source == "system" { " [auto]" } else { "" };
                out.push_str(&format!("{}. {}{}\n", i + 1, f.statement, src_tag));
                for ev in f.evidence.iter().take(2) {
                    out.push_str(&format!("   evidence: {ev}\n"));
                }
                // Show up to 4 typed measurements
                for m in f.measurements.iter().take(4) {
                    let unc = m.uncertainty
                        .map(|u| format!(" ±{u:.4}"))
                        .unwrap_or_default();
                    let unit = if m.unit.is_empty() { String::new() } else { format!(" {}", m.unit) };
                    out.push_str(&format!(
                        "   {}: {:.4}{}{}\n",
                        m.parameter, m.value, unit, unc
                    ));
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

        // Append parameter coverage
        let coverage = self.coverage_summary_for_llm();
        if !coverage.is_empty() {
            out.push('\n');
            out.push_str(&coverage);
            out.push('\n');
        }

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
        let mut idx = max_chars;
        while !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_finding_with_measurements_round_trips() {
        let mut j = DiscoveryJournal::default();
        let measurements = vec![
            Measurement { parameter: "slope".into(), value: 1.23, unit: "AU/µL".into(), uncertainty: Some(0.05) },
            Measurement { parameter: "r_squared".into(), value: 0.98, unit: "".into(), uncertainty: None },
        ];
        let id = j.add_finding(
            "Linear fit confirmed".into(),
            vec!["run-1: slope=1.23".into()],
            measurements.clone(),
            Some("exp-1".into()),
            "system",
        );
        let f = j.findings.iter().find(|f| f.id == id).unwrap();
        assert_eq!(f.source, "system");
        assert_eq!(f.measurements.len(), 2);
        assert_eq!(f.measurements[0].parameter, "slope");
        assert!((f.measurements[0].value - 1.23).abs() < 1e-9);
        assert_eq!(f.experiment_id.as_deref(), Some("exp-1"));
    }

    #[test]
    fn add_finding_llm_source_default() {
        let mut j = DiscoveryJournal::default();
        let id = j.add_finding("test".into(), vec![], vec![], None, "llm");
        let f = j.findings.iter().find(|f| f.id == id).unwrap();
        assert_eq!(f.source, "llm");
    }

    #[test]
    fn record_calibration_and_last() {
        let mut j = DiscoveryJournal::default();
        j.record_calibration("ph_meter", "pH 4 + pH 7", -0.02);
        std::thread::sleep(std::time::Duration::from_millis(10));
        j.record_calibration("ph_meter", "pH 7 + pH 10", 0.01);
        j.record_calibration("spectrophotometer", "blank water", 0.0);

        let last = j.last_calibration_for("ph_meter").unwrap();
        assert_eq!(last.standard, "pH 7 + pH 10");
        assert!(j.last_calibration_for("centrifuge").is_none());
    }

    #[test]
    fn coverage_capped_at_500() {
        let mut j = DiscoveryJournal::default();
        for i in 0..501 {
            j.record_coverage(ParameterProbe {
                tool: "read_absorbance".into(),
                parameter: "wavelength_nm".into(),
                value: i as f64,
                experiment_id: "exp-1".into(),
                observed_at_secs: i as i64,
            });
        }
        assert_eq!(j.coverage.len(), 500);
        // Oldest entry (0) trimmed; newest (500) retained
        assert!((j.coverage.last().unwrap().value - 500.0).abs() < 1e-9);
    }

    #[test]
    fn coverage_summary_groups_and_ranges() {
        let mut j = DiscoveryJournal::default();
        for wl in [400.0f64, 500.0, 700.0] {
            j.record_coverage(ParameterProbe {
                tool: "read_absorbance".into(),
                parameter: "wavelength_nm".into(),
                value: wl,
                experiment_id: "exp-1".into(),
                observed_at_secs: now_secs(),
            });
        }
        let summary = j.coverage_summary_for_llm();
        assert!(summary.contains("read_absorbance.wavelength_nm"));
        assert!(summary.contains("400.0"));
        assert!(summary.contains("700.0"));
        assert!(summary.contains("3 values"));
    }

    #[test]
    fn old_journal_json_deserializes_with_defaults() {
        // Simulate a journal saved before the new fields existed
        let old_json = r#"{
            "schema_version": 0,
            "findings": [{"id": "x", "statement": "old", "evidence": [], "first_observed_secs": 0}],
            "hypotheses": [],
            "runs": []
        }"#;
        let j: DiscoveryJournal = serde_json::from_str(old_json).unwrap();
        assert!(j.calibrations.is_empty());
        assert!(j.coverage.is_empty());
        // Old finding gets default values
        assert_eq!(j.findings[0].source, "llm");
        assert!(j.findings[0].measurements.is_empty());
    }
}
