-- A drag scar becomes a finding somebody can answer.
--
-- The detector has existed since M2.x and the plan invariant has refused to
-- place a scar in the active tree ever since, but the finding never reached
-- the review queue, so there was no item to decide. On the real archive that
-- left a plan unapprovable over `ESCANER\DOCUMENTOS ESCANER\ESCANER` — a
-- branch that genuinely holds nothing of its own, and whose owner had already
-- said to leave it exactly as it was. Both true; only one of them writable.
--
-- SQLite cannot alter a CHECK, so the table is rebuilt. Rows are carried over
-- verbatim: this widens what may be stored and changes nothing already stored.
PRAGMA foreign_keys = OFF;

-- Out of the way first: this trigger on `review_items` reads
-- `structural_anomalies` by name, and the table is about to stop existing for
-- the length of a rebuild. It is recreated verbatim at the end.
DROP TRIGGER IF EXISTS review_items_snapshot_guard;

CREATE TABLE structural_anomalies_new (
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
                          'EMBEDDED_TREE',
                          'DRAG_SCAR'
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

INSERT INTO structural_anomalies_new
SELECT id, snapshot_id, analysis_version, occurrence_id, folder_a, folder_b,
       kind, severity, requires_review, summary, evidence_json, created_at
FROM structural_anomalies;

DROP TABLE structural_anomalies;
ALTER TABLE structural_anomalies_new RENAME TO structural_anomalies;

CREATE INDEX idx_structural_anomalies_snapshot
    ON structural_anomalies(snapshot_id, severity, kind);
CREATE INDEX idx_structural_anomalies_occurrence
    ON structural_anomalies(occurrence_id);

-- Recreated verbatim from 0010. It was dropped before the rebuild because a
-- trigger whose body names a table that is about to disappear cannot survive
-- the gap: SQLite re-parses it on the next statement that touches its table,
-- and fails there rather than where the table went.
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


-- Every trigger the rebuilt table carried, recreated verbatim. Dropping a
-- table drops its triggers, and the guarantees they enforce — append-only,
-- snapshot ownership, sealed after completion — are not the kind that may
-- quietly lapse because a column list changed.
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

PRAGMA foreign_keys = ON;
