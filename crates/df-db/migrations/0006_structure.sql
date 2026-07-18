-- Migration 0006 — structural intelligence (Milestone 0.2).
--
-- Folder Merkle signatures (RFC-0001 §19.2) and the exact tree-clone sets
-- derived from them (§19.3). Same conventions as earlier migrations: STRICT,
-- created_at, explicit foreign keys. These tables are analysis evidence,
-- recomputed from the inventory; they hold no file bytes.

-- One Merkle signature per folder of a snapshot. `signature` is NULL when the
-- subtree is incomplete (a descendant file is unhashed, or the subtree has an
-- error entry or an unfollowed reparse point); incomplete folders never take
-- part in a clone set (safety, §19.4).
CREATE TABLE folder_signatures (
    folder_id      TEXT PRIMARY KEY REFERENCES folders(id),
    snapshot_id    TEXT NOT NULL REFERENCES snapshots(id),
    source_root_id TEXT NOT NULL REFERENCES source_roots(id),
    relative_path  TEXT NOT NULL,
    signature      TEXT CHECK (signature IS NULL OR length(signature) = 64),
    is_complete    INTEGER NOT NULL CHECK (is_complete IN (0, 1)),
    subtree_files  INTEGER NOT NULL CHECK (subtree_files >= 0),
    subtree_bytes  INTEGER NOT NULL CHECK (subtree_bytes >= 0),
    created_at     TEXT NOT NULL
) STRICT;

CREATE INDEX idx_folder_signatures_snapshot ON folder_signatures(snapshot_id);
-- Clone detection groups complete folders by signature.
CREATE INDEX idx_folder_signatures_sig
    ON folder_signatures(snapshot_id, signature)
    WHERE signature IS NOT NULL AND is_complete = 1;

-- Materialised groups of two or more complete, non-empty folders that share
-- a signature (§19.3). Members are queried from folder_signatures by
-- (snapshot_id, signature); no per-member table is needed.
CREATE TABLE tree_clone_sets (
    id            TEXT PRIMARY KEY,
    snapshot_id   TEXT NOT NULL REFERENCES snapshots(id),
    signature     TEXT NOT NULL CHECK (length(signature) = 64),
    relationship  TEXT NOT NULL,
    folder_count  INTEGER NOT NULL CHECK (folder_count >= 2),
    subtree_files INTEGER NOT NULL CHECK (subtree_files >= 1),
    subtree_bytes INTEGER NOT NULL CHECK (subtree_bytes >= 0),
    created_at    TEXT NOT NULL,
    UNIQUE (snapshot_id, signature)
) STRICT;
