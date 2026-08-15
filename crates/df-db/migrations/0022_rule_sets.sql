-- Persisted rule sets: the tunable half of the deterministic gate (ADR-0041).
--
-- The hard boundaries live in Rust because a boundary the caller can edit is
-- not a boundary. The weights — which copy wins, how much a generic container
-- is penalised, what margin an auto-approval demands — depend on the corpus,
-- so they live here, versioned and with a digest, under the same discipline as
-- the migrations themselves.
--
-- **Append-only, and that is the whole point.** A stored version is immutable:
-- every decision the gate ever recorded names the set that produced it, so
-- editing one in place would silently rewrite what a past decision meant. A
-- change is a new version, never an update, which is why there is no path in
-- the API that overwrites `params` for an existing `(id, version)`.
--
-- `params` is stored as the canonical JSON the digest was taken over, not as
-- columns. Columns would mean this table has to change shape every time a
-- weight is added — and a schema migration to add a tunable knob is exactly
-- the friction that gets a knob hardcoded instead.
CREATE TABLE rule_sets (
    id         TEXT NOT NULL,
    version    INTEGER NOT NULL CHECK (version > 0),
    -- Contract version of the params shape, so a set written by an older
    -- build is recognisable rather than merely unparseable.
    schema     TEXT NOT NULL,
    -- Canonical JSON, exactly the bytes the digest covers.
    params     TEXT NOT NULL,
    digest     TEXT NOT NULL CHECK (length(digest) = 64),
    created_at TEXT NOT NULL,
    PRIMARY KEY (id, version)
) STRICT;
