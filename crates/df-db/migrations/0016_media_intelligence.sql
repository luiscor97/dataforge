-- Migration 0016 — media intelligence evidence (Milestone 0.5).
--
-- Perceptual fingerprints (image pHash, audio Chromaprint, video keyframe
-- pHash) and the review relations derived from comparing them. Same doctrine
-- as similarity (0013): configuration-addressed runs sealed on completion,
-- evidence append-only, and relations that are structurally incapable of
-- authorising a plan operation. SHA-256 remains the only identity.

CREATE TABLE media_runs (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES projects(id),
    snapshot_id       TEXT NOT NULL REFERENCES snapshots(id),
    status            TEXT NOT NULL
        CHECK (status IN ('RUNNING', 'COMPLETED', 'FAILED')),
    contract_version  TEXT NOT NULL,
    config_digest     TEXT NOT NULL CHECK (length(config_digest) = 64),
    config_json       TEXT NOT NULL,
    contents_total    INTEGER NOT NULL DEFAULT 0 CHECK (contents_total >= 0),
    contents_analyzed INTEGER NOT NULL DEFAULT 0 CHECK (contents_analyzed >= 0),
    contents_limited  INTEGER NOT NULL DEFAULT 0 CHECK (contents_limited >= 0),
    contents_failed   INTEGER NOT NULL DEFAULT 0 CHECK (contents_failed >= 0),
    pairs_compared    INTEGER NOT NULL DEFAULT 0 CHECK (pairs_compared >= 0),
    pair_cap_reached  INTEGER NOT NULL DEFAULT 0 CHECK (pair_cap_reached IN (0, 1)),
    relations_total   INTEGER NOT NULL DEFAULT 0 CHECK (relations_total >= 0),
    error             TEXT,
    started_at        TEXT NOT NULL,
    finished_at       TEXT,
    created_at        TEXT NOT NULL
) STRICT;

CREATE INDEX idx_media_runs_project_snapshot
    ON media_runs(project_id, snapshot_id, created_at);

-- One analysis per (run, content): the full serialized engine contract.
-- `automatic_action` inside the JSON is rejected as true on deserialization
-- by the domain contract itself.
CREATE TABLE media_evidence (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL REFERENCES media_runs(id),
    snapshot_id   TEXT NOT NULL REFERENCES snapshots(id),
    content_id    TEXT NOT NULL REFERENCES content_objects(id),
    media_kind    TEXT NOT NULL CHECK (media_kind IN ('IMAGE', 'AUDIO', 'VIDEO')),
    status        TEXT NOT NULL CHECK (status IN ('EXTRACTED', 'LIMITED', 'FAILED')),
    analysis_json TEXT NOT NULL,
    failure_code  TEXT,
    created_at    TEXT NOT NULL,
    UNIQUE (run_id, content_id)
) STRICT;

CREATE INDEX idx_media_evidence_run
    ON media_evidence(run_id, media_kind, status);

CREATE TABLE media_relations (
    id               TEXT PRIMARY KEY,
    run_id           TEXT NOT NULL REFERENCES media_runs(id),
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    content_a        TEXT NOT NULL REFERENCES content_objects(id),
    content_b        TEXT NOT NULL REFERENCES content_objects(id),
    relation         TEXT NOT NULL CHECK (relation IN
        ('IMAGE_PERCEPTUAL_MATCH', 'AUDIO_ACOUSTIC_MATCH', 'VIDEO_PERCEPTUAL_MATCH')),
    score_millionths INTEGER NOT NULL CHECK (score_millionths BETWEEN 0 AND 1000000),
    evidence_json    TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    CHECK (content_a < content_b),
    UNIQUE (run_id, content_a, content_b, relation)
) STRICT;

CREATE INDEX idx_media_relations_run ON media_relations(run_id, relation);

-- Evidence is append-only forever and only insertable while its run is
-- RUNNING and inside the run's own snapshot.
CREATE TRIGGER media_evidence_no_update BEFORE UPDATE ON media_evidence
BEGIN
    SELECT RAISE(ABORT, 'media evidence is append-only');
END;

CREATE TRIGGER media_evidence_no_delete BEFORE DELETE ON media_evidence
BEGIN
    SELECT RAISE(ABORT, 'media evidence is append-only');
END;

CREATE TRIGGER media_evidence_guard_insert
BEFORE INSERT ON media_evidence
WHEN NOT EXISTS (
    SELECT 1 FROM media_runs r
    WHERE r.id = NEW.run_id
      AND r.status = 'RUNNING'
      AND r.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'media evidence requires a RUNNING run in the same snapshot');
END;

-- A relation may only cite contents whose fingerprints were actually
-- extracted by the same run, and never updates once written. Deleting is
-- allowed only while the run is RUNNING so an interrupted comparison phase
-- can rebuild deterministically before sealing.
CREATE TRIGGER media_relations_guard_insert
BEFORE INSERT ON media_relations
WHEN NOT EXISTS (
    SELECT 1 FROM media_runs r
    WHERE r.id = NEW.run_id
      AND r.status = 'RUNNING'
      AND r.snapshot_id = NEW.snapshot_id
)
  OR NOT EXISTS (
    SELECT 1 FROM media_evidence e
    WHERE e.run_id = NEW.run_id
      AND e.content_id = NEW.content_a
      AND e.status = 'EXTRACTED'
)
  OR NOT EXISTS (
    SELECT 1 FROM media_evidence e
    WHERE e.run_id = NEW.run_id
      AND e.content_id = NEW.content_b
      AND e.status = 'EXTRACTED'
)
BEGIN
    SELECT RAISE(ABORT, 'media relations require extracted evidence in a RUNNING run');
END;

CREATE TRIGGER media_relations_no_update BEFORE UPDATE ON media_relations
BEGIN
    SELECT RAISE(ABORT, 'media relations are immutable');
END;

CREATE TRIGGER media_relations_guard_delete
BEFORE DELETE ON media_relations
WHEN NOT EXISTS (
    SELECT 1 FROM media_runs r WHERE r.id = OLD.run_id AND r.status = 'RUNNING'
)
BEGIN
    SELECT RAISE(ABORT, 'sealed media relations are immutable');
END;

-- Run identity is immutable; only RUNNING rows may move, and only to a
-- terminal status whose summary matches the sealed evidence.
CREATE TRIGGER media_runs_guard_update
BEFORE UPDATE ON media_runs
WHEN OLD.status <> 'RUNNING'
  OR NEW.status = 'RUNNING'
  OR NEW.id <> OLD.id
  OR NEW.project_id <> OLD.project_id
  OR NEW.snapshot_id <> OLD.snapshot_id
  OR NEW.contract_version <> OLD.contract_version
  OR NEW.config_digest <> OLD.config_digest
  OR NEW.config_json <> OLD.config_json
  OR NEW.started_at <> OLD.started_at
  OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'media run identity is immutable or already sealed');
END;

CREATE TRIGGER media_runs_validate_completion
BEFORE UPDATE ON media_runs
WHEN NEW.status = 'COMPLETED' AND (
       NEW.contents_total <> (
           SELECT COUNT(*) FROM media_evidence e WHERE e.run_id = OLD.id
       )
    OR NEW.contents_analyzed <> (
           SELECT COUNT(*) FROM media_evidence e
           WHERE e.run_id = OLD.id AND e.status = 'EXTRACTED'
       )
    OR NEW.contents_limited <> (
           SELECT COUNT(*) FROM media_evidence e
           WHERE e.run_id = OLD.id AND e.status = 'LIMITED'
       )
    OR NEW.contents_failed <> (
           SELECT COUNT(*) FROM media_evidence e
           WHERE e.run_id = OLD.id AND e.status = 'FAILED'
       )
    OR NEW.relations_total <> (
           SELECT COUNT(*) FROM media_relations r WHERE r.run_id = OLD.id
       )
)
BEGIN
    SELECT RAISE(ABORT, 'media run completion summary does not match evidence');
END;

CREATE TRIGGER media_runs_no_delete BEFORE DELETE ON media_runs
BEGIN
    SELECT RAISE(ABORT, 'media runs are append-only');
END;
