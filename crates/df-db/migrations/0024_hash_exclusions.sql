-- Migration 0024 — occurrences the profile declined to hash (M2.6).
--
-- Reading 443 GB of video to prove it is video costs hours and answers
-- nothing, so a profile may name material the identity pipeline skips. This
-- table is what keeps that from being a silent omission.
--
-- **Excluded is not discarded.** The occurrence stays in `path_occurrences`
-- exactly as scanned: the inventory still describes the origin completely,
-- which is what RFC-0001's coverage criterion asks for — nothing left without
-- representation *or reason*. This table is the reason.
--
-- The rule id and its reason are copied in rather than referenced, because a
-- profile is edited and re-released while a snapshot is evidence. A year from
-- now "why was this never read?" has to answer with the words that were in
-- force at the time, not with whatever that rule id means today.
CREATE TABLE hash_exclusions (
    occurrence_id TEXT PRIMARY KEY REFERENCES path_occurrences(id),
    snapshot_id   TEXT NOT NULL REFERENCES snapshots(id),
    -- The profile rule that matched, and its wording at the time.
    rule_id       TEXT NOT NULL,
    reason        TEXT NOT NULL,
    created_at    TEXT NOT NULL
) STRICT;

CREATE INDEX idx_hash_exclusions_snapshot ON hash_exclusions(snapshot_id);

CREATE TRIGGER hash_exclusions_no_update BEFORE UPDATE ON hash_exclusions
BEGIN
    SELECT RAISE(ABORT, 'a recorded exclusion is evidence; re-scan to change it');
END;
