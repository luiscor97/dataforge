-- Migration 0020 — routing provenance (Milestone 2.2, ADR-0040 §3).
--
-- Until now the only record of why an operation landed where it did was the
-- free-text `reason`, which said things like "routed to operational bucket
-- `90_DataForge_Review`". That is readable and unqueryable: reconstructing how
-- a destination was chosen meant parsing prose, and a prose format nobody
-- declared is one that drifts.
--
-- `destination_root_id` records the declared root the planner chose, by its
-- stable id rather than its folder name, so a profile that renames a folder
-- does not rewrite the provenance of plans made before the rename. A pretty
-- output whose provenance cannot be reconstructed is no use in an evidential
-- archive.
--
-- Nullable on purpose, and it must stay that way: operations planned before
-- this migration have no recorded root, and inventing one for them would be
-- fabricating provenance. `NULL` means "not recorded", never "the active
-- root". Operations that copy nothing (SKIP_REPRESENTED) legitimately have no
-- destination and therefore no root either.

ALTER TABLE plan_operations
    ADD COLUMN destination_root_id TEXT;
