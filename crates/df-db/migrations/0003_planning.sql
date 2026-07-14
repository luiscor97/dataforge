-- Migration 0003 — planning and verification schema (Milestone 0.1).
--
-- Adds: duplicate_sets (analysis output, RFC-0001 §15), plans and
-- plan_operations (§9.9, §9.10, §26), operation_results (§27) and the
-- verification tables (§28). Same conventions as 0001/0002.

-- Exact duplicate sets materialised by the analysis phase. Membership is
-- derivable (occurrence_content), so only the set row is stored.
CREATE TABLE duplicate_sets (
    id               TEXT PRIMARY KEY,
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    content_id       TEXT NOT NULL REFERENCES content_objects(id),
    occurrence_count INTEGER NOT NULL CHECK (occurrence_count >= 2),
    size_bytes       INTEGER NOT NULL CHECK (size_bytes >= 0),
    created_at       TEXT NOT NULL,
    UNIQUE (snapshot_id, content_id)
) STRICT;

CREATE TABLE plans (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES projects(id),
    snapshot_id       TEXT NOT NULL REFERENCES snapshots(id),
    version           INTEGER NOT NULL CHECK (version >= 1),
    status            TEXT NOT NULL,
    serialized_sha256 TEXT CHECK (serialized_sha256 IS NULL OR length(serialized_sha256) = 64),
    created_at        TEXT NOT NULL,
    approved_at       TEXT,
    UNIQUE (project_id, version)
) STRICT;

-- Plans are never deleted; superseded versions stay for audit.
CREATE TRIGGER plans_no_delete
BEFORE DELETE ON plans
BEGIN
    SELECT RAISE(ABORT, 'plans are never deleted');
END;

CREATE TABLE plan_operations (
    id                        TEXT PRIMARY KEY,
    plan_id                   TEXT NOT NULL REFERENCES plans(id),
    sequence                  INTEGER NOT NULL CHECK (sequence >= 1),
    operation_type            TEXT NOT NULL,
    source_occurrence         TEXT REFERENCES path_occurrences(id),
    content_id                TEXT REFERENCES content_objects(id),
    destination_relative_path TEXT,
    confidence                REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    risk                      TEXT NOT NULL,
    approval                  TEXT NOT NULL,
    execution_state           TEXT NOT NULL,
    idempotency_key           TEXT NOT NULL UNIQUE CHECK (length(idempotency_key) = 64),
    reason                    TEXT NOT NULL,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    UNIQUE (plan_id, sequence)
) STRICT;

CREATE INDEX idx_plan_operations_plan ON plan_operations(plan_id, execution_state);

CREATE TRIGGER plan_operations_no_delete
BEFORE DELETE ON plan_operations
BEGIN
    SELECT RAISE(ABORT, 'plan operations are never deleted');
END;

-- Once a plan is approved it is immutable (§26.4): only execution progress
-- and review metadata may change on its operations.
CREATE TRIGGER plan_operations_frozen_after_approval
BEFORE UPDATE OF plan_id, sequence, operation_type, source_occurrence,
                 content_id, destination_relative_path, idempotency_key
ON plan_operations
WHEN (SELECT status FROM plans WHERE id = OLD.plan_id) = 'APPROVED'
BEGIN
    SELECT RAISE(ABORT, 'approved plans are immutable');
END;

-- Append-only journal of execution attempts (§27.1 "record result").
-- final_relative_path is where the artefact actually landed: it may differ
-- from the planned destination when a collision forced the deterministic
-- suffix of §27.3.
CREATE TABLE operation_results (
    id                  TEXT PRIMARY KEY,
    operation_id        TEXT NOT NULL REFERENCES plan_operations(id),
    outcome             TEXT NOT NULL,
    error_code          TEXT,
    detail              TEXT,
    final_relative_path TEXT,
    bytes_copied        INTEGER NOT NULL DEFAULT 0 CHECK (bytes_copied >= 0),
    sha256              TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
    blake3              TEXT CHECK (blake3 IS NULL OR length(blake3) = 64),
    started_at          TEXT NOT NULL,
    finished_at         TEXT NOT NULL,
    created_at          TEXT NOT NULL
) STRICT;

CREATE INDEX idx_operation_results_operation ON operation_results(operation_id);

CREATE TRIGGER operation_results_no_update
BEFORE UPDATE ON operation_results
BEGIN
    SELECT RAISE(ABORT, 'operation_results is append-only');
END;

CREATE TRIGGER operation_results_no_delete
BEFORE DELETE ON operation_results
BEGIN
    SELECT RAISE(ABORT, 'operation_results is append-only');
END;

CREATE TABLE verification_runs (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(id),
    plan_id        TEXT NOT NULL REFERENCES plans(id),
    verdict        TEXT NOT NULL,
    checked        INTEGER NOT NULL DEFAULT 0 CHECK (checked >= 0),
    problems       INTEGER NOT NULL DEFAULT 0 CHECK (problems >= 0),
    warnings       INTEGER NOT NULL DEFAULT 0 CHECK (warnings >= 0),
    started_at     TEXT NOT NULL,
    finished_at    TEXT NOT NULL,
    created_at     TEXT NOT NULL
) STRICT;

CREATE INDEX idx_verification_runs_plan ON verification_runs(plan_id);

CREATE TABLE verification_findings (
    id                  TEXT PRIMARY KEY,
    verification_run_id TEXT NOT NULL REFERENCES verification_runs(id),
    kind                TEXT NOT NULL,
    severity            TEXT NOT NULL CHECK (severity IN ('PROBLEM', 'WARNING')),
    subject             TEXT NOT NULL,
    detail              TEXT NOT NULL,
    created_at          TEXT NOT NULL
) STRICT;

CREATE INDEX idx_verification_findings_run ON verification_findings(verification_run_id);
