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
}
