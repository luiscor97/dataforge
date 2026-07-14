-- Migration 0002 — inventory schema (Milestone 0.1, scan + hash).
--
-- Adds the tables that the scanner and the hasher need:
--   scan_runs, folders, path_occurrences, content_objects,
--   occurrence_content, hash_jobs (RFC-0001 §10.1).
-- The remaining §10.1 tables (contexts, plans, verification…) arrive with
-- the milestone increment that implements their feature.
--
-- Same conventions as 0001: STRICT tables, enforced foreign keys,
-- created_at everywhere, no binary content stored (RFC-0001 §10.3).

CREATE TABLE scan_runs (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id),
    snapshot_id    TEXT NOT NULL REFERENCES snapshots(id),
    status         TEXT NOT NULL,
    files          INTEGER NOT NULL DEFAULT 0 CHECK (files >= 0),
    folders        INTEGER NOT NULL DEFAULT 0 CHECK (folders >= 0),
    bytes          INTEGER NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    errors         INTEGER NOT NULL DEFAULT 0 CHECK (errors >= 0),
    reparse_points INTEGER NOT NULL DEFAULT 0 CHECK (reparse_points >= 0),
    started_at     TEXT NOT NULL,
    finished_at    TEXT,
    created_at     TEXT NOT NULL
) STRICT;

CREATE INDEX idx_scan_runs_snapshot ON scan_runs(snapshot_id);

-- Directories seen during a scan. The source root itself is stored with
-- relative_path = '' and depth = 0 so error states on the root are
-- representable like any other folder.
CREATE TABLE folders (
    id                   TEXT PRIMARY KEY,
    snapshot_id          TEXT NOT NULL REFERENCES snapshots(id),
    source_root_id       TEXT NOT NULL REFERENCES source_roots(id),
    relative_path        TEXT NOT NULL,
    parent_relative_path TEXT,
    name                 TEXT NOT NULL,
    normalized_name      TEXT NOT NULL,
    depth                INTEGER NOT NULL CHECK (depth >= 0),
    status               TEXT NOT NULL,
    error                TEXT,
    created_at           TEXT NOT NULL,
    UNIQUE (snapshot_id, source_root_id, relative_path)
) STRICT;

CREATE INDEX idx_folders_snapshot ON folders(snapshot_id);

-- One physical appearance of a file (RFC-0001 §9.3). Path and content are
-- distinct entities (rule 7): the content link lives in occurrence_content.
-- The absolute path is source_roots.absolute_path + relative_path; it is
-- not duplicated here.
CREATE TABLE path_occurrences (
    id                   TEXT PRIMARY KEY,
    snapshot_id          TEXT NOT NULL REFERENCES snapshots(id),
    source_root_id       TEXT NOT NULL REFERENCES source_roots(id),
    relative_path        TEXT NOT NULL,
    parent_relative_path TEXT NOT NULL,
    file_name            TEXT NOT NULL,
    normalized_name      TEXT NOT NULL,
    extension            TEXT,
    size_bytes           INTEGER NOT NULL CHECK (size_bytes >= 0),
    created_at_fs        TEXT,
    modified_at_fs       TEXT,
    attributes           INTEGER NOT NULL DEFAULT 0,
    path_length          INTEGER NOT NULL CHECK (path_length >= 0),
    depth                INTEGER NOT NULL CHECK (depth >= 0),
    fingerprint          TEXT NOT NULL,
    scan_status          TEXT NOT NULL,
    error                TEXT,
    name_is_lossy        INTEGER NOT NULL DEFAULT 0 CHECK (name_is_lossy IN (0, 1)),
    created_at           TEXT NOT NULL,
    UNIQUE (snapshot_id, source_root_id, relative_path)
) STRICT;

CREATE INDEX idx_occurrences_snapshot ON path_occurrences(snapshot_id);
CREATE INDEX idx_occurrences_size ON path_occurrences(snapshot_id, size_bytes);
CREATE INDEX idx_occurrences_name ON path_occurrences(snapshot_id, normalized_name);

-- Unique binary content (RFC-0001 §9.4). SHA-256 is the canonical identity
-- (ADR-0007); BLAKE3 is the operational one. No file bytes are stored.
CREATE TABLE content_objects (
    id                  TEXT PRIMARY KEY,
    size_bytes          INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256              TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
    blake3              TEXT CHECK (blake3 IS NULL OR length(blake3) = 64),
    mime_type           TEXT,
    first_seen_snapshot TEXT NOT NULL REFERENCES snapshots(id),
    hash_state          TEXT NOT NULL,
    created_at          TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX idx_content_sha256 ON content_objects(sha256)
    WHERE sha256 IS NOT NULL;

-- Occurrence → content binding, written by the hasher.
CREATE TABLE occurrence_content (
    occurrence_id TEXT PRIMARY KEY REFERENCES path_occurrences(id),
    content_id    TEXT NOT NULL REFERENCES content_objects(id),
    created_at    TEXT NOT NULL
) STRICT;

CREATE INDEX idx_occurrence_content_content ON occurrence_content(content_id);

-- Resumable hash work queue (RFC-0001 §12.3). A killed or paused hash run
-- leaves PENDING rows behind; the next run picks them up.
CREATE TABLE hash_jobs (
    id            TEXT PRIMARY KEY,
    snapshot_id   TEXT NOT NULL REFERENCES snapshots(id),
    occurrence_id TEXT NOT NULL UNIQUE REFERENCES path_occurrences(id),
    status        TEXT NOT NULL,
    error         TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
) STRICT;

CREATE INDEX idx_hash_jobs_pending ON hash_jobs(snapshot_id, status);
