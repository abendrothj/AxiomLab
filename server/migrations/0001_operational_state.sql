PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS directives (
    id TEXT PRIMARY KEY,
    directive TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','running','recovery_required','completed','failed','cancelled')),
    summary TEXT,
    created_secs INTEGER NOT NULL,
    updated_secs INTEGER NOT NULL,
    submitted_by TEXT NOT NULL,
    lease_owner TEXT,
    lease_expires_secs INTEGER,
    version INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS directives_status_created
ON directives(status, created_secs);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('viewer','operator','approver','admin','service')),
    csrf_token TEXT NOT NULL,
    created_secs INTEGER NOT NULL,
    expires_secs INTEGER NOT NULL,
    revoked_secs INTEGER
);

CREATE TABLE IF NOT EXISTS approval_records (
    id TEXT PRIMARY KEY,
    run_id TEXT,
    scope_hash TEXT NOT NULL,
    request_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','approved','denied','timed_out','interrupted')),
    requested_by TEXT,
    decided_by TEXT,
    decision_json TEXT,
    created_secs INTEGER NOT NULL,
    resolved_secs INTEGER
);
