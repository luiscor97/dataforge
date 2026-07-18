-- Migration 0007 — folder context classification (Milestone 0.2).
--
-- A deterministic first slice of the context graph (RFC-0001 §18): one row
-- per folder tagging it as a generic low-value container, a protected
-- boundary, or neutral. Analysis evidence, recomputed from the inventory and
-- the active profile; holds no file bytes. The richer `contexts` /
-- `context_memberships` graph of §10.1 arrives with entity anchors later.
CREATE TABLE folder_contexts (
    folder_id             TEXT PRIMARY KEY REFERENCES folders(id),
    snapshot_id           TEXT NOT NULL REFERENCES snapshots(id),
    relative_path         TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    is_protected_boundary INTEGER NOT NULL CHECK (is_protected_boundary IN (0, 1)),
    penalty               INTEGER NOT NULL CHECK (penalty >= 0),
    marker                TEXT,
    created_at            TEXT NOT NULL
) STRICT;

CREATE INDEX idx_folder_contexts_snapshot ON folder_contexts(snapshot_id);
CREATE INDEX idx_folder_contexts_kind ON folder_contexts(snapshot_id, kind);
