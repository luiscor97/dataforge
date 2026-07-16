-- Migration 0010 — structural rules, anomalies and human review (M0.2).
--
-- Every row is evidence derived from an immutable snapshot. Rule matches,
-- anomalies and review items are deterministic and append-only; rerunning an
-- interrupted analysis inserts the same identifiers and is therefore safe.
-- Human decisions are a separate append-only stream so the original finding
-- is never rewritten or erased.

-- ADR-0026 originally stored only the marker. Keep the profile-authored
-- explanation beside the classification so protected-boundary decisions are
-- self-contained evidence.
ALTER TABLE folder_contexts ADD COLUMN reason TEXT;
UPDATE folder_contexts
SET reason = 'protected boundary declared by profile marker `' ||
             COALESCE(marker, 'unknown') || '`'
WHERE kind = 'PROTECTED' AND reason IS NULL;

CREATE TABLE rule_matches (
    id             TEXT PRIMARY KEY,
    snapshot_id    TEXT NOT NULL REFERENCES snapshots(id),
    occurrence_id  TEXT NOT NULL REFERENCES path_occurrences(id),
    analysis_version INTEGER NOT NULL CHECK (analysis_version > 0),
    profile_id      TEXT NOT NULL,
    profile_sha256  TEXT NOT NULL CHECK (length(profile_sha256) = 64),
    rule_id         TEXT NOT NULL,
    rule_version    INTEGER NOT NULL CHECK (rule_version > 0),
    priority        INTEGER NOT NULL CHECK (priority >= 0),
    is_selected     INTEGER NOT NULL CHECK (is_selected IN (0, 1)),
    category        TEXT NOT NULL,
    action          TEXT NOT NULL CHECK (action IN (
                        'COPY_ACTIVE', 'COPY_REVIEW', 'COPY_SEPARATED',
                        'COPY_TEMPORARY'
                    )),
    confidence      REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    risk            TEXT NOT NULL CHECK (risk IN ('LOW', 'MEDIUM', 'HIGH')),
    evidence_json   TEXT NOT NULL CHECK (json_valid(evidence_json)),
    created_at      TEXT NOT NULL,
    UNIQUE (snapshot_id, occurrence_id, analysis_version, rule_id, rule_version)
) STRICT;

CREATE INDEX idx_rule_matches_snapshot ON rule_matches(snapshot_id, action);
CREATE INDEX idx_rule_matches_occurrence ON rule_matches(occurrence_id);
CREATE UNIQUE INDEX idx_rule_matches_selected
    ON rule_matches(snapshot_id, occurrence_id, analysis_version)
    WHERE is_selected = 1;
CREATE UNIQUE INDEX idx_rule_matches_priority
    ON rule_matches(snapshot_id, occurrence_id, analysis_version, priority);

CREATE TABLE structural_anomalies (
    id               TEXT PRIMARY KEY,
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    analysis_version INTEGER NOT NULL CHECK (analysis_version > 0),
    occurrence_id    TEXT REFERENCES path_occurrences(id),
    folder_a          TEXT REFERENCES folders(id),
    folder_b          TEXT REFERENCES folders(id),
    kind              TEXT NOT NULL CHECK (kind IN (
                          'SAME_NAME_DIFFERENT_CONTENT',
                          'LOSSY_PATH_IDENTITY',
                          'UNREADABLE_ENTRY',
                          'EXTREME_PATH',
                          'PARTIAL_TREE_UNIQUE_CONTENT',
                          'EMBEDDED_TREE'
                      )),
    severity          TEXT NOT NULL CHECK (severity IN ('INFO', 'WARNING', 'HIGH')),
    requires_review   INTEGER NOT NULL CHECK (requires_review IN (0, 1)),
    summary           TEXT NOT NULL,
    evidence_json     TEXT NOT NULL CHECK (json_valid(evidence_json)),
    created_at        TEXT NOT NULL,
    CHECK (
        occurrence_id IS NOT NULL
        OR folder_a IS NOT NULL
        OR folder_b IS NOT NULL
    )
) STRICT;

CREATE INDEX idx_structural_anomalies_snapshot
    ON structural_anomalies(snapshot_id, severity, kind);
CREATE INDEX idx_structural_anomalies_occurrence
    ON structural_anomalies(occurrence_id);

CREATE TABLE review_items (
    id                 TEXT PRIMARY KEY,
    snapshot_id        TEXT NOT NULL REFERENCES snapshots(id),
    analysis_version   INTEGER NOT NULL CHECK (analysis_version > 0),
    anomaly_id         TEXT UNIQUE REFERENCES structural_anomalies(id),
    rule_match_id      TEXT UNIQUE REFERENCES rule_matches(id),
    occurrence_id      TEXT REFERENCES path_occurrences(id),
    recommended_action TEXT NOT NULL CHECK (recommended_action IN (
                           'COPY_ACTIVE', 'COPY_REVIEW', 'COPY_SEPARATED',
                           'COPY_TEMPORARY'
                       )),
    risk               TEXT NOT NULL CHECK (risk IN ('LOW', 'MEDIUM', 'HIGH')),
    reason             TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    CHECK (
        (anomaly_id IS NOT NULL AND rule_match_id IS NULL)
        OR (anomaly_id IS NULL AND rule_match_id IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_review_items_snapshot ON review_items(snapshot_id, risk);
CREATE INDEX idx_review_items_occurrence ON review_items(occurrence_id);

CREATE TABLE review_decisions (
    id             TEXT PRIMARY KEY,
    review_item_id TEXT NOT NULL REFERENCES review_items(id),
    sequence       INTEGER NOT NULL CHECK (sequence > 0),
    decision       TEXT NOT NULL CHECK (decision IN (
                       'COPY_ACTIVE', 'COPY_REVIEW', 'COPY_SEPARATED',
                       'COPY_TEMPORARY'
                   )),
    rationale      TEXT NOT NULL CHECK (length(trim(rationale)) > 0),
    actor          TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    UNIQUE (review_item_id, sequence)
) STRICT;

CREATE INDEX idx_review_decisions_item
    ON review_decisions(review_item_id, sequence);

-- Written last, in the same transaction as the final analysis event. Reports
-- require this marker so an interrupted multi-stage analysis can never look
-- like a valid empty diagnostic.
CREATE TABLE analysis_completions (
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    project_id       TEXT NOT NULL REFERENCES projects(id),
    analysis_version INTEGER NOT NULL CHECK (analysis_version > 0),
    profile_id       TEXT NOT NULL,
    profile_sha256   TEXT NOT NULL CHECK (length(profile_sha256) = 64),
    summary_json     TEXT NOT NULL CHECK (json_valid(summary_json)),
    created_at       TEXT NOT NULL,
    PRIMARY KEY (snapshot_id, analysis_version)
) STRICT;

-- Composite ownership is validated explicitly because the foundation schema
-- predates composite foreign keys for these derived evidence tables.
CREATE TRIGGER rule_matches_snapshot_guard
BEFORE INSERT ON rule_matches
WHEN NOT EXISTS (
    SELECT 1 FROM path_occurrences o
    JOIN snapshots s ON s.id = o.snapshot_id
    JOIN projects p ON p.id = s.project_id
    WHERE o.id = NEW.occurrence_id AND o.snapshot_id = NEW.snapshot_id
      AND s.status = 'COMPLETE' AND p.profile = NEW.profile_id
)
BEGIN
    SELECT RAISE(ABORT, 'rule match scope or profile does not match');
END;

CREATE TRIGGER structural_anomalies_snapshot_guard
BEFORE INSERT ON structural_anomalies
WHEN NOT EXISTS (
         SELECT 1 FROM snapshots s
         WHERE s.id = NEW.snapshot_id AND s.status = 'COMPLETE'
     )
   OR (NEW.occurrence_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM path_occurrences o
          WHERE o.id = NEW.occurrence_id AND o.snapshot_id = NEW.snapshot_id
      ))
   OR (NEW.folder_a IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM folders f
          WHERE f.id = NEW.folder_a AND f.snapshot_id = NEW.snapshot_id
      ))
   OR (NEW.folder_b IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM folders f
          WHERE f.id = NEW.folder_b AND f.snapshot_id = NEW.snapshot_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'anomaly subject belongs to another snapshot');
END;

CREATE TRIGGER review_items_snapshot_guard
BEFORE INSERT ON review_items
WHEN NOT EXISTS (
         SELECT 1 FROM snapshots s
         WHERE s.id = NEW.snapshot_id AND s.status = 'COMPLETE'
     )
   OR (NEW.occurrence_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM path_occurrences o
          WHERE o.id = NEW.occurrence_id AND o.snapshot_id = NEW.snapshot_id
      ))
   OR (NEW.anomaly_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM structural_anomalies a
          WHERE a.id = NEW.anomaly_id
            AND a.snapshot_id = NEW.snapshot_id
            AND a.analysis_version = NEW.analysis_version
            AND a.occurrence_id IS NEW.occurrence_id
      ))
   OR (NEW.rule_match_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM rule_matches r
          WHERE r.id = NEW.rule_match_id
            AND r.snapshot_id = NEW.snapshot_id
            AND r.analysis_version = NEW.analysis_version
            AND r.occurrence_id IS NEW.occurrence_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'review source ownership or version does not match');
END;

CREATE TRIGGER analysis_completions_project_guard
BEFORE INSERT ON analysis_completions
WHEN NOT EXISTS (
    SELECT 1 FROM snapshots s
    JOIN projects p ON p.id = s.project_id
    WHERE s.id = NEW.snapshot_id AND s.project_id = NEW.project_id
      AND s.status = 'COMPLETE' AND p.profile = NEW.profile_id
)
BEGIN
    SELECT RAISE(ABORT, 'analysis completion scope or profile does not match');
END;

-- Once the final marker exists, its automatic evidence set is sealed. A
-- crash after this point may replay the lifecycle transition, but cannot add
-- findings behind the immutable summary. Human review decisions remain the
-- only allowed post-completion append stream.
CREATE TRIGGER rule_matches_sealed_after_completion
BEFORE INSERT ON rule_matches
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c
    WHERE c.snapshot_id = NEW.snapshot_id
      AND c.analysis_version = NEW.analysis_version
)
BEGIN
    SELECT RAISE(ABORT, 'completed rule evidence is sealed');
END;

CREATE TRIGGER structural_anomalies_sealed_after_completion
BEFORE INSERT ON structural_anomalies
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c
    WHERE c.snapshot_id = NEW.snapshot_id
      AND c.analysis_version = NEW.analysis_version
)
BEGIN
    SELECT RAISE(ABORT, 'completed anomaly evidence is sealed');
END;

CREATE TRIGGER review_items_sealed_after_completion
BEFORE INSERT ON review_items
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c
    WHERE c.snapshot_id = NEW.snapshot_id
      AND c.analysis_version = NEW.analysis_version
)
BEGIN
    SELECT RAISE(ABORT, 'completed review evidence is sealed');
END;

-- Plan coverage is a database invariant as well as a planner check: one
-- occurrence may appear at most once and must belong to the plan snapshot.
CREATE UNIQUE INDEX idx_plan_operations_occurrence_once
    ON plan_operations(plan_id, source_occurrence)
    WHERE source_occurrence IS NOT NULL;

CREATE TRIGGER plans_snapshot_guard
BEFORE INSERT ON plans
WHEN NOT EXISTS (
    SELECT 1 FROM snapshots s
    WHERE s.id = NEW.snapshot_id AND s.project_id = NEW.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'plan snapshot belongs to another project');
END;

CREATE TRIGGER plan_operations_snapshot_guard
BEFORE INSERT ON plan_operations
WHEN NEW.source_occurrence IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM plans p
    JOIN path_occurrences o ON o.snapshot_id = p.snapshot_id
    WHERE p.id = NEW.plan_id AND o.id = NEW.source_occurrence
)
BEGIN
    SELECT RAISE(ABORT, 'plan occurrence belongs to another snapshot');
END;

CREATE TRIGGER plan_operations_snapshot_guard_update
BEFORE UPDATE OF plan_id, source_occurrence ON plan_operations
WHEN NEW.source_occurrence IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM plans p
    JOIN path_occurrences o ON o.snapshot_id = p.snapshot_id
    WHERE p.id = NEW.plan_id AND o.id = NEW.source_occurrence
)
BEGIN
    SELECT RAISE(ABORT, 'plan occurrence belongs to another snapshot');
END;

CREATE TRIGGER plan_operations_frozen_after_ready
BEFORE UPDATE OF plan_id, sequence, operation_type, source_occurrence,
                 content_id, destination_relative_path, confidence, risk,
                 idempotency_key, reason
ON plan_operations
WHEN (SELECT status FROM plans WHERE id = OLD.plan_id) = 'READY'
BEGIN
    SELECT RAISE(ABORT, 'ready plans are immutable');
END;

-- Evidence is append-only and carries the analyzer version that produced it.
-- A completed snapshot keeps one immutable analysis lineage; re-analysis with
-- a future analyzer starts from a fresh snapshot rather than rewriting it.
CREATE TRIGGER rule_matches_no_update
BEFORE UPDATE ON rule_matches
BEGIN
    SELECT RAISE(ABORT, 'rule matches are append-only');
END;

CREATE TRIGGER rule_matches_no_delete
BEFORE DELETE ON rule_matches
BEGIN
    SELECT RAISE(ABORT, 'rule matches are append-only');
END;

CREATE TRIGGER structural_anomalies_no_update
BEFORE UPDATE ON structural_anomalies
BEGIN
    SELECT RAISE(ABORT, 'structural anomalies are append-only');
END;

CREATE TRIGGER structural_anomalies_no_delete
BEFORE DELETE ON structural_anomalies
BEGIN
    SELECT RAISE(ABORT, 'structural anomalies are append-only');
END;

CREATE TRIGGER review_items_no_update
BEFORE UPDATE ON review_items
BEGIN
    SELECT RAISE(ABORT, 'review items are append-only');
END;

CREATE TRIGGER review_items_no_delete
BEFORE DELETE ON review_items
BEGIN
    SELECT RAISE(ABORT, 'review items are append-only');
END;

CREATE TRIGGER review_decisions_no_update
BEFORE UPDATE ON review_decisions
BEGIN
    SELECT RAISE(ABORT, 'review decisions are append-only');
END;

CREATE TRIGGER review_decisions_no_delete
BEFORE DELETE ON review_decisions
BEGIN
    SELECT RAISE(ABORT, 'review decisions are append-only');
END;

CREATE TRIGGER analysis_completions_no_update
BEFORE UPDATE ON analysis_completions
BEGIN
    SELECT RAISE(ABORT, 'analysis completions are append-only');
END;

CREATE TRIGGER analysis_completions_no_delete
BEFORE DELETE ON analysis_completions
BEGIN
    SELECT RAISE(ABORT, 'analysis completions are append-only');
END;
