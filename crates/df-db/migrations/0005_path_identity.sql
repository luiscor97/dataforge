-- Migration 0005 — exact path identity (v0.1.1-dev hardening).
--
-- RFC-0001 §13.4 asks for the raw path to be kept alongside the display and
-- comparison forms. Until now only the lossy display form existed (plus a
-- `name_is_lossy` flag that recorded the damage without recording the
-- original), so a file whose name is not valid UTF-16 — an unpaired
-- surrogate is a legal Windows filename — could be inventoried and then never
-- reopened, or worse, a *different* file opened in its place (threat T9).
--
-- Strategy (ADR-0020), one and only one: the exact UTF-16 code units, stored
-- little-endian in a BLOB. Where a blob cannot travel (the manifest's
-- canonical JSON) the same bytes are rendered as lowercase hex.
--
-- Nullable because snapshots taken before v0.1.1 have no raw form; the code
-- treats NULL as "display only, degraded" rather than inventing one.
ALTER TABLE path_occurrences ADD COLUMN raw_relative_path BLOB;

ALTER TABLE folders ADD COLUMN raw_relative_path BLOB;

-- The approved manifest must carry the raw path too (§P0-5): the executor
-- reconstructs the source from here, never from the display string. The
-- column is covered by the approval hash, so it cannot be edited unnoticed.
ALTER TABLE execution_manifest ADD COLUMN source_raw_relative_path BLOB;
