//! Immutable event log backed by SQLite.
//!
//! Every orchestrator event is appended to a single `events` table.
//! No rows are ever updated or deleted — the table is a permanent audit trail.

use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};

// ── Schema ────────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;

CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type  TEXT    NOT NULL,
    payload     TEXT    NOT NULL,
    recorded_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_events_time ON events(recorded_at);
";

// ── EventDb ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventDb {
    conn: Arc<Mutex<Connection>>,
}

impl EventDb {
    /// Open (or create) the SQLite database at `path` and apply the schema.
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Return the last `limit` events of `event_type`, ordered oldest-first.
    pub fn query_recent(&self, event_type: &str, limit: usize) -> Vec<serde_json::Value> {
        let conn = match self.conn.lock() {
            Ok(c)  => c,
            Err(_) => return vec![],
        };
        let mut stmt = match conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(s)  => s,
            Err(_) => return vec![],
        };
        let Ok(mapped) = stmt.query_map(params![event_type, limit as i64], |row| row.get::<_, String>(0)) else {
            return vec![];
        };
        let rows: Vec<serde_json::Value> = mapped
            .filter_map(|r| r.ok())
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect();

        // Reverse so the result is chronological (oldest first)
        let mut rows = rows;
        rows.reverse();
        rows
    }

    /// Append a single event.  Called synchronously from `EventSink` methods.
    pub fn record(&self, event_type: &str, payload: &impl serde::Serialize) {
        let json = match serde_json::to_string(payload) {
            Ok(j)  => j,
            Err(e) => { tracing::warn!("db: failed to serialize {event_type}: {e}"); return; }
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        if let Ok(conn) = self.conn.lock() {
            if let Err(e) = conn.execute(
                "INSERT INTO events (event_type, payload, recorded_at) VALUES (?1, ?2, ?3)",
                params![event_type, json, now_ms],
            ) {
                tracing::warn!("db: insert failed: {e}");
            }
        }
    }
}
