//! SQLite persistence layer for the discovery journal.
//!
//! # Rationale
//! All journal state previously lived in a `Mutex<DiscoveryJournal>` and a
//! JSON flat-file.  After this change the in-memory `Vec`s are preserved for
//! WebSocket broadcasts, while every mutation is **also** written to SQLite for
//! queryable, crash-safe persistence.
//!
//! # Schema
//! Tables are created idempotently on [`Db::open`].  WAL mode is enabled so
//! reads never block writes.  The JSON file is kept as a backup; SQLite is
//! authoritative for queries from this point forward.
//!
//! # Recovery
//! On startup, if all tables are empty (fresh install or deleted DB), the
//! database is reconstructed from the JSON backup via
//! [`Db::reconstruct_from_journal`].

use rusqlite::{params, Connection, Result as SqlResult};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_runtime::hypothesis::{
    Evidence as HypEvidence, Hypothesis as HypState, HypothesisManager, KeyStatistic,
};
use crate::discovery::{CalibrationRecord, DiscoveryJournal, Finding, Hypothesis, RunSummary};

// ── Path ──────────────────────────────────────────────────────────────────────

/// Canonical path for the SQLite database file.
pub fn db_path() -> PathBuf {
    agent_runtime::audit::data_dir()
        .join("discovery")
        .join("journal.db")
}

// ── Connection ────────────────────────────────────────────────────────────────

/// Thin wrapper around a SQLite connection, safe to share via `Arc`.
///
/// All methods acquire the inner `Mutex<Connection>` per-operation.  This is
/// appropriate for the lab's write rate (< 100 rows/minute).  Phase 4A's
/// `LabScheduler` can upgrade to a proper connection pool if contention becomes
/// measurable.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Open (or create) the database at `path` and initialise the schema.
    pub fn open(path: &Path) -> SqlResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Returns `true` when all journal tables are empty.
    ///
    /// Used at startup to decide whether to reconstruct from the JSON backup.
    pub fn is_empty(&self) -> bool {
        let conn = self.conn.lock().expect("db mutex");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM findings", [], |r| r.get(0))
            .unwrap_or(0);
        count == 0
    }

    // ── Findings ──────────────────────────────────────────────────────────────

    pub fn insert_finding(&self, f: &Finding) {
        let conn = self.conn.lock().expect("db mutex");
        let evidence     = serde_json::to_string(&f.evidence).unwrap_or_default();
        let measurements = serde_json::to_string(&f.measurements).unwrap_or_default();
        conn.execute(
            "INSERT OR IGNORE INTO findings
               (id, statement, evidence, measurements, experiment_id, source, first_observed_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                f.id,
                f.statement,
                evidence,
                measurements,
                f.experiment_id,
                f.source,
                f.first_observed_secs,
            ],
        )
        .unwrap_or_else(|e| {
            tracing::warn!(id = %f.id, "DB insert_finding: {e}");
            0
        });
    }

    // ── Hypotheses ────────────────────────────────────────────────────────────

    pub fn upsert_hypothesis(&self, h: &Hypothesis) {
        let conn = self.conn.lock().expect("db mutex");
        let status = h.status.to_string();
        conn.execute(
            "INSERT INTO hypotheses (id, statement, status, created_secs, updated_secs)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               status       = excluded.status,
               updated_secs = excluded.updated_secs",
            params![h.id, h.statement, status, h.created_secs, h.updated_secs],
        )
        .unwrap_or_else(|e| {
            tracing::warn!(id = %h.id, "DB upsert_hypothesis: {e}");
            0
        });
    }

    // ── State-machine hypotheses (agent_runtime::hypothesis) ──────────────────

    /// Upsert a rich hypothesis (from the agent_runtime state machine).
    ///
    /// `status_json` is the serialised `HypothesisStatus` enum; `confidence` is
    /// the fraction of supporting evidence.
    pub fn upsert_hyp_state(&self, h: &HypState) -> SqlResult<()> {
        let conn = self.conn.lock().expect("db mutex");
        let status_json = serde_json::to_string(&h.status)
            .unwrap_or_else(|_| "\"Proposed\"".into());
        conn.execute(
            "INSERT INTO hyp_state (id, statement, status_json, confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               status_json = excluded.status_json,
               confidence  = excluded.confidence,
               updated_at  = excluded.updated_at",
            params![
                h.id, h.statement, status_json, h.confidence,
                h.created_at_utc, h.updated_at_utc,
            ],
        )?;
        Ok(())
    }

    /// Insert one piece of hypothesis evidence.
    pub fn insert_hyp_evidence(&self, hyp_id: &str, e: &HypEvidence) -> SqlResult<()> {
        let conn = self.conn.lock().expect("db mutex");
        let (stat_name, stat_value, stat_threshold) = match &e.key_statistic {
            Some(k) => (Some(&k.name as &str), Some(k.value), k.threshold),
            None    => (None, None, None),
        };
        conn.execute(
            "INSERT OR IGNORE INTO hyp_evidence
               (id, hypothesis_id, experiment_id, run_id, supports,
                summary, key_stat_name, key_stat_value, key_stat_threshold, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                e.id, hyp_id, e.experiment_id, e.run_id,
                e.supports as i64, e.summary,
                stat_name, stat_value, stat_threshold,
                e.recorded_at_utc,
            ],
        )?;
        Ok(())
    }

    /// Load all rich hypotheses from `hyp_state`.
    pub fn load_all_hyp_states(&self) -> SqlResult<Vec<HypState>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut stmt = conn.prepare(
            "SELECT id, statement, status_json, confidence, created_at, updated_at
             FROM hyp_state ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let status_json: String = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                status_json,
                row.get::<_, f64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (id, statement, status_json, confidence, created_at_utc, updated_at_utc) = row?;
            let status = serde_json::from_str(&status_json)
                .unwrap_or(agent_runtime::hypothesis::HypothesisStatus::Proposed);
            out.push(HypState {
                id,
                statement,
                status,
                evidence: Vec::new(), // populated separately
                confidence,
                created_at_utc,
                updated_at_utc,
            });
        }
        Ok(out)
    }

    /// Load all evidence records for a given hypothesis id.
    pub fn load_hyp_evidence(&self, hyp_id: &str) -> SqlResult<Vec<HypEvidence>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut stmt = conn.prepare(
            "SELECT id, experiment_id, run_id, supports,
                    summary, key_stat_name, key_stat_value, key_stat_threshold, recorded_at
             FROM hyp_evidence
             WHERE hypothesis_id = ?1
             ORDER BY recorded_at",
        )?;
        let rows = stmt.query_map(params![hyp_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, Option<f64>>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (id, experiment_id, run_id, supports, summary,
                 stat_name, stat_value, stat_threshold, recorded_at_utc) = row?;
            let key_statistic = stat_name.map(|name| KeyStatistic {
                name,
                value: stat_value.unwrap_or(0.0),
                threshold: stat_threshold,
            });
            out.push(HypEvidence {
                id,
                experiment_id,
                run_id,
                supports: supports != 0,
                summary,
                key_statistic,
                recorded_at_utc,
            });
        }
        Ok(out)
    }

    /// Load all hypotheses with their evidence and rehydrate a [`HypothesisManager`].
    pub fn load_hypothesis_manager(&self) -> SqlResult<HypothesisManager> {
        let mut states = self.load_all_hyp_states()?;
        for h in &mut states {
            h.evidence = self.load_hyp_evidence(&h.id)?;
        }
        let mut mgr = HypothesisManager::new();
        for h in states {
            mgr.insert(h);
        }
        Ok(mgr)
    }

    // ── Runs ──────────────────────────────────────────────────────────────────

    pub fn insert_run(&self, r: &RunSummary) {
        let conn = self.conn.lock().expect("db mutex");
        conn.execute(
            "INSERT OR IGNORE INTO runs
               (run_id, protocol_name, hypothesis, conclusion,
                steps_succeeded, steps_total, timestamp_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                r.run_id,
                r.protocol_name,
                r.hypothesis,
                r.conclusion,
                r.steps_succeeded as i64,
                r.steps_total as i64,
                r.timestamp_secs,
            ],
        )
        .unwrap_or_else(|e| {
            tracing::warn!(run_id = %r.run_id, "DB insert_run: {e}");
            0
        });
    }

    // ── Calibrations ──────────────────────────────────────────────────────────

    pub fn insert_calibration(&self, c: &CalibrationRecord) {
        let conn = self.conn.lock().expect("db mutex");
        conn.execute(
            "INSERT OR IGNORE INTO calibrations
               (id, instrument, standard, offset_val, performed_at_secs, valid_until_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                c.id,
                c.instrument,
                c.standard,
                c.offset,
                c.performed_at_secs,
                c.valid_until_secs,
            ],
        )
        .unwrap_or_else(|e| {
            tracing::warn!(id = %c.id, "DB insert_calibration: {e}");
            0
        });
    }

    // ── Bulk reconstruction ───────────────────────────────────────────────────

    /// Populate all journal tables from an existing [`DiscoveryJournal`].
    ///
    /// Called once on startup when the SQLite file is new or empty, so that a
    /// fresh restart does not lose the JSON-backed history.
    pub fn reconstruct_from_journal(&self, journal: &DiscoveryJournal) {
        for f in &journal.findings     { self.insert_finding(f); }
        for h in &journal.hypotheses   { self.upsert_hypothesis(h); }
        for r in &journal.runs         { self.insert_run(r); }
        for c in &journal.calibrations { self.insert_calibration(c); }
        tracing::info!(
            findings     = journal.findings.len(),
            hypotheses   = journal.hypotheses.len(),
            runs         = journal.runs.len(),
            calibrations = journal.calibrations.len(),
            "SQLite journal reconstructed from JSON backup"
        );
    }
}

// ── Schema ────────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS findings (
    id                  TEXT PRIMARY KEY,
    statement           TEXT    NOT NULL,
    evidence            TEXT    NOT NULL,   -- JSON array
    measurements        TEXT    NOT NULL,   -- JSON array
    experiment_id       TEXT,
    source              TEXT    NOT NULL,
    first_observed_secs INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS hypotheses (
    id           TEXT    PRIMARY KEY,
    statement    TEXT    NOT NULL,
    status       TEXT    NOT NULL,
    created_secs INTEGER NOT NULL,
    updated_secs INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    run_id           TEXT    PRIMARY KEY,
    protocol_name    TEXT    NOT NULL,
    hypothesis       TEXT    NOT NULL,
    conclusion       TEXT    NOT NULL,
    steps_succeeded  INTEGER NOT NULL,
    steps_total      INTEGER NOT NULL,
    timestamp_secs   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS calibrations (
    id                TEXT    PRIMARY KEY,
    instrument        TEXT    NOT NULL,
    standard          TEXT    NOT NULL,
    offset_val        REAL    NOT NULL,
    performed_at_secs INTEGER NOT NULL,
    valid_until_secs  INTEGER
);

-- Rich hypothesis state machine (agent_runtime::hypothesis).
-- Distinct from the simpler 'hypotheses' table used by DiscoveryJournal.
CREATE TABLE IF NOT EXISTS hyp_state (
    id          TEXT    PRIMARY KEY,
    statement   TEXT    NOT NULL,
    status_json TEXT    NOT NULL,   -- serialised HypothesisStatus
    confidence  REAL    NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS hyp_evidence (
    id                 TEXT    PRIMARY KEY,
    hypothesis_id      TEXT    NOT NULL REFERENCES hyp_state(id),
    experiment_id      TEXT    NOT NULL,
    run_id             TEXT,
    supports           INTEGER NOT NULL,   -- 1 = true, 0 = false
    summary            TEXT    NOT NULL,
    key_stat_name      TEXT,
    key_stat_value     REAL,
    key_stat_threshold REAL,
    recorded_at        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_hyp_evidence_hyp ON hyp_evidence(hypothesis_id);
CREATE INDEX IF NOT EXISTS idx_hyp_evidence_exp ON hyp_evidence(experiment_id);

-- Lightweight index for fast audit log queries (Phase 4D will fully leverage this).
CREATE TABLE IF NOT EXISTS audit_index (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    unix_secs  INTEGER NOT NULL,
    action     TEXT    NOT NULL,
    decision   TEXT    NOT NULL,
    trace_id   TEXT    NOT NULL,
    entry_hash TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_action   ON audit_index (action);
CREATE INDEX IF NOT EXISTS idx_audit_decision ON audit_index (decision);
CREATE INDEX IF NOT EXISTS idx_audit_time     ON audit_index (unix_secs);
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_and_schema_runs() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let db = Db { conn: Mutex::new(conn) };
        assert!(db.is_empty());
    }

    #[test]
    fn insert_and_detect_non_empty() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let db = Db { conn: Mutex::new(conn) };

        use crate::discovery::{Finding, Measurement};
        let f = Finding {
            id: "f1".into(),
            statement: "test finding".into(),
            evidence: vec!["ev1".into()],
            measurements: vec![Measurement {
                parameter: "ec50".into(),
                value: 1.5,
                unit: "µM".into(),
                uncertainty: Some(0.1),
            }],
            experiment_id: None,
            source: "system".into(),
            first_observed_secs: 0,
        };
        db.insert_finding(&f);
        assert!(!db.is_empty());
    }

    // ── hyp_state / hyp_evidence round-trips ──────────────────────────────────

    fn open_in_mem() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        Db { conn: Mutex::new(conn) }
    }

    #[test]
    fn upsert_and_load_hyp_state() {
        use agent_runtime::hypothesis::{Hypothesis as HypState, HypothesisStatus};
        let db = open_in_mem();

        let h = HypState {
            id:             "hyp-001".into(),
            statement:      "temperature increases rate".into(),
            status:         HypothesisStatus::Active,
            evidence:       vec![],
            confidence:     0.5,
            created_at_utc: 1_000,
            updated_at_utc: 1_001,
        };
        db.upsert_hyp_state(&h).unwrap();

        let loaded = db.load_all_hyp_states().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "hyp-001");
        assert_eq!(loaded[0].statement, "temperature increases rate");
        assert_eq!(loaded[0].status, HypothesisStatus::Active);
        assert!((loaded[0].confidence - 0.5).abs() < 1e-9);
    }

    #[test]
    fn upsert_updates_status() {
        use agent_runtime::hypothesis::{Hypothesis as HypState, HypothesisStatus};
        let db = open_in_mem();

        let mut h = HypState {
            id:             "hyp-002".into(),
            statement:      "stmt".into(),
            status:         HypothesisStatus::Proposed,
            evidence:       vec![],
            confidence:     0.5,
            created_at_utc: 0,
            updated_at_utc: 0,
        };
        db.upsert_hyp_state(&h).unwrap();
        h.status = HypothesisStatus::Supported;
        h.confidence = 1.0;
        h.updated_at_utc = 999;
        db.upsert_hyp_state(&h).unwrap();

        let loaded = db.load_all_hyp_states().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].status, HypothesisStatus::Supported);
        assert!((loaded[0].confidence - 1.0).abs() < 1e-9);
    }

    #[test]
    fn insert_and_load_evidence() {
        use agent_runtime::hypothesis::{
            Evidence as HypEvidence, Hypothesis as HypState, HypothesisStatus, KeyStatistic,
        };
        let db = open_in_mem();

        let h = HypState {
            id:             "hyp-003".into(),
            statement:      "stmt".into(),
            status:         HypothesisStatus::Active,
            evidence:       vec![],
            confidence:     0.5,
            created_at_utc: 0,
            updated_at_utc: 0,
        };
        db.upsert_hyp_state(&h).unwrap();

        let ev = HypEvidence {
            id:              "ev-001".into(),
            experiment_id:   "exp-42".into(),
            run_id:          Some("run-7".into()),
            supports:        true,
            summary:         "F-stat = 14.2, significant".into(),
            key_statistic:   Some(KeyStatistic { name: "f_statistic".into(), value: 14.2, threshold: Some(4.0) }),
            recorded_at_utc: 500,
        };
        db.insert_hyp_evidence("hyp-003", &ev).unwrap();

        let loaded = db.load_hyp_evidence("hyp-003").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].experiment_id, "exp-42");
        assert!(loaded[0].supports);
        let ks = loaded[0].key_statistic.as_ref().unwrap();
        assert_eq!(ks.name, "f_statistic");
        assert!((ks.value - 14.2).abs() < 1e-9);
        assert_eq!(ks.threshold, Some(4.0));
    }

    #[test]
    fn load_hypothesis_manager_roundtrip() {
        use agent_runtime::hypothesis::{
            Evidence as HypEvidence, Hypothesis as HypState, HypothesisStatus,
        };
        let db = open_in_mem();

        let h = HypState {
            id:             "hyp-004".into(),
            statement:      "complex stmt".into(),
            status:         HypothesisStatus::Supported,
            evidence:       vec![],
            confidence:     1.0,
            created_at_utc: 0,
            updated_at_utc: 0,
        };
        db.upsert_hyp_state(&h).unwrap();
        let ev = HypEvidence {
            id:              "ev-002".into(),
            experiment_id:   "exp-10".into(),
            run_id:          None,
            supports:        true,
            summary:         "confirmed".into(),
            key_statistic:   None,
            recorded_at_utc: 0,
        };
        db.insert_hyp_evidence("hyp-004", &ev).unwrap();

        let mgr = db.load_hypothesis_manager().unwrap();
        let loaded = mgr.get("hyp-004").expect("must exist");
        assert_eq!(loaded.evidence.len(), 1);
        assert_eq!(loaded.evidence[0].experiment_id, "exp-10");
    }
}
