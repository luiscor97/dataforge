-- Migration 0025 — hash exclusions an operator declares for one project.
--
-- Migration 0024 taught a *profile* to decline reading material. Profiles are
-- compiled into the binary (`include_str!`, ADR-0026), so that feature could
-- only ever be used by whoever builds DataForge — not by the person with the
-- archive. This is the missing half.
--
-- Declared once, when the project is created, and stored here. The operator
-- points at a JSON file with `--exclusions`; after that the file is
-- irrelevant, because the project owns its rules. That is deliberate and it
-- is what keeps ADR-0026's property intact: behaviour is not determined by a
-- file that might drift, be edited by something else, or disappear. Passing a
-- path once is an act; reading a path on every run is a dependency.
--
-- Set at creation and not edited afterwards, exactly like source roots. A
-- project whose exclusions changed halfway would have a snapshot that no
-- single set of rules explains — and every exclusion already carries the rule
-- text into `hash_exclusions`, so the evidence survives regardless.
CREATE TABLE project_hash_exclusions (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id         TEXT NOT NULL,
    -- Why, in the operator's words. Copied into every exclusion it causes.
    reason     TEXT NOT NULL CHECK (length(reason) > 0),
    -- The matcher, as canonical JSON: path_glob, file_name_glob,
    -- min_size_bytes. Stored whole rather than as columns so that adding a
    -- criterion is not a schema migration — the friction that gets a knob
    -- hardcoded instead.
    match_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
) STRICT;

CREATE TRIGGER project_hash_exclusions_no_update
BEFORE UPDATE ON project_hash_exclusions
BEGIN
    SELECT RAISE(ABORT, 'a project declares its exclusions once, at creation');
END;
