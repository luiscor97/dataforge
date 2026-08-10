-- Who is running a long stage on this project, right now.
--
-- A long stage — hashing 443 GB, copying a plan — occupies a process for
-- hours and leaves no trace of itself. A second process opening the same
-- project cannot tell "another run is working" from "a run died three days
-- ago", and neither can a person. The engine's existing answer is to refuse
-- and make the operator assert that no run is active; this table is what lets
-- that assertion be informed instead of blind.
--
-- It records **evidence, never a verdict**. There is no `alive` column,
-- because whether a run is alive is not a fact this database can hold: a pid
-- from another host means nothing locally, a pid can be reused, and a laptop
-- that slept for a day has an old heartbeat and a perfectly live run. What is
-- recorded is who claimed the stage, from where, and when it last said
-- anything — and reading that is what a caller decides on.
--
-- One row per project. Only one long stage runs at a time on a project (the
-- state machine allows no other shape), so a primary key on the project is
-- the constraint, not an index choice.
CREATE TABLE run_liveness (
    project_id   TEXT PRIMARY KEY REFERENCES projects(id),
    -- SCAN | HASH | ANALYZE | EXECUTE | VERIFY.
    stage        TEXT NOT NULL,
    -- Operating-system process id, and the host it means something on.
    -- Without the host a pid is not an identity: pid 4242 exists on every
    -- machine, and comparing one across machines invents a coincidence.
    pid          INTEGER NOT NULL CHECK (pid > 0),
    host         TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    -- Refreshed as the stage makes progress. Its *age* is the evidence; the
    -- meaning of that age is the reader's to decide.
    heartbeat_at TEXT NOT NULL
) STRICT;
