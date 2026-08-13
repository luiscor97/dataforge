-- Migration 0023 — consent by policy, and what it has spent (ADR-0042, M2.5).
--
-- ADR-0034 asks for human approval per request, which is right when somebody
-- is sitting there and impossible in an autonomous run: a queue of 5.334
-- items is not approved prompt by prompt. ADR-0042 replaces that with one
-- approval of a disclosure policy carrying a budget. These two tables are
-- where that approval and its spending live.
--
-- **Consumption is a ledger, never a counter.**
--
-- There is no `spent_cents` column on the policy that gets incremented. What
-- is spent is the SUM over `disclosure_charges`, and the difference matters:
-- a stored total is one UPDATE away from a budget that never ran out, while a
-- ledger can only be rewound by deleting rows that the triggers below refuse
-- to delete. The number an operator is protected by should be reconstructible
-- from evidence, not asserted.
--
-- A charge names the policy **digest**, not its id and version. The terms a
-- disclosure happened under are the exact bytes that were approved, so
-- superseding a policy leaves old charges pointing at what was actually
-- agreed rather than at whatever the name means now.

CREATE TABLE disclosure_policies (
    project_id  TEXT NOT NULL REFERENCES projects(id),
    id          TEXT NOT NULL,
    version     INTEGER NOT NULL CHECK (version > 0),
    -- Contract version of the policy shape, so one written by an older build
    -- is recognisable rather than merely unparseable.
    schema      TEXT NOT NULL,
    -- Canonical JSON: exactly the bytes the digest covers.
    policy      TEXT NOT NULL,
    digest      TEXT NOT NULL CHECK (length(digest) = 64),
    approved_at TEXT NOT NULL,
    -- Who approved it. A policy is a human act; recording the actor is what
    -- separates "a person agreed to this" from "a process wrote a row".
    approved_by TEXT NOT NULL,
    PRIMARY KEY (project_id, id, version)
) STRICT;

CREATE INDEX idx_disclosure_policies_digest ON disclosure_policies(digest);

CREATE TRIGGER disclosure_policies_no_update BEFORE UPDATE ON disclosure_policies
BEGIN
    SELECT RAISE(ABORT, 'an approved disclosure policy is immutable; approve a new version');
END;

CREATE TRIGGER disclosure_policies_no_delete BEFORE DELETE ON disclosure_policies
BEGIN
    SELECT RAISE(ABORT, 'an approved disclosure policy is immutable; approve a new version');
END;

-- One row per invocation charged against a policy.
CREATE TABLE disclosure_charges (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id),
    -- The exact terms this disclosure happened under.
    policy_digest   TEXT NOT NULL CHECK (length(policy_digest) = 64),
    -- Bytes that actually left after redaction, not bytes considered.
    disclosed_bytes INTEGER NOT NULL CHECK (disclosed_bytes >= 0),
    spend_cents     INTEGER NOT NULL CHECK (spend_cents >= 0),
    created_at      TEXT NOT NULL
) STRICT;

CREATE INDEX idx_disclosure_charges_policy ON disclosure_charges(policy_digest);

CREATE TRIGGER disclosure_charges_no_update BEFORE UPDATE ON disclosure_charges
BEGIN
    SELECT RAISE(ABORT, 'disclosure charges are append-only');
END;

CREATE TRIGGER disclosure_charges_no_delete BEFORE DELETE ON disclosure_charges
BEGIN
    SELECT RAISE(ABORT, 'disclosure charges are append-only');
END;
