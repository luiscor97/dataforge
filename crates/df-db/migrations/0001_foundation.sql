-- Migration 0001 — foundation schema (Milestone 0.0).
--
-- Only the entities that exist in the current milestone get tables.
-- The remaining RFC-0001 §10.1 tables arrive with the milestone that
-- implements their feature, each in its own versioned migration.
--
-- Conventions (RFC-0001 §10.3):
--   * foreign keys are enforced (PRAGMA foreign_keys=ON per connection);
--   * every table carries created_at (RFC 3339 UTC, millisecond precision);
--   * audit_events is append-only, enforced by triggers.

CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL CHECK (length(name) > 0),
    state       TEXT NOT NULL,
    profile     TEXT NOT NULL,
    output_root TEXT NOT NULL,
    audit_root  TEXT NOT NULL,
    app_version TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;

CREATE TABLE source_roots (
    id               TEXT PRIMARY KEY,
    project_id       TEXT NOT NULL REFERENCES projects(id),
    absolute_path    TEXT NOT NULL,
    volume_id        TEXT,
    filesystem       TEXT NOT NULL,
    is_network       INTEGER NOT NULL DEFAULT 0 CHECK (is_network IN (0, 1)),
    is_removable     INTEGER NOT NULL DEFAULT 0 CHECK (is_removable IN (0, 1)),
    read_only_policy INTEGER NOT NULL DEFAULT 1 CHECK (read_only_policy = 1),
    created_at       TEXT NOT NULL
) STRICT;

CREATE INDEX idx_source_roots_project ON source_roots(project_id);

CREATE TABLE snapshots (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    status     TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX idx_snapshots_project ON snapshots(project_id);

CREATE TABLE audit_events (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id),
    sequence      INTEGER NOT NULL CHECK (sequence >= 1),
    timestamp     TEXT NOT NULL,
    previous_hash TEXT NOT NULL CHECK (length(previous_hash) = 64),
    event_type    TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    payload_hash  TEXT NOT NULL CHECK (length(payload_hash) = 64),
    actor         TEXT NOT NULL,
    event_hash    TEXT NOT NULL CHECK (length(event_hash) = 64),
    created_at    TEXT NOT NULL,
    UNIQUE (project_id, sequence)
) STRICT;

-- The ledger is append-only (RFC-0001 §10.3, §29).
CREATE TRIGGER audit_events_no_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events is append-only');
END;

CREATE TRIGGER audit_events_no_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events is append-only');
END;
