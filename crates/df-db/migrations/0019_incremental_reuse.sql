-- Migration 0019 — incremental snapshot reuse provenance (Milestone 0.8).
--
-- When a rescan proves a file physically identical to the previous
-- snapshot (byte-equal v2 fingerprint with every field present: size,
-- mtime, ctime, attributes, volume and file id), its content binding may
-- be carried forward instead of re-reading the bytes. The carried binding
-- records exactly which snapshot it came from, so reused identity is
-- always distinguishable from freshly hashed identity.

ALTER TABLE occurrence_content
    ADD COLUMN reused_from_snapshot TEXT REFERENCES snapshots(id);
