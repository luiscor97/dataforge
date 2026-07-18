-- Migration 0017 — plugin ecosystem evidence (Milestone 0.6).
--
-- A registration is the signed, content-addressed identity of a component:
-- append-only forever, so what analysed the corpus is always auditable.
-- Runs and findings follow the sealing doctrine of 0013/0016: findings are
-- observations and suggestions, never operations.

CREATE TABLE plugin_registrations (
    id                       TEXT PRIMARY KEY,
    project_id               TEXT NOT NULL REFERENCES projects(id),
    plugin_id                TEXT NOT NULL,
    plugin_version           TEXT NOT NULL,
    manifest_json            TEXT NOT NULL,
    component_sha256         TEXT NOT NULL CHECK (length(component_sha256) = 64),
    component                BLOB NOT NULL,
    publisher_public_key_hex TEXT NOT NULL CHECK (length(publisher_public_key_hex) = 64),
    signature_hex            TEXT NOT NULL CHECK (length(signature_hex) = 128),
    created_at               TEXT NOT NULL,
    UNIQUE (project_id, plugin_id, plugin_version)
) STRICT;

CREATE TRIGGER plugin_registrations_no_update BEFORE UPDATE ON plugin_registrations
BEGIN
    SELECT RAISE(ABORT, 'plugin registrations are append-only');
END;

CREATE TRIGGER plugin_registrations_no_delete BEFORE DELETE ON plugin_registrations
BEGIN
    SELECT RAISE(ABORT, 'plugin registrations are append-only');
END;

CREATE TABLE plugin_runs (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES projects(id),
    snapshot_id         TEXT NOT NULL REFERENCES snapshots(id),
    registration_id     TEXT NOT NULL REFERENCES plugin_registrations(id),
    status              TEXT NOT NULL
        CHECK (status IN ('RUNNING', 'COMPLETED', 'FAILED')),
    config_digest       TEXT NOT NULL CHECK (length(config_digest) = 64),
    config_json         TEXT NOT NULL,
    subjects_total      INTEGER NOT NULL DEFAULT 0 CHECK (subjects_total >= 0),
    subjects_analyzed   INTEGER NOT NULL DEFAULT 0 CHECK (subjects_analyzed >= 0),
    subjects_failed     INTEGER NOT NULL DEFAULT 0 CHECK (subjects_failed >= 0),
    subject_cap_reached INTEGER NOT NULL DEFAULT 0 CHECK (subject_cap_reached IN (0, 1)),
    findings_total      INTEGER NOT NULL DEFAULT 0 CHECK (findings_total >= 0),
    error               TEXT,
    started_at          TEXT NOT NULL,
    finished_at         TEXT,
    created_at          TEXT NOT NULL,
    UNIQUE (snapshot_id, registration_id, config_digest)
) STRICT;

CREATE INDEX idx_plugin_runs_project_snapshot
    ON plugin_runs(project_id, snapshot_id, created_at);

CREATE TABLE plugin_findings (
    id               TEXT PRIMARY KEY,
    run_id           TEXT NOT NULL REFERENCES plugin_runs(id),
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    subject_id       TEXT NOT NULL,
    code             TEXT NOT NULL,
    severity         TEXT NOT NULL CHECK (severity IN ('INFO', 'WARNING')),
    message          TEXT NOT NULL,
    suggestions_json TEXT NOT NULL,
    evidence_json    TEXT NOT NULL,
    created_at       TEXT NOT NULL
) STRICT;

CREATE INDEX idx_plugin_findings_run ON plugin_findings(run_id, severity);

-- Findings are only writable while their run is RUNNING and in the run's
-- snapshot; they never update. Deleting is allowed only while RUNNING so an
-- interrupted run rebuilds deterministically before sealing.
CREATE TRIGGER plugin_findings_guard_insert
BEFORE INSERT ON plugin_findings
WHEN NOT EXISTS (
    SELECT 1 FROM plugin_runs r
    WHERE r.id = NEW.run_id
      AND r.status = 'RUNNING'
      AND r.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'plugin findings require a RUNNING run in the same snapshot');
END;

CREATE TRIGGER plugin_findings_no_update BEFORE UPDATE ON plugin_findings
BEGIN
    SELECT RAISE(ABORT, 'plugin findings are immutable');
END;

CREATE TRIGGER plugin_findings_guard_delete
BEFORE DELETE ON plugin_findings
WHEN NOT EXISTS (
    SELECT 1 FROM plugin_runs r WHERE r.id = OLD.run_id AND r.status = 'RUNNING'
)
BEGIN
    SELECT RAISE(ABORT, 'sealed plugin findings are immutable');
END;

CREATE TRIGGER plugin_runs_guard_update
BEFORE UPDATE ON plugin_runs
WHEN OLD.status <> 'RUNNING'
  OR NEW.status = 'RUNNING'
  OR NEW.id <> OLD.id
  OR NEW.project_id <> OLD.project_id
  OR NEW.snapshot_id <> OLD.snapshot_id
  OR NEW.registration_id <> OLD.registration_id
  OR NEW.config_digest <> OLD.config_digest
  OR NEW.config_json <> OLD.config_json
  OR NEW.started_at <> OLD.started_at
  OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'plugin run identity is immutable or already sealed');
END;

CREATE TRIGGER plugin_runs_validate_completion
BEFORE UPDATE ON plugin_runs
WHEN NEW.status = 'COMPLETED' AND (
    NEW.findings_total <> (
        SELECT COUNT(*) FROM plugin_findings f WHERE f.run_id = OLD.id
    )
    OR NEW.subjects_total <> NEW.subjects_analyzed + NEW.subjects_failed
)
BEGIN
    SELECT RAISE(ABORT, 'plugin run completion summary does not match evidence');
END;

CREATE TRIGGER plugin_runs_no_delete BEFORE DELETE ON plugin_runs
BEGIN
    SELECT RAISE(ABORT, 'plugin runs are append-only');
END;
