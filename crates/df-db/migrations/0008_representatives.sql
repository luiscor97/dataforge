-- Migration 0006 — logical representative of each duplicate set (M0.2).
--
-- RFC-0001 §15.5: within a set of exact duplicates, score every occurrence
-- and record which one is the best *canonical* location. This is evidence,
-- not an action: §15.5 states explicitly that the logical representative
-- does not imply deleting the other occurrences, and rule 8 keeps a duplicate
-- from being automatically dispensable.
--
-- `reason` carries the human explanation of the decision (§5.3
-- explainable-by-design, M0.2 "evidencia por decisión").
CREATE TABLE duplicate_representatives (
    duplicate_set_id TEXT PRIMARY KEY REFERENCES duplicate_sets(id),
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    occurrence_id    TEXT NOT NULL REFERENCES path_occurrences(id),
    -- Higher is better; it is the negated cost of the location.
    score            INTEGER NOT NULL,
    reason           TEXT NOT NULL,
    created_at       TEXT NOT NULL
) STRICT;

CREATE INDEX idx_duplicate_representatives_snapshot
    ON duplicate_representatives(snapshot_id);
