-- Migration 0002 — safe inventory core (Milestone 0.1, fase de inventario).
--
-- Tablas para escaneo (snapshots ya existe), hashing y duplicados exactos.
-- Mismas convenciones que 0001: STRICT, created_at, FKs explícitas.

CREATE TABLE scan_runs (
    id            TEXT PRIMARY KEY,
    snapshot_id   TEXT NOT NULL REFERENCES snapshots(id),
    status        TEXT NOT NULL,
    files         INTEGER NOT NULL DEFAULT 0,
    folders       INTEGER NOT NULL DEFAULT 0,
    bytes         INTEGER NOT NULL DEFAULT 0,
    reparse_points INTEGER NOT NULL DEFAULT 0,
    errors        INTEGER NOT NULL DEFAULT 0,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    created_at    TEXT NOT NULL
) STRICT;

CREATE INDEX idx_scan_runs_snapshot ON scan_runs(snapshot_id);

CREATE TABLE folders (
    id             TEXT PRIMARY KEY,
    snapshot_id    TEXT NOT NULL REFERENCES snapshots(id),
    source_root_id TEXT NOT NULL REFERENCES source_roots(id),
    relative_path  TEXT NOT NULL,
    depth          INTEGER NOT NULL,
    entry_count    INTEGER,
    error          TEXT,
    created_at     TEXT NOT NULL,
    UNIQUE (snapshot_id, source_root_id, relative_path)
) STRICT;

CREATE TABLE path_occurrences (
    id             TEXT PRIMARY KEY,
    snapshot_id    TEXT NOT NULL REFERENCES snapshots(id),
    source_root_id TEXT NOT NULL REFERENCES source_roots(id),
    relative_path  TEXT NOT NULL,
    raw_path_utf16 BLOB,
    file_name      TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    extension      TEXT,
    kind           TEXT NOT NULL,
    size_bytes     INTEGER NOT NULL DEFAULT 0,
    created_at_fs  TEXT,
    modified_at_fs TEXT,
    attributes     INTEGER NOT NULL DEFAULT 0,
    depth          INTEGER NOT NULL,
    path_length    INTEGER NOT NULL,
    fingerprint    TEXT,
    scan_status    TEXT NOT NULL,
    error          TEXT,
    created_at     TEXT NOT NULL,
    UNIQUE (snapshot_id, source_root_id, relative_path)
) STRICT;

CREATE INDEX idx_occurrences_snapshot ON path_occurrences(snapshot_id);
CREATE INDEX idx_occurrences_size ON path_occurrences(snapshot_id, size_bytes);

CREATE TABLE content_objects (
    id                  TEXT PRIMARY KEY,
    size_bytes          INTEGER NOT NULL,
    sha256              TEXT NOT NULL CHECK (length(sha256) = 64),
    blake3              TEXT NOT NULL CHECK (length(blake3) = 64),
    first_seen_snapshot TEXT NOT NULL REFERENCES snapshots(id),
    created_at          TEXT NOT NULL,
    UNIQUE (sha256, size_bytes)
) STRICT;

CREATE TABLE occurrence_content (
    occurrence_id TEXT PRIMARY KEY REFERENCES path_occurrences(id),
    content_id    TEXT NOT NULL REFERENCES content_objects(id),
    created_at    TEXT NOT NULL
) STRICT;

CREATE INDEX idx_occurrence_content_content ON occurrence_content(content_id);

CREATE TABLE hash_jobs (
    id            TEXT PRIMARY KEY,
    snapshot_id   TEXT NOT NULL REFERENCES snapshots(id),
    occurrence_id TEXT NOT NULL UNIQUE REFERENCES path_occurrences(id),
    state         TEXT NOT NULL,
    error         TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
) STRICT;

CREATE INDEX idx_hash_jobs_pending ON hash_jobs(snapshot_id, state);

CREATE TABLE duplicate_sets (
    id               TEXT PRIMARY KEY,
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    content_id       TEXT NOT NULL REFERENCES content_objects(id),
    occurrence_count INTEGER NOT NULL CHECK (occurrence_count >= 2),
    wasted_bytes     INTEGER NOT NULL,
    created_at       TEXT NOT NULL,
    UNIQUE (snapshot_id, content_id)
) STRICT;
