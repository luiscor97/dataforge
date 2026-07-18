-- Migration 0015 — hash queue batch index (performance only).
--
-- `pending_hash_jobs` filters on (snapshot_id, status) and orders by
-- (created_at, id). The 0002 index covered only the filter, so every batch
-- fetch re-sorted all remaining PENDING rows — quadratic work across a
-- large run. This index serves the filter and the order together; the old
-- index is dropped because the new one fully covers its prefix.

CREATE INDEX idx_hash_jobs_batch
    ON hash_jobs(snapshot_id, status, created_at, id);

DROP INDEX idx_hash_jobs_pending;
