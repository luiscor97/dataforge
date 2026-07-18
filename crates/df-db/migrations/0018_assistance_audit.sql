-- Migration 0018 — assisted-intelligence audit trail (Milestone 0.7).
--
-- Every assistance invocation leaves one immutable row: what was disclosed
-- (by digest), to which provider, under which prompt version, and how it
-- ended. The consent token itself is never stored — consent is provable
-- only as "this exact disclosure digest was accepted", never replayable.

CREATE TABLE assistance_audits (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES projects(id),
    request_id_sha256 TEXT NOT NULL CHECK (length(request_id_sha256) = 64),
    purpose           TEXT NOT NULL,
    provider_kind     TEXT NOT NULL CHECK (provider_kind IN ('LOCAL_PROCESS', 'CLOUD')),
    provider          TEXT NOT NULL,
    model             TEXT NOT NULL,
    endpoint          TEXT NOT NULL,
    status            TEXT NOT NULL,
    failure           TEXT,
    disclosure_sha256 TEXT NOT NULL CHECK (length(disclosure_sha256) = 64),
    prompt_sha256     TEXT NOT NULL CHECK (length(prompt_sha256) = 64),
    audit_json        TEXT NOT NULL,
    created_at        TEXT NOT NULL
) STRICT;

CREATE INDEX idx_assistance_audits_project
    ON assistance_audits(project_id, created_at);

CREATE TRIGGER assistance_audits_no_update BEFORE UPDATE ON assistance_audits
BEGIN
    SELECT RAISE(ABORT, 'assistance audits are append-only');
END;

CREATE TRIGGER assistance_audits_no_delete BEFORE DELETE ON assistance_audits
BEGIN
    SELECT RAISE(ABORT, 'assistance audits are append-only');
END;
