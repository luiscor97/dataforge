-- Migration 0009 — pairwise tree relations (Milestone 0.2, RFC-0001 §19.3).
--
-- `tree_clone_sets` (0006) groups folders whose subtrees are byte-for-byte
-- identical: one signature, N folders. That cannot express the interesting
-- case of §19.4 — two folders that are *almost* the same, where each holds
-- something the other does not. That relation is pairwise and carries the
-- evidence of what would be lost, so it needs its own table.
--
-- This is evidence only. Nothing here proposes an action: a relation with
-- unique content on both sides is precisely a reason NOT to consolidate.

CREATE TABLE tree_relations (
    id             TEXT PRIMARY KEY,
    snapshot_id    TEXT NOT NULL REFERENCES snapshots(id),
    -- Ordered pair: folder_a < folder_b by relative path, so a relation is
    -- stored once. For TREE_EMBEDDED, `contained` names which side is inside
    -- the other; it is not implied by the ordering.
    folder_a       TEXT NOT NULL REFERENCES folders(id),
    folder_b       TEXT NOT NULL REFERENCES folders(id),
    relationship   TEXT NOT NULL,
    -- Which folder is contained in the other ('A' or 'B'), only for
    -- TREE_EMBEDDED; NULL otherwise.
    contained      TEXT CHECK (contained IS NULL OR contained IN ('A', 'B')),
    shared_files   INTEGER NOT NULL CHECK (shared_files >= 0),
    unique_a_files INTEGER NOT NULL CHECK (unique_a_files >= 0),
    unique_b_files INTEGER NOT NULL CHECK (unique_b_files >= 0),
    shared_bytes   INTEGER NOT NULL CHECK (shared_bytes >= 0),
    -- Jaccard index over distinct contents, in [0, 1].
    similarity     REAL NOT NULL CHECK (similarity >= 0.0 AND similarity <= 1.0),
    created_at     TEXT NOT NULL,
    UNIQUE (snapshot_id, folder_a, folder_b),
    -- A relation is between two *different* folders.
    CHECK (folder_a <> folder_b),
    -- An embedded relation means one side has nothing of its own.
    CHECK (
        relationship <> 'TREE_EMBEDDED'
        OR (contained = 'A' AND unique_a_files = 0)
        OR (contained = 'B' AND unique_b_files = 0)
    ),
    -- A partial clone means both sides hold something unique: that is what
    -- makes it a warning rather than a consolidation opportunity (§19.4).
    CHECK (
        relationship <> 'PARTIAL_TREE_CLONE'
        OR (unique_a_files > 0 AND unique_b_files > 0)
    )
) STRICT;

CREATE INDEX idx_tree_relations_snapshot ON tree_relations(snapshot_id, relationship);
CREATE INDEX idx_tree_relations_folder_a ON tree_relations(folder_a);
CREATE INDEX idx_tree_relations_folder_b ON tree_relations(folder_b);
