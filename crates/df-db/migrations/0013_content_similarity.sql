-- Migration 0013 — streaming content similarity (Milestone 0.3).
--
-- Exact SHA-256 identity remains authoritative. Chunks and MinHash signatures
-- are immutable, reusable evidence; candidates and relationships belong to a
-- configuration-addressed run and are sealed when that run completes.

CREATE TABLE similarity_runs (
    id                    TEXT PRIMARY KEY,
    project_id            TEXT NOT NULL REFERENCES projects(id),
    snapshot_id           TEXT NOT NULL REFERENCES snapshots(id),
    status                TEXT NOT NULL CHECK (status IN ('RUNNING', 'COMPLETED', 'FAILED')),
    algorithm_version     TEXT NOT NULL CHECK (length(algorithm_version) > 0),
    config_digest         TEXT NOT NULL CHECK (length(config_digest) = 64),
    config_json           TEXT NOT NULL CHECK (json_valid(config_json)),
    min_chunk_bytes       INTEGER NOT NULL CHECK (min_chunk_bytes >= 64),
    avg_chunk_bytes       INTEGER NOT NULL CHECK (avg_chunk_bytes >= min_chunk_bytes),
    max_chunk_bytes       INTEGER NOT NULL CHECK (max_chunk_bytes >= avg_chunk_bytes),
    min_file_bytes        INTEGER NOT NULL CHECK (min_file_bytes >= 1),
    threshold             REAL NOT NULL CHECK (threshold >= 0.0 AND threshold <= 1.0),
    min_shared_chunks     INTEGER NOT NULL CHECK (min_shared_chunks >= 1),
    min_shared_bytes      INTEGER NOT NULL CHECK (min_shared_bytes >= 1),
    minhash_permutations  INTEGER NOT NULL CHECK (minhash_permutations >= 16),
    lsh_bands             INTEGER NOT NULL CHECK (lsh_bands >= 1),
    max_bucket_contents   INTEGER NOT NULL CHECK (max_bucket_contents >= 2),
    max_candidates        INTEGER NOT NULL CHECK (max_candidates >= 1),
    contents_total        INTEGER NOT NULL DEFAULT 0 CHECK (contents_total >= 0),
    contents_chunked      INTEGER NOT NULL DEFAULT 0 CHECK (contents_chunked >= 0),
    contents_skipped      INTEGER NOT NULL DEFAULT 0 CHECK (contents_skipped >= 0),
    chunks_total          INTEGER NOT NULL DEFAULT 0 CHECK (chunks_total >= 0),
    candidates_total      INTEGER NOT NULL DEFAULT 0 CHECK (candidates_total >= 0),
    relations_total       INTEGER NOT NULL DEFAULT 0 CHECK (relations_total >= 0),
    candidate_cap_reached INTEGER NOT NULL DEFAULT 0 CHECK (candidate_cap_reached IN (0, 1)),
    error                 TEXT,
    started_at            TEXT NOT NULL,
    finished_at           TEXT,
    created_at            TEXT NOT NULL,
    UNIQUE (snapshot_id, config_digest),
    CHECK (minhash_permutations % lsh_bands = 0),
    CHECK (
        (status = 'RUNNING' AND finished_at IS NULL AND error IS NULL)
        OR (status = 'COMPLETED' AND finished_at IS NOT NULL AND error IS NULL)
        OR (status = 'FAILED' AND finished_at IS NOT NULL AND error IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_similarity_runs_project_snapshot
    ON similarity_runs(project_id, snapshot_id, status);

-- Normalized chunk content: one row per BLAKE3+length under an algorithm
-- contract. The bytes themselves are deliberately never stored.
CREATE TABLE chunks (
    id                TEXT PRIMARY KEY,
    algorithm_version TEXT NOT NULL,
    blake3            TEXT NOT NULL CHECK (length(blake3) = 64),
    length_bytes      INTEGER NOT NULL CHECK (length_bytes > 0),
    created_at        TEXT NOT NULL,
    UNIQUE (algorithm_version, blake3, length_bytes),
    UNIQUE (id, algorithm_version)
) STRICT;

CREATE INDEX idx_chunks_digest
    ON chunks(algorithm_version, blake3, length_bytes);

-- Ordered, multiset-preserving membership. One content is committed
-- atomically by the repository, so interrupted reads leave no partial list.
CREATE TABLE chunk_memberships (
    content_id        TEXT NOT NULL REFERENCES content_objects(id),
    algorithm_version TEXT NOT NULL,
    ordinal           INTEGER NOT NULL CHECK (ordinal >= 0),
    offset_bytes      INTEGER NOT NULL CHECK (offset_bytes >= 0),
    chunk_id          TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    PRIMARY KEY (content_id, algorithm_version, ordinal),
    UNIQUE (content_id, algorithm_version, offset_bytes),
    FOREIGN KEY (chunk_id, algorithm_version)
        REFERENCES chunks(id, algorithm_version)
) STRICT;

CREATE INDEX idx_chunk_memberships_chunk
    ON chunk_memberships(algorithm_version, chunk_id, content_id);

-- Presence of this row is the completion marker for a content. `signature`
-- is minhash_permutations little-endian u64 values.
CREATE TABLE content_minhash (
    content_id           TEXT NOT NULL REFERENCES content_objects(id),
    algorithm_version    TEXT NOT NULL,
    signature            BLOB NOT NULL,
    permutations         INTEGER NOT NULL CHECK (permutations >= 16),
    total_chunks         INTEGER NOT NULL CHECK (total_chunks >= 1),
    total_bytes          INTEGER NOT NULL CHECK (total_bytes >= 1),
    source_sha256        TEXT NOT NULL CHECK (length(source_sha256) = 64),
    created_at           TEXT NOT NULL,
    PRIMARY KEY (content_id, algorithm_version),
    CHECK (length(signature) = permutations * 8)
) STRICT;

-- Precomputed LSH buckets keep candidate discovery inside SQLite instead of
-- loading every signature into memory. `band_hash` is a 64-character BLAKE3
-- digest of the ordered u64 values in that band.
CREATE TABLE content_lsh_bands (
    content_id        TEXT NOT NULL REFERENCES content_objects(id),
    algorithm_version TEXT NOT NULL,
    band_index        INTEGER NOT NULL CHECK (band_index >= 0),
    band_hash         TEXT NOT NULL CHECK (length(band_hash) = 64),
    created_at        TEXT NOT NULL,
    PRIMARY KEY (content_id, algorithm_version, band_index)
) STRICT;

CREATE INDEX idx_content_lsh_bucket
    ON content_lsh_bands(algorithm_version, band_index, band_hash, content_id);

CREATE TABLE similarity_candidates (
    run_id                TEXT NOT NULL REFERENCES similarity_runs(id),
    content_a             TEXT NOT NULL REFERENCES content_objects(id),
    content_b             TEXT NOT NULL REFERENCES content_objects(id),
    shared_bands          INTEGER NOT NULL DEFAULT 0 CHECK (shared_bands >= 0),
    rare_chunk_hits       INTEGER NOT NULL DEFAULT 0 CHECK (rare_chunk_hits >= 0),
    estimated_similarity REAL NOT NULL CHECK (estimated_similarity >= 0.0 AND estimated_similarity <= 1.0),
    exact_similarity     REAL CHECK (exact_similarity IS NULL OR (exact_similarity >= 0.0 AND exact_similarity <= 1.0)),
    shared_chunks         INTEGER CHECK (shared_chunks IS NULL OR shared_chunks >= 0),
    shared_bytes          INTEGER CHECK (shared_bytes IS NULL OR shared_bytes >= 0),
    union_bytes           INTEGER CHECK (union_bytes IS NULL OR union_bytes >= 0),
    status                TEXT NOT NULL CHECK (status IN ('PENDING', 'EVALUATED')),
    created_at            TEXT NOT NULL,
    PRIMARY KEY (run_id, content_a, content_b),
    CHECK (content_a < content_b),
    CHECK (
        (status = 'PENDING' AND exact_similarity IS NULL AND shared_chunks IS NULL AND shared_bytes IS NULL AND union_bytes IS NULL)
        OR
        (status = 'EVALUATED' AND exact_similarity IS NOT NULL AND shared_chunks IS NOT NULL AND shared_bytes IS NOT NULL AND union_bytes IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_similarity_candidates_status
    ON similarity_candidates(run_id, status, content_a, content_b);

CREATE TABLE content_relationships (
    id                   TEXT PRIMARY KEY,
    run_id               TEXT NOT NULL REFERENCES similarity_runs(id),
    snapshot_id          TEXT NOT NULL REFERENCES snapshots(id),
    content_a            TEXT NOT NULL REFERENCES content_objects(id),
    content_b            TEXT NOT NULL REFERENCES content_objects(id),
    kind                 TEXT NOT NULL CHECK (kind IN ('LIKELY_VERSION', 'TRUNCATED_VARIANT', 'RECOMPOSED_CONTENT', 'SIMILAR_CONTENT')),
    direction            TEXT NOT NULL CHECK (direction IN ('A_TO_B', 'B_TO_A', 'UNKNOWN')),
    similarity           REAL NOT NULL CHECK (similarity >= 0.0 AND similarity <= 1.0),
    shared_chunks        INTEGER NOT NULL CHECK (shared_chunks >= 1),
    shared_bytes         INTEGER NOT NULL CHECK (shared_bytes >= 1),
    union_bytes          INTEGER NOT NULL CHECK (union_bytes >= shared_bytes),
    estimated_similarity REAL NOT NULL CHECK (estimated_similarity >= 0.0 AND estimated_similarity <= 1.0),
    confidence           REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    evidence_json        TEXT NOT NULL CHECK (json_valid(evidence_json)),
    created_at           TEXT NOT NULL,
    UNIQUE (run_id, content_a, content_b),
    CHECK (content_a < content_b)
) STRICT;

CREATE INDEX idx_content_relationships_snapshot
    ON content_relationships(snapshot_id, similarity DESC, content_a, content_b);

-- Reusable chunk evidence is append-only. A changed algorithm gets a new
-- algorithm_version; an old interpretation is never silently rewritten.
CREATE TRIGGER chunks_no_update BEFORE UPDATE ON chunks
BEGIN
    SELECT RAISE(ABORT, 'chunks are append-only');
END;
CREATE TRIGGER chunks_no_delete BEFORE DELETE ON chunks
BEGIN
    SELECT RAISE(ABORT, 'chunks are append-only');
END;
CREATE TRIGGER chunk_memberships_no_update BEFORE UPDATE ON chunk_memberships
BEGIN
    SELECT RAISE(ABORT, 'chunk memberships are append-only');
END;
CREATE TRIGGER chunk_memberships_no_delete BEFORE DELETE ON chunk_memberships
BEGIN
    SELECT RAISE(ABORT, 'chunk memberships are append-only');
END;
CREATE TRIGGER content_minhash_no_update BEFORE UPDATE ON content_minhash
BEGIN
    SELECT RAISE(ABORT, 'content minhash is append-only');
END;
CREATE TRIGGER content_minhash_validate_insert
BEFORE INSERT ON content_minhash
WHEN NOT EXISTS (
    SELECT 1 FROM content_objects c
    WHERE c.id = NEW.content_id AND c.hash_state = 'HASHED'
      AND c.size_bytes = NEW.total_bytes AND c.sha256 = NEW.source_sha256
)
BEGIN
    SELECT RAISE(ABORT, 'content minhash does not match canonical content');
END;
CREATE TRIGGER content_minhash_no_delete BEFORE DELETE ON content_minhash
BEGIN
    SELECT RAISE(ABORT, 'content minhash is append-only');
END;
CREATE TRIGGER content_lsh_bands_no_update BEFORE UPDATE ON content_lsh_bands
BEGIN
    SELECT RAISE(ABORT, 'content LSH bands are append-only');
END;
CREATE TRIGGER content_lsh_bands_validate_insert
BEFORE INSERT ON content_lsh_bands
WHEN NOT EXISTS (
    SELECT 1 FROM content_minhash m
    WHERE m.content_id = NEW.content_id
      AND m.algorithm_version = NEW.algorithm_version
)
BEGIN
    SELECT RAISE(ABORT, 'content LSH band has no completed minhash');
END;
CREATE TRIGGER content_lsh_bands_no_delete BEFORE DELETE ON content_lsh_bands
BEGIN
    SELECT RAISE(ABORT, 'content LSH bands are append-only');
END;

-- A run may only move once out of RUNNING, and its identity/configuration is
-- immutable even before completion.
CREATE TRIGGER similarity_runs_guard_update
BEFORE UPDATE ON similarity_runs
WHEN OLD.status <> 'RUNNING'
  OR NEW.status = 'RUNNING'
  OR NEW.id <> OLD.id
  OR NEW.project_id <> OLD.project_id
  OR NEW.snapshot_id <> OLD.snapshot_id
  OR NEW.algorithm_version <> OLD.algorithm_version
  OR NEW.config_digest <> OLD.config_digest
  OR NEW.config_json <> OLD.config_json
  OR NEW.min_chunk_bytes <> OLD.min_chunk_bytes
  OR NEW.avg_chunk_bytes <> OLD.avg_chunk_bytes
  OR NEW.max_chunk_bytes <> OLD.max_chunk_bytes
  OR NEW.min_file_bytes <> OLD.min_file_bytes
  OR NEW.threshold <> OLD.threshold
  OR NEW.min_shared_chunks <> OLD.min_shared_chunks
  OR NEW.min_shared_bytes <> OLD.min_shared_bytes
  OR NEW.minhash_permutations <> OLD.minhash_permutations
  OR NEW.lsh_bands <> OLD.lsh_bands
  OR NEW.max_bucket_contents <> OLD.max_bucket_contents
  OR NEW.max_candidates <> OLD.max_candidates
  OR NEW.started_at <> OLD.started_at
  OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'similarity run identity is immutable or already sealed');
END;

CREATE TRIGGER similarity_runs_validate_completion
BEFORE UPDATE ON similarity_runs
WHEN NEW.status = 'COMPLETED' AND (
       EXISTS (
           SELECT 1 FROM similarity_candidates c
           WHERE c.run_id = OLD.id AND c.status <> 'EVALUATED'
       )
    OR NEW.candidates_total <> (
           SELECT COUNT(*) FROM similarity_candidates c WHERE c.run_id = OLD.id
       )
    OR NEW.relations_total <> (
           SELECT COUNT(*) FROM content_relationships r WHERE r.run_id = OLD.id
       )
)
BEGIN
    SELECT RAISE(ABORT, 'similarity run completion summary does not match evidence');
END;

CREATE TRIGGER similarity_runs_no_delete BEFORE DELETE ON similarity_runs
BEGIN
    SELECT RAISE(ABORT, 'similarity runs are append-only');
END;

CREATE TRIGGER similarity_candidates_guard_insert
BEFORE INSERT ON similarity_candidates
WHEN NOT EXISTS (
    SELECT 1 FROM similarity_runs r
    WHERE r.id = NEW.run_id AND r.status = 'RUNNING'
      AND EXISTS (
          SELECT 1 FROM content_minhash m
          WHERE m.content_id = NEW.content_a
            AND m.algorithm_version = r.algorithm_version
      )
      AND EXISTS (
          SELECT 1 FROM content_minhash m
          WHERE m.content_id = NEW.content_b
            AND m.algorithm_version = r.algorithm_version
      )
      AND EXISTS (
          SELECT 1 FROM occurrence_content oc
          JOIN path_occurrences o ON o.id = oc.occurrence_id
          WHERE oc.content_id = NEW.content_a AND o.snapshot_id = r.snapshot_id
      )
      AND EXISTS (
          SELECT 1 FROM occurrence_content oc
          JOIN path_occurrences o ON o.id = oc.occurrence_id
          WHERE oc.content_id = NEW.content_b AND o.snapshot_id = r.snapshot_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'similarity candidate is outside its running evidence scope');
END;
CREATE TRIGGER similarity_candidates_guard_update
BEFORE UPDATE ON similarity_candidates
WHEN OLD.run_id <> NEW.run_id
  OR OLD.content_a <> NEW.content_a
  OR OLD.content_b <> NEW.content_b
  OR NOT EXISTS (
    SELECT 1 FROM similarity_runs r WHERE r.id = OLD.run_id AND r.status = 'RUNNING'
)
BEGIN
    SELECT RAISE(ABORT, 'similarity candidate run is sealed');
END;
CREATE TRIGGER similarity_candidates_guard_delete
BEFORE DELETE ON similarity_candidates
WHEN NOT EXISTS (
    SELECT 1 FROM similarity_runs r WHERE r.id = OLD.run_id AND r.status = 'RUNNING'
)
BEGIN
    SELECT RAISE(ABORT, 'similarity candidate run is sealed');
END;

CREATE TRIGGER content_relationships_guard_insert
BEFORE INSERT ON content_relationships
WHEN NOT EXISTS (
    SELECT 1 FROM similarity_runs r
    JOIN similarity_candidates c
      ON c.run_id = r.id
     AND c.content_a = NEW.content_a AND c.content_b = NEW.content_b
    WHERE r.id = NEW.run_id AND r.status = 'RUNNING'
      AND r.snapshot_id = NEW.snapshot_id AND c.status = 'EVALUATED'
      AND c.exact_similarity = NEW.similarity
      AND c.shared_chunks = NEW.shared_chunks
      AND c.shared_bytes = NEW.shared_bytes
      AND c.union_bytes = NEW.union_bytes
)
BEGIN
    SELECT RAISE(ABORT, 'content relationship does not match evaluated evidence');
END;
CREATE TRIGGER content_relationships_guard_update
BEFORE UPDATE ON content_relationships
BEGIN
    SELECT RAISE(ABORT, 'content relationships are immutable');
END;
CREATE TRIGGER content_relationships_guard_delete
BEFORE DELETE ON content_relationships
WHEN NOT EXISTS (
    SELECT 1 FROM similarity_runs r WHERE r.id = OLD.run_id AND r.status = 'RUNNING'
)
BEGIN
    SELECT RAISE(ABORT, 'content relationship run is sealed');
END;
