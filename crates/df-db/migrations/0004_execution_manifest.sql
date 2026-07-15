-- Migration 0004 — immutable execution manifest (v0.1.1-dev hardening).
--
-- RFC-0001 rule 10: "toda ejecución parte de un plan aprobado e inmutable".
-- Until now that was only half true. The approved plan's SHA-256 covered the
-- sequence, type, occurrence id, content id, destination and idempotency key —
-- but the executor resolved *what to read* and *what content to expect* at run
-- time, with live joins against path_occurrences, source_roots and
-- content_objects. Editing content_objects.sha256 or a source root's path
-- after approval therefore changed the material executed **without changing
-- the plan hash**. Approval did not bind what mattered (threat T5, ADR-0018).
--
-- This table freezes the whole execution contract at approval time. Its rows
-- are the only thing the executor may read: the inventory tables become
-- evidence for consistency checks, not a mutable contract.
--
-- Immutability is enforced by the database, not by convention: the triggers
-- below reject every UPDATE and DELETE on the table. A new plan version
-- produces new operations, hence new manifest rows, so nothing legitimate ever
-- needs to mutate one.
CREATE TABLE execution_manifest (
    operation_id               TEXT PRIMARY KEY REFERENCES plan_operations(id),
    plan_id                    TEXT NOT NULL REFERENCES plans(id),
    sequence                   INTEGER NOT NULL CHECK (sequence >= 0),
    operation_type             TEXT NOT NULL,
    idempotency_key            TEXT NOT NULL,

    -- What will be read (NULL for operations with no source, e.g. a directory).
    source_root_id             TEXT REFERENCES source_roots(id),
    -- Physical identity of the source root at approval time, when the
    -- filesystem provides one ("volume:index"); NULL means degraded identity
    -- and must not be presented as strong evidence (ADR-0019).
    source_root_identity       TEXT,
    -- The root's path as it was at approval; the live row may drift.
    source_root_path_snapshot  TEXT,
    source_relative_path_exact TEXT,
    source_fingerprint         TEXT,

    -- What content is expected.
    expected_size_bytes        INTEGER CHECK (expected_size_bytes IS NULL OR expected_size_bytes >= 0),
    expected_sha256            TEXT CHECK (expected_sha256 IS NULL OR length(expected_sha256) = 64),
    expected_blake3            TEXT CHECK (expected_blake3 IS NULL OR length(expected_blake3) = 64),

    -- Where it will be written.
    destination_relative_path  TEXT,

    created_at                 TEXT NOT NULL
) STRICT;

CREATE INDEX idx_execution_manifest_plan ON execution_manifest(plan_id, sequence);

-- The manifest is frozen the moment it exists: it is only ever written once,
-- inside the approval transaction.
CREATE TRIGGER execution_manifest_no_update
BEFORE UPDATE ON execution_manifest
BEGIN
    SELECT RAISE(ABORT, 'the execution manifest is immutable once approved');
END;

CREATE TRIGGER execution_manifest_no_delete
BEFORE DELETE ON execution_manifest
BEGIN
    SELECT RAISE(ABORT, 'the execution manifest is immutable once approved');
END;
