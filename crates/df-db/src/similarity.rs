//! Persistence boundary for content-defined chunks and similarity evidence.
//!
//! This module deliberately keeps candidate generation in SQLite and exposes
//! an atomic streaming writer for one content. The algorithm crate never
//! obtains a raw connection and never has to retain an entire corpus in RAM.

use std::path::PathBuf;
use std::str::FromStr;

use df_domain::{
    Actor, ChunkId, ContentId, ContentRelationship, ProjectId, RawPath, RelationshipDirection,
    SimilarityRelationId, SimilarityRun, SimilarityRunCounters, SimilarityRunId,
    SimilarityRunStatus, SnapshotId, Timestamp,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::repository::{append_event, parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

pub const EVENT_SIMILARITY_STARTED: &str = "SIMILARITY_STARTED";
pub const EVENT_SIMILARITY_COMPLETED: &str = "SIMILARITY_COMPLETED";

/// Fully expanded, immutable identity of a similarity run.
#[derive(Debug, Clone)]
pub struct SimilarityRunSpec {
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub algorithm_version: String,
    pub config_digest: String,
    pub config_json: String,
    pub min_chunk_bytes: u32,
    pub avg_chunk_bytes: u32,
    pub max_chunk_bytes: u32,
    pub min_file_bytes: u64,
    pub threshold: f64,
    pub min_shared_chunks: u32,
    pub min_shared_bytes: u64,
    pub minhash_permutations: u32,
    pub lsh_bands: u32,
    pub max_bucket_contents: u32,
    pub max_candidates: u64,
}

struct StoredRunSpec {
    project_id: String,
    algorithm_version: String,
    config_json: String,
    min_chunk_bytes: i64,
    avg_chunk_bytes: i64,
    max_chunk_bytes: i64,
    min_file_bytes: i64,
    threshold: f64,
    min_shared_chunks: i64,
    min_shared_bytes: i64,
    minhash_permutations: i64,
    lsh_bands: i64,
    max_bucket_contents: i64,
    max_candidates: i64,
}

/// Stable representative used to reopen one unique content from the source.
#[derive(Debug, Clone)]
pub struct SimilarityContentSource {
    pub content_id: ContentId,
    pub size_bytes: u64,
    pub sha256: String,
    pub root_path: PathBuf,
    pub raw_relative_path: Option<RawPath>,
    pub relative_path: String,
    pub fingerprint: String,
    pub modified_at: Option<Timestamp>,
}

/// Bounded candidate page. Signature blobs contain little-endian u64 values.
#[derive(Debug, Clone)]
pub struct PendingSimilarityCandidate {
    pub content_a: ContentId,
    pub content_b: ContentId,
    pub size_a: u64,
    pub size_b: u64,
    pub modified_a: Option<Timestamp>,
    pub modified_b: Option<Timestamp>,
    pub signature_a: Vec<u8>,
    pub signature_b: Vec<u8>,
    pub shared_bands: u32,
    pub rare_chunk_hits: u32,
}

/// Exact multiset-weighted overlap for one candidate pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExactChunkOverlap {
    pub shared_chunks: u64,
    pub shared_bytes: u64,
    pub union_bytes: u64,
    pub similarity: f64,
}

type StoredRunRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    String,
    Option<String>,
);

fn run_from_stored(row: StoredRunRow) -> DfResult<SimilarityRun> {
    let (
        id,
        project,
        snapshot,
        status,
        algorithm_version,
        config_digest,
        config_json,
        contents_total,
        contents_chunked,
        contents_skipped,
        chunks_total,
        candidates_total,
        relations_total,
        candidate_cap_reached,
        error,
        started_at,
        finished_at,
    ) = row;
    Ok(SimilarityRun {
        id: SimilarityRunId::from_str(&id)?,
        project_id: ProjectId::from_str(&project)?,
        snapshot_id: SnapshotId::from_str(&snapshot)?,
        status: SimilarityRunStatus::parse(&status)?,
        algorithm_version,
        config_digest,
        config: serde_json::from_str(&config_json).map_err(|error| {
            DfError::Serialization(format!("stored similarity config: {error}"))
        })?,
        counters: SimilarityRunCounters {
            contents_total: contents_total as u64,
            contents_chunked: contents_chunked as u64,
            contents_skipped: contents_skipped as u64,
            chunks_total: chunks_total as u64,
            candidates_total: candidates_total as u64,
            relations_total: relations_total as u64,
        },
        candidate_cap_reached: candidate_cap_reached != 0,
        error,
        started_at: parse_stored_timestamp(&started_at)?,
        finished_at: finished_at
            .as_deref()
            .map(parse_stored_timestamp)
            .transpose()?,
    })
}

fn load_run_by_id(db: &Db, run_id: SimilarityRunId) -> DfResult<SimilarityRun> {
    let stored = db
        .conn()
        .query_row(
            "SELECT id, project_id, snapshot_id, status, algorithm_version,
                    config_digest, config_json, contents_total, contents_chunked,
                    contents_skipped, chunks_total, candidates_total,
                    relations_total, candidate_cap_reached, error, started_at,
                    finished_at
             FROM similarity_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| DfError::NotFound(format!("similarity run `{run_id}`")))?;
    run_from_stored(stored)
}

fn validate_spec(spec: &SimilarityRunSpec) -> DfResult<()> {
    if spec.algorithm_version.is_empty() || spec.config_digest.len() != 64 {
        return Err(DfError::Validation(
            "similarity algorithm and 64-character config digest are required".to_string(),
        ));
    }
    serde_json::from_str::<serde_json::Value>(&spec.config_json)
        .map_err(|error| DfError::Validation(format!("invalid similarity config JSON: {error}")))?;
    if !(spec.min_chunk_bytes <= spec.avg_chunk_bytes
        && spec.avg_chunk_bytes <= spec.max_chunk_bytes)
        || !(0.0..=1.0).contains(&spec.threshold)
        || spec.min_shared_chunks == 0
        || spec.min_shared_bytes == 0
        || spec.minhash_permutations < 16
        || spec.lsh_bands == 0
        || !spec.minhash_permutations.is_multiple_of(spec.lsh_bands)
        || spec.max_bucket_contents < 2
        || spec.max_candidates == 0
        || spec.min_file_bytes == 0
        || spec.min_file_bytes > i64::MAX as u64
        || spec.min_shared_bytes > i64::MAX as u64
        || spec.max_candidates >= i64::MAX as u64
    {
        return Err(DfError::Validation(
            "invalid similarity run configuration".to_string(),
        ));
    }
    Ok(())
}

fn ensure_run_matches_spec(
    db: &Db,
    run_id: SimilarityRunId,
    spec: &SimilarityRunSpec,
) -> DfResult<()> {
    let stored = db
        .conn()
        .query_row(
            "SELECT project_id, algorithm_version, config_json,
                    min_chunk_bytes, avg_chunk_bytes, max_chunk_bytes,
                    min_file_bytes, threshold, min_shared_chunks,
                    min_shared_bytes, minhash_permutations, lsh_bands,
                    max_bucket_contents, max_candidates
             FROM similarity_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| {
                Ok(StoredRunSpec {
                    project_id: row.get(0)?,
                    algorithm_version: row.get(1)?,
                    config_json: row.get(2)?,
                    min_chunk_bytes: row.get(3)?,
                    avg_chunk_bytes: row.get(4)?,
                    max_chunk_bytes: row.get(5)?,
                    min_file_bytes: row.get(6)?,
                    threshold: row.get(7)?,
                    min_shared_chunks: row.get(8)?,
                    min_shared_bytes: row.get(9)?,
                    minhash_permutations: row.get(10)?,
                    lsh_bands: row.get(11)?,
                    max_bucket_contents: row.get(12)?,
                    max_candidates: row.get(13)?,
                })
            },
        )
        .map_err(db_err)?;
    let matches = stored.project_id == spec.project_id.to_string()
        && stored.algorithm_version == spec.algorithm_version
        && stored.config_json == spec.config_json
        && stored.min_chunk_bytes == i64::from(spec.min_chunk_bytes)
        && stored.avg_chunk_bytes == i64::from(spec.avg_chunk_bytes)
        && stored.max_chunk_bytes == i64::from(spec.max_chunk_bytes)
        && stored.min_file_bytes == spec.min_file_bytes as i64
        && stored.threshold == spec.threshold
        && stored.min_shared_chunks == i64::from(spec.min_shared_chunks)
        && stored.min_shared_bytes == spec.min_shared_bytes as i64
        && stored.minhash_permutations == i64::from(spec.minhash_permutations)
        && stored.lsh_bands == i64::from(spec.lsh_bands)
        && stored.max_bucket_contents == i64::from(spec.max_bucket_contents)
        && stored.max_candidates == spec.max_candidates as i64;
    if !matches {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` does not match the configuration addressed by its digest"
        )));
    }
    Ok(())
}

/// Start a run or resume the identical configuration for the same completed
/// snapshot. A completed run is returned unchanged (idempotent replay).
pub fn start_or_resume_run(
    db: &mut Db,
    spec: &SimilarityRunSpec,
    actor: Actor,
) -> DfResult<SimilarityRun> {
    validate_spec(spec)?;
    let snapshot_ok: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM snapshots s
                JOIN analysis_completions a ON a.snapshot_id = s.id
                WHERE s.id = ?1 AND s.project_id = ?2 AND s.status = 'COMPLETE'
             )",
            params![spec.snapshot_id.to_string(), spec.project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !snapshot_ok {
        return Err(DfError::Validation(
            "similarity requires a completed, structurally analysed snapshot".to_string(),
        ));
    }

    let existing: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM similarity_runs
             WHERE snapshot_id = ?1 AND config_digest = ?2",
            params![spec.snapshot_id.to_string(), spec.config_digest],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if let Some(id) = existing {
        let run_id = SimilarityRunId::from_str(&id)?;
        ensure_run_matches_spec(db, run_id, spec)?;
        let run = load_run_by_id(db, run_id)?;
        if run.status == SimilarityRunStatus::Failed {
            return Err(DfError::Conflict(format!(
                "similarity run `{}` failed; use a new configuration digest",
                run.id
            )));
        }
        return Ok(run);
    }

    let contents_total: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(DISTINCT oc.content_id)
             FROM occurrence_content oc
             JOIN path_occurrences o ON o.id = oc.occurrence_id
             JOIN content_objects c ON c.id = oc.content_id
             WHERE o.snapshot_id = ?1 AND o.scan_status = 'OK'
               AND c.hash_state = 'HASHED' AND c.sha256 IS NOT NULL",
            [spec.snapshot_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let run_id = SimilarityRunId::new();
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO similarity_runs
            (id, project_id, snapshot_id, status, algorithm_version,
             config_digest, config_json, min_chunk_bytes, avg_chunk_bytes,
             max_chunk_bytes, min_file_bytes, threshold, min_shared_chunks,
             min_shared_bytes, minhash_permutations, lsh_bands,
             max_bucket_contents, max_candidates, contents_total, started_at,
             created_at)
         VALUES (?1, ?2, ?3, 'RUNNING', ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?19)",
        params![
            run_id.to_string(),
            spec.project_id.to_string(),
            spec.snapshot_id.to_string(),
            spec.algorithm_version,
            spec.config_digest,
            spec.config_json,
            spec.min_chunk_bytes as i64,
            spec.avg_chunk_bytes as i64,
            spec.max_chunk_bytes as i64,
            spec.min_file_bytes as i64,
            spec.threshold,
            spec.min_shared_chunks as i64,
            spec.min_shared_bytes as i64,
            spec.minhash_permutations as i64,
            spec.lsh_bands as i64,
            spec.max_bucket_contents as i64,
            spec.max_candidates as i64,
            contents_total,
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        spec.project_id,
        EVENT_SIMILARITY_STARTED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": spec.snapshot_id.to_string(),
            "algorithm_version": spec.algorithm_version,
            "config_digest": spec.config_digest,
            "contents_total": contents_total,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run_by_id(db, run_id)
}

/// Page through unique contents without making memory proportional to corpus
/// size. `after_content_id` is the last UUID string returned by the prior page.
pub fn similarity_sources_after(
    db: &Db,
    snapshot_id: SnapshotId,
    after_content_id: Option<&str>,
    limit: u32,
) -> DfResult<Vec<SimilarityContentSource>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = db
        .conn()
        .prepare(
            "WITH ranked AS (
                SELECT c.id AS content_id, c.size_bytes, c.sha256,
                       r.absolute_path, o.raw_relative_path, o.relative_path,
                       o.fingerprint, o.modified_at_fs,
                       ROW_NUMBER() OVER (
                           PARTITION BY c.id
                           ORDER BY (o.raw_relative_path IS NULL),
                                    o.source_root_id, o.relative_path, o.id
                       ) AS rn
                FROM occurrence_content oc
                JOIN path_occurrences o ON o.id = oc.occurrence_id
                JOIN content_objects c ON c.id = oc.content_id
                JOIN source_roots r ON r.id = o.source_root_id
                WHERE o.snapshot_id = ?1 AND o.scan_status = 'OK'
                  AND c.hash_state = 'HASHED' AND c.sha256 IS NOT NULL
                  AND (?2 IS NULL OR c.id > ?2)
             )
             SELECT content_id, size_bytes, sha256, absolute_path,
                    raw_relative_path, relative_path, fingerprint,
                    modified_at_fs
             FROM ranked WHERE rn = 1
             ORDER BY content_id LIMIT ?3",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(
            params![snapshot_id.to_string(), after_content_id, limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(id, size, sha, root, raw_path, relative, fingerprint, modified)| {
                Ok(SimilarityContentSource {
                    content_id: ContentId::from_str(&id)?,
                    size_bytes: size as u64,
                    sha256: sha,
                    root_path: PathBuf::from(root),
                    raw_relative_path: raw_path.as_deref().map(RawPath::from_blob).transpose()?,
                    relative_path: relative,
                    fingerprint,
                    modified_at: modified
                        .as_deref()
                        .map(parse_stored_timestamp)
                        .transpose()?,
                })
            },
        )
        .collect()
}

pub fn has_content_signature(
    db: &Db,
    content_id: ContentId,
    algorithm_version: &str,
) -> DfResult<bool> {
    db.conn()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM content_minhash
                WHERE content_id = ?1 AND algorithm_version = ?2
             )",
            params![content_id.to_string(), algorithm_version],
            |row| row.get(0),
        )
        .map_err(db_err)
}

/// Transactional writer for one content. Dropping it before `finish` rolls
/// back chunks and memberships, so an interrupted read has no completion
/// marker and no partial membership list.
pub struct ContentChunkWriter<'db> {
    tx: Transaction<'db>,
    content_id: ContentId,
    algorithm_version: String,
    expected_size: u64,
    expected_sha256: String,
    next_ordinal: u64,
    next_offset: u64,
}

pub fn begin_content_chunks<'db>(
    db: &'db mut Db,
    content_id: ContentId,
    algorithm_version: &str,
) -> DfResult<ContentChunkWriter<'db>> {
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let (size, sha): (i64, Option<String>) = tx
        .query_row(
            "SELECT size_bytes, sha256 FROM content_objects
             WHERE id = ?1 AND hash_state = 'HASHED'",
            [content_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| DfError::NotFound(format!("hashed content `{content_id}`")))?;
    let expected_sha256 = sha.ok_or_else(|| {
        DfError::Validation(format!("content `{content_id}` has no canonical SHA-256"))
    })?;
    let existing: bool = tx
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM content_minhash
                WHERE content_id = ?1 AND algorithm_version = ?2
             )",
            params![content_id.to_string(), algorithm_version],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if existing {
        return Err(DfError::Conflict(format!(
            "content `{content_id}` already has similarity evidence for `{algorithm_version}`"
        )));
    }
    let partial: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM chunk_memberships
             WHERE content_id = ?1 AND algorithm_version = ?2",
            params![content_id.to_string(), algorithm_version],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if partial != 0 {
        return Err(DfError::Conflict(format!(
            "content `{content_id}` has {partial} membership rows without a completion marker"
        )));
    }
    Ok(ContentChunkWriter {
        tx,
        content_id,
        algorithm_version: algorithm_version.to_string(),
        expected_size: size as u64,
        expected_sha256,
        next_ordinal: 0,
        next_offset: 0,
    })
}

impl ContentChunkWriter<'_> {
    pub fn write_chunk(&mut self, offset: u64, length: u64, digest: &str) -> DfResult<()> {
        let next_offset = offset
            .checked_add(length)
            .ok_or_else(|| DfError::Validation("chunk offsets overflow u64".to_string()))?;
        if offset != self.next_offset
            || length == 0
            || next_offset > self.expected_size
            || offset > i64::MAX as u64
            || length > i64::MAX as u64
            || next_offset > i64::MAX as u64
            || self.next_ordinal > i64::MAX as u64
            || digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DfError::Validation(format!(
                "non-contiguous or invalid chunk for content `{}`: offset {offset}, length {length}",
                self.content_id
            )));
        }
        let proposed = ChunkId::new();
        let now = to_stored_timestamp(chrono::Utc::now());
        self.tx
            .execute(
                "INSERT INTO chunks
                    (id, algorithm_version, blake3, length_bytes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(algorithm_version, blake3, length_bytes) DO NOTHING",
                params![
                    proposed.to_string(),
                    self.algorithm_version,
                    digest,
                    length as i64,
                    now,
                ],
            )
            .map_err(db_err)?;
        let chunk_id: String = self
            .tx
            .query_row(
                "SELECT id FROM chunks
                 WHERE algorithm_version = ?1 AND blake3 = ?2 AND length_bytes = ?3",
                params![self.algorithm_version, digest, length as i64],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        self.tx
            .execute(
                "INSERT INTO chunk_memberships
                    (content_id, algorithm_version, ordinal, offset_bytes,
                     chunk_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    self.content_id.to_string(),
                    self.algorithm_version,
                    self.next_ordinal as i64,
                    offset as i64,
                    chunk_id,
                    now,
                ],
            )
            .map_err(db_err)?;
        self.next_ordinal = self
            .next_ordinal
            .checked_add(1)
            .ok_or_else(|| DfError::Validation("chunk ordinals overflow u64".to_string()))?;
        self.next_offset = next_offset;
        Ok(())
    }

    pub fn finish(
        self,
        signature: &[u64],
        band_hashes: &[String],
        observed_sha256: &str,
    ) -> DfResult<()> {
        if self.next_ordinal == 0 || self.next_offset != self.expected_size {
            return Err(DfError::Conflict(format!(
                "chunked size {} does not match content `{}` size {}",
                self.next_offset, self.content_id, self.expected_size
            )));
        }
        if observed_sha256 != self.expected_sha256 {
            return Err(DfError::Conflict(format!(
                "source SHA-256 changed while chunking content `{}`",
                self.content_id
            )));
        }
        if signature.len() < 16
            || band_hashes.is_empty()
            || !signature.len().is_multiple_of(band_hashes.len())
        {
            return Err(DfError::Validation(
                "invalid MinHash signature or LSH band count".to_string(),
            ));
        }
        let mut blob = Vec::with_capacity(signature.len() * 8);
        for value in signature {
            blob.extend_from_slice(&value.to_le_bytes());
        }
        let now = to_stored_timestamp(chrono::Utc::now());
        self.tx
            .execute(
                "INSERT INTO content_minhash
                    (content_id, algorithm_version, signature, permutations,
                     total_chunks, total_bytes, source_sha256, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    self.content_id.to_string(),
                    self.algorithm_version,
                    blob,
                    signature.len() as i64,
                    self.next_ordinal as i64,
                    self.next_offset as i64,
                    observed_sha256,
                    now,
                ],
            )
            .map_err(db_err)?;
        for (index, digest) in band_hashes.iter().enumerate() {
            if digest.len() != 64 {
                return Err(DfError::Validation(
                    "LSH band digest must be 64 hexadecimal characters".to_string(),
                ));
            }
            self.tx
                .execute(
                    "INSERT INTO content_lsh_bands
                        (content_id, algorithm_version, band_index, band_hash,
                         created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        self.content_id.to_string(),
                        self.algorithm_version,
                        index as i64,
                        digest,
                        now,
                    ],
                )
                .map_err(db_err)?;
        }
        self.tx.commit().map_err(db_err)
    }
}

/// Rebuild only run-scoped evidence. Global chunks/signatures remain
/// immutable and reusable.
pub fn reset_run_pairs(db: &mut Db, run_id: SimilarityRunId) -> DfResult<()> {
    let run = load_run_by_id(db, run_id)?;
    if run.status != SimilarityRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` is sealed"
        )));
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "DELETE FROM content_relationships WHERE run_id = ?1",
        [run_id.to_string()],
    )
    .map_err(db_err)?;
    tx.execute(
        "DELETE FROM similarity_candidates WHERE run_id = ?1",
        [run_id.to_string()],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

pub fn candidate_count(db: &Db, run_id: SimilarityRunId) -> DfResult<u64> {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM similarity_candidates WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count as u64)
        .map_err(db_err)
}

/// Candidate fallback based on uncommon shared chunks. This is what catches
/// variants whose edits happen to disturb every LSH band.
pub fn generate_rare_chunk_candidates(
    db: &mut Db,
    run_id: SimilarityRunId,
    limit: u64,
) -> DfResult<u64> {
    if limit == 0 {
        return Ok(0);
    }
    let run = load_run_by_id(db, run_id)?;
    if run.status != SimilarityRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` is sealed"
        )));
    }
    let (algorithm, min_file, max_bucket): (String, i64, i64) = db
        .conn()
        .query_row(
            "SELECT algorithm_version, min_file_bytes, max_bucket_contents
             FROM similarity_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_err)?;
    let before = candidate_count(db, run_id)?;
    db.conn()
        .execute(
            "WITH eligible AS (
                SELECT DISTINCT oc.content_id
                FROM occurrence_content oc
                JOIN path_occurrences o ON o.id = oc.occurrence_id
                JOIN content_objects c ON c.id = oc.content_id
                JOIN content_minhash m
                  ON m.content_id = c.id AND m.algorithm_version = ?2
                WHERE o.snapshot_id = ?3 AND c.size_bytes >= ?4
             ), rare AS (
                SELECT cm.chunk_id
                FROM chunk_memberships cm
                JOIN eligible e ON e.content_id = cm.content_id
                WHERE cm.algorithm_version = ?2
                GROUP BY cm.chunk_id
                HAVING COUNT(DISTINCT cm.content_id) BETWEEN 2 AND ?5
             ), pairs AS (
                SELECT a.content_id AS content_a, b.content_id AS content_b,
                       COUNT(DISTINCT a.chunk_id) AS hits
                FROM chunk_memberships a
                JOIN chunk_memberships b
                  ON b.algorithm_version = a.algorithm_version
                 AND b.chunk_id = a.chunk_id AND a.content_id < b.content_id
                JOIN rare r ON r.chunk_id = a.chunk_id
                JOIN eligible ea ON ea.content_id = a.content_id
                JOIN eligible eb ON eb.content_id = b.content_id
                WHERE a.algorithm_version = ?2
                GROUP BY a.content_id, b.content_id
                ORDER BY a.content_id, b.content_id
                LIMIT ?6
             )
             INSERT INTO similarity_candidates
                (run_id, content_a, content_b, shared_bands, rare_chunk_hits,
                 estimated_similarity, status, created_at)
             SELECT ?1, content_a, content_b, 0, hits, 0.0, 'PENDING', ?7
             FROM pairs WHERE true
             ON CONFLICT(run_id, content_a, content_b) DO UPDATE SET
                 rare_chunk_hits = excluded.rare_chunk_hits",
            params![
                run_id.to_string(),
                algorithm,
                run.snapshot_id.to_string(),
                min_file,
                max_bucket,
                limit as i64,
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
    Ok(candidate_count(db, run_id)?.saturating_sub(before))
}

/// Add pairs that collide in one or more bounded LSH buckets.
pub fn generate_lsh_candidates(db: &mut Db, run_id: SimilarityRunId, limit: u64) -> DfResult<u64> {
    if limit == 0 {
        return Ok(0);
    }
    let run = load_run_by_id(db, run_id)?;
    if run.status != SimilarityRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` is sealed"
        )));
    }
    let (algorithm, min_file, max_bucket, bands): (String, i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT algorithm_version, min_file_bytes, max_bucket_contents,
                    lsh_bands
             FROM similarity_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_err)?;
    let before = candidate_count(db, run_id)?;
    db.conn()
        .execute(
            "WITH eligible AS (
                SELECT DISTINCT oc.content_id
                FROM occurrence_content oc
                JOIN path_occurrences o ON o.id = oc.occurrence_id
                JOIN content_objects c ON c.id = oc.content_id
                JOIN content_minhash m
                  ON m.content_id = c.id AND m.algorithm_version = ?2
                WHERE o.snapshot_id = ?3 AND c.size_bytes >= ?4
             ), bounded_buckets AS (
                SELECT b.band_index, b.band_hash
                FROM content_lsh_bands b
                JOIN eligible e ON e.content_id = b.content_id
                WHERE b.algorithm_version = ?2
                GROUP BY b.band_index, b.band_hash
                HAVING COUNT(*) BETWEEN 2 AND ?5
             ), pairs AS (
                SELECT a.content_id AS content_a, b.content_id AS content_b,
                       COUNT(*) AS shared
                FROM content_lsh_bands a
                JOIN content_lsh_bands b
                  ON b.algorithm_version = a.algorithm_version
                 AND b.band_index = a.band_index
                 AND b.band_hash = a.band_hash
                 AND a.content_id < b.content_id
                JOIN bounded_buckets k
                  ON k.band_index = a.band_index AND k.band_hash = a.band_hash
                JOIN eligible ea ON ea.content_id = a.content_id
                JOIN eligible eb ON eb.content_id = b.content_id
                WHERE a.algorithm_version = ?2
                GROUP BY a.content_id, b.content_id
                HAVING NOT EXISTS (
                    SELECT 1 FROM similarity_candidates existing
                    WHERE existing.run_id = ?1
                      AND existing.content_a = a.content_id
                      AND existing.content_b = b.content_id
                )
                ORDER BY a.content_id, b.content_id
                LIMIT ?7
             )
             INSERT INTO similarity_candidates
                (run_id, content_a, content_b, shared_bands, rare_chunk_hits,
                 estimated_similarity, status, created_at)
             SELECT ?1, content_a, content_b, shared, 0,
                    MIN(1.0, CAST(shared AS REAL) / ?6), 'PENDING', ?8
             FROM pairs WHERE true
             ON CONFLICT(run_id, content_a, content_b) DO UPDATE SET
                 shared_bands = excluded.shared_bands,
                 estimated_similarity = excluded.estimated_similarity",
            params![
                run_id.to_string(),
                algorithm,
                run.snapshot_id.to_string(),
                min_file,
                max_bucket,
                bands,
                limit as i64,
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
    Ok(candidate_count(db, run_id)?.saturating_sub(before))
}

/// Keep the deterministic lexical prefix after probing one candidate beyond
/// the configured cap. Returns the number removed.
pub fn trim_candidates(db: &mut Db, run_id: SimilarityRunId, keep: u64) -> DfResult<u64> {
    let before = candidate_count(db, run_id)?;
    if before <= keep {
        return Ok(0);
    }
    db.conn()
        .execute(
            "DELETE FROM similarity_candidates
             WHERE run_id = ?1 AND rowid IN (
                 SELECT rowid FROM similarity_candidates
                 WHERE run_id = ?1
                 ORDER BY content_a, content_b
                 LIMIT -1 OFFSET ?2
             )",
            params![run_id.to_string(), keep as i64],
        )
        .map_err(db_err)?;
    Ok(before - candidate_count(db, run_id)?)
}

/// Recompute LSH collision evidence for every retained candidate. Rare-chunk
/// discovery runs before LSH discovery, so without this pass a pair found by
/// both paths would incorrectly retain `shared_bands = 0`.
pub fn refresh_candidate_band_evidence(db: &mut Db, run_id: SimilarityRunId) -> DfResult<()> {
    let run = load_run_by_id(db, run_id)?;
    if run.status != SimilarityRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` is sealed"
        )));
    }
    db.conn()
        .execute(
            "WITH evidence AS (
                SELECT c.content_a, c.content_b, COUNT(b.band_index) AS shared
                FROM similarity_candidates c
                JOIN similarity_runs r ON r.id = c.run_id
                LEFT JOIN content_lsh_bands a
                  ON a.content_id = c.content_a
                 AND a.algorithm_version = r.algorithm_version
                LEFT JOIN content_lsh_bands b
                  ON b.content_id = c.content_b
                 AND b.algorithm_version = r.algorithm_version
                 AND b.band_index = a.band_index
                 AND b.band_hash = a.band_hash
                WHERE c.run_id = ?1
                GROUP BY c.content_a, c.content_b
             )
             UPDATE similarity_candidates
             SET shared_bands = COALESCE((
                     SELECT shared FROM evidence e
                     WHERE e.content_a = similarity_candidates.content_a
                       AND e.content_b = similarity_candidates.content_b
                 ), 0),
                 estimated_similarity = MIN(1.0, CAST(COALESCE((
                     SELECT shared FROM evidence e
                     WHERE e.content_a = similarity_candidates.content_a
                       AND e.content_b = similarity_candidates.content_b
                 ), 0) AS REAL) / (
                     SELECT lsh_bands FROM similarity_runs WHERE id = ?1
                 ))
             WHERE run_id = ?1",
            [run_id.to_string()],
        )
        .map_err(db_err)?;
    Ok(())
}

pub fn pending_candidates(
    db: &Db,
    run_id: SimilarityRunId,
    limit: u32,
) -> DfResult<Vec<PendingSimilarityCandidate>> {
    let mut stmt = db
        .conn()
        .prepare(
            "WITH timestamps AS (
                SELECT oc.content_id, MIN(o.modified_at_fs) AS modified_at
                FROM occurrence_content oc
                JOIN path_occurrences o ON o.id = oc.occurrence_id
                JOIN similarity_runs r ON r.snapshot_id = o.snapshot_id
                WHERE r.id = ?1
                GROUP BY oc.content_id
             )
             SELECT c.content_a, c.content_b, a.size_bytes, b.size_bytes,
                    ta.modified_at, tb.modified_at, ma.signature, mb.signature,
                    c.shared_bands, c.rare_chunk_hits
             FROM similarity_candidates c
             JOIN similarity_runs r ON r.id = c.run_id
             JOIN content_objects a ON a.id = c.content_a
             JOIN content_objects b ON b.id = c.content_b
             JOIN content_minhash ma
               ON ma.content_id = c.content_a
              AND ma.algorithm_version = r.algorithm_version
             JOIN content_minhash mb
               ON mb.content_id = c.content_b
              AND mb.algorithm_version = r.algorithm_version
             LEFT JOIN timestamps ta ON ta.content_id = c.content_a
             LEFT JOIN timestamps tb ON tb.content_id = c.content_b
             WHERE c.run_id = ?1 AND c.status = 'PENDING'
             ORDER BY c.content_a, c.content_b LIMIT ?2",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(params![run_id.to_string(), limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, Vec<u8>>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(|(a, b, sa, sb, ma, mb, siga, sigb, bands, rare)| {
            Ok(PendingSimilarityCandidate {
                content_a: ContentId::from_str(&a)?,
                content_b: ContentId::from_str(&b)?,
                size_a: sa as u64,
                size_b: sb as u64,
                modified_a: ma.as_deref().map(parse_stored_timestamp).transpose()?,
                modified_b: mb.as_deref().map(parse_stored_timestamp).transpose()?,
                signature_a: siga,
                signature_b: sigb,
                shared_bands: bands as u32,
                rare_chunk_hits: rare as u32,
            })
        })
        .collect()
}

pub fn exact_chunk_overlap(
    db: &Db,
    algorithm_version: &str,
    content_a: ContentId,
    content_b: ContentId,
    size_a: u64,
    size_b: u64,
) -> DfResult<ExactChunkOverlap> {
    let (shared_chunks, shared_bytes): (i64, i64) = db
        .conn()
        .query_row(
            "WITH a AS (
                SELECT chunk_id, COUNT(*) AS n
                FROM chunk_memberships
                WHERE content_id = ?1 AND algorithm_version = ?3
                GROUP BY chunk_id
             ), b AS (
                SELECT chunk_id, COUNT(*) AS n
                FROM chunk_memberships
                WHERE content_id = ?2 AND algorithm_version = ?3
                GROUP BY chunk_id
             )
             SELECT COALESCE(SUM(MIN(a.n, b.n)), 0),
                    COALESCE(SUM(MIN(a.n, b.n) * c.length_bytes), 0)
             FROM a JOIN b ON b.chunk_id = a.chunk_id
             JOIN chunks c ON c.id = a.chunk_id",
            params![
                content_a.to_string(),
                content_b.to_string(),
                algorithm_version
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_err)?;
    let shared_bytes = shared_bytes as u64;
    let union_bytes = size_a
        .checked_add(size_b)
        .and_then(|total| total.checked_sub(shared_bytes))
        .ok_or_else(|| DfError::Conflict("invalid chunk overlap byte accounting".to_string()))?;
    let similarity = if union_bytes == 0 {
        0.0
    } else {
        shared_bytes as f64 / union_bytes as f64
    };
    Ok(ExactChunkOverlap {
        shared_chunks: shared_chunks as u64,
        shared_bytes,
        union_bytes,
        similarity,
    })
}

/// Persist the exact candidate result and optional review-only relation in one
/// transaction.
pub fn record_candidate_evaluation(
    db: &mut Db,
    run_id: SimilarityRunId,
    content_a: ContentId,
    content_b: ContentId,
    estimated_similarity: f64,
    exact: ExactChunkOverlap,
    relation: Option<&ContentRelationship>,
) -> DfResult<()> {
    if !(0.0..=1.0).contains(&estimated_similarity)
        || !(0.0..=1.0).contains(&exact.similarity)
        || exact.shared_chunks > i64::MAX as u64
        || exact.shared_bytes > i64::MAX as u64
        || exact.union_bytes > i64::MAX as u64
    {
        return Err(DfError::Validation(
            "similarity scores or exact evidence exceed persistence bounds".to_string(),
        ));
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let changed = tx
        .execute(
            "UPDATE similarity_candidates
             SET estimated_similarity = ?1, exact_similarity = ?2,
                 shared_chunks = ?3, shared_bytes = ?4, union_bytes = ?5,
                 status = 'EVALUATED'
             WHERE run_id = ?6 AND content_a = ?7 AND content_b = ?8
               AND status = 'PENDING'",
            params![
                estimated_similarity,
                exact.similarity,
                exact.shared_chunks as i64,
                exact.shared_bytes as i64,
                exact.union_bytes as i64,
                run_id.to_string(),
                content_a.to_string(),
                content_b.to_string(),
            ],
        )
        .map_err(db_err)?;
    if changed != 1 {
        return Err(DfError::Conflict(format!(
            "candidate `{content_a}` / `{content_b}` is absent or already evaluated"
        )));
    }
    if let Some(relation) = relation {
        if relation.run_id != run_id
            || relation.content_a != content_a
            || relation.content_b != content_b
        {
            return Err(DfError::Validation(
                "relationship identity does not match its candidate".to_string(),
            ));
        }
        let evidence = serde_json::to_string(&relation.evidence)
            .map_err(|error| DfError::Serialization(format!("relation evidence: {error}")))?;
        tx.execute(
            "INSERT INTO content_relationships
                (id, run_id, snapshot_id, content_a, content_b, kind,
                 direction, similarity, shared_chunks, shared_bytes,
                 union_bytes, estimated_similarity, confidence, evidence_json,
                 created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?13, ?14, ?15)",
            params![
                relation.id.to_string(),
                relation.run_id.to_string(),
                relation.snapshot_id.to_string(),
                relation.content_a.to_string(),
                relation.content_b.to_string(),
                relation.kind.as_str(),
                relation.direction.as_str(),
                relation.similarity,
                relation.shared_chunks as i64,
                relation.shared_bytes as i64,
                relation.union_bytes as i64,
                relation.estimated_similarity,
                relation.confidence,
                evidence,
                to_stored_timestamp(relation.created_at),
            ],
        )
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)
}

/// Seal the run and its exact summary in the same transaction as the audit
/// event. Completion is refused while content or candidate work is missing.
pub fn complete_run(
    db: &mut Db,
    run_id: SimilarityRunId,
    candidate_cap_reached: bool,
    actor: Actor,
) -> DfResult<SimilarityRun> {
    let run = load_run_by_id(db, run_id)?;
    if run.status == SimilarityRunStatus::Completed {
        return Ok(run);
    }
    if run.status != SimilarityRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` cannot complete"
        )));
    }
    let (min_file, algorithm): (i64, String) = db
        .conn()
        .query_row(
            "SELECT min_file_bytes, algorithm_version FROM similarity_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_err)?;
    let (total, chunked, skipped, chunks): (i64, i64, i64, i64) = db
        .conn()
        .query_row(
            "WITH eligible AS (
                SELECT DISTINCT c.id, c.size_bytes
                FROM occurrence_content oc
                JOIN path_occurrences o ON o.id = oc.occurrence_id
                JOIN content_objects c ON c.id = oc.content_id
                WHERE o.snapshot_id = ?1 AND o.scan_status = 'OK'
                  AND c.hash_state = 'HASHED' AND c.sha256 IS NOT NULL
             )
             SELECT COUNT(*),
                    COUNT(*) FILTER (
                        WHERE size_bytes >= ?2 AND EXISTS (
                            SELECT 1 FROM content_minhash m
                            WHERE m.content_id = eligible.id
                              AND m.algorithm_version = ?3
                        )
                    ),
                    COUNT(*) FILTER (WHERE size_bytes < ?2),
                    COALESCE(SUM((
                        SELECT m.total_chunks FROM content_minhash m
                        WHERE m.content_id = eligible.id
                          AND m.algorithm_version = ?3
                    )) FILTER (WHERE size_bytes >= ?2), 0)
             FROM eligible",
            params![run.snapshot_id.to_string(), min_file, algorithm],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_err)?;
    if chunked + skipped != total {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` still has {} unchunked contents",
            total - chunked - skipped
        )));
    }
    let (candidates, pending, relations): (i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT COUNT(*),
                    COUNT(*) FILTER (WHERE status = 'PENDING'),
                    (SELECT COUNT(*) FROM content_relationships WHERE run_id = ?1)
             FROM similarity_candidates WHERE run_id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_err)?;
    if pending != 0 {
        return Err(DfError::Conflict(format!(
            "similarity run `{run_id}` still has {pending} pending candidates"
        )));
    }
    let counters = SimilarityRunCounters {
        contents_total: total as u64,
        contents_chunked: chunked as u64,
        contents_skipped: skipped as u64,
        chunks_total: chunks as u64,
        candidates_total: candidates as u64,
        relations_total: relations as u64,
    };
    let finished = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE similarity_runs
         SET status = 'COMPLETED', contents_total = ?1,
             contents_chunked = ?2, contents_skipped = ?3, chunks_total = ?4,
             candidates_total = ?5, relations_total = ?6,
             candidate_cap_reached = ?7, finished_at = ?8
         WHERE id = ?9 AND status = 'RUNNING'",
        params![
            counters.contents_total as i64,
            counters.contents_chunked as i64,
            counters.contents_skipped as i64,
            counters.chunks_total as i64,
            counters.candidates_total as i64,
            counters.relations_total as i64,
            i64::from(candidate_cap_reached),
            finished,
            run_id.to_string(),
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        run.project_id,
        EVENT_SIMILARITY_COMPLETED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "algorithm_version": run.algorithm_version,
            "config_digest": run.config_digest,
            "counters": counters,
            "candidate_cap_reached": candidate_cap_reached,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run_by_id(db, run_id)
}

pub fn latest_completed_run(db: &Db, project_id: ProjectId) -> DfResult<Option<SimilarityRun>> {
    let id: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM similarity_runs
             WHERE project_id = ?1 AND status = 'COMPLETED'
             ORDER BY finished_at DESC, id DESC LIMIT 1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    id.map(|value| load_run_by_id(db, SimilarityRunId::from_str(&value)?))
        .transpose()
}

pub fn list_relationships(
    db: &Db,
    run_id: SimilarityRunId,
    limit: u32,
) -> DfResult<Vec<ContentRelationship>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, snapshot_id, content_a, content_b, kind, direction,
                    similarity, shared_chunks, shared_bytes, union_bytes,
                    estimated_similarity, confidence, evidence_json, created_at
             FROM content_relationships WHERE run_id = ?1
             ORDER BY similarity DESC, shared_bytes DESC, content_a, content_b
             LIMIT ?2",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(params![run_id.to_string(), limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, f64>(10)?,
                row.get::<_, f64>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(
                id,
                snapshot,
                a,
                b,
                kind,
                direction,
                similarity,
                shared_chunks,
                shared_bytes,
                union_bytes,
                estimated,
                confidence,
                evidence,
                created,
            )| {
                Ok(ContentRelationship {
                    id: SimilarityRelationId::from_str(&id)?,
                    run_id,
                    snapshot_id: SnapshotId::from_str(&snapshot)?,
                    content_a: ContentId::from_str(&a)?,
                    content_b: ContentId::from_str(&b)?,
                    kind: df_domain::ContentRelationshipKind::parse(&kind)?,
                    direction: RelationshipDirection::parse(&direction)?,
                    similarity,
                    shared_chunks: shared_chunks as u64,
                    shared_bytes: shared_bytes as u64,
                    union_bytes: union_bytes as u64,
                    estimated_similarity: estimated,
                    confidence,
                    evidence: serde_json::from_str(&evidence).map_err(|error| {
                        DfError::Serialization(format!("stored relationship evidence: {error}"))
                    })?,
                    created_at: parse_stored_timestamp(&created)?,
                })
            },
        )
        .collect()
}

/// Human-facing representative path for one content in a snapshot. The raw
/// path is decoded only for display here; no filesystem operation uses it.
pub fn representative_display_path(
    db: &Db,
    snapshot_id: SnapshotId,
    content_id: ContentId,
) -> DfResult<String> {
    let stored: Option<(String, String, Option<Vec<u8>>)> = db
        .conn()
        .query_row(
            "SELECT r.absolute_path, o.relative_path, o.raw_relative_path
             FROM occurrence_content oc
             JOIN path_occurrences o ON o.id = oc.occurrence_id
             JOIN source_roots r ON r.id = o.source_root_id
             WHERE o.snapshot_id = ?1 AND oc.content_id = ?2
             ORDER BY (o.raw_relative_path IS NULL), o.source_root_id,
                      o.relative_path, o.id LIMIT 1",
            params![snapshot_id.to_string(), content_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_err)?;
    let Some((root, legacy, raw)) = stored else {
        return Err(DfError::NotFound(format!(
            "content `{content_id}` in snapshot `{snapshot_id}`"
        )));
    };
    let relative = raw
        .as_deref()
        .map(RawPath::from_blob)
        .transpose()?
        .map(|path| path.display())
        .unwrap_or(legacy);
    Ok(PathBuf::from(root).join(relative).display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{ProfileRef, Project, SourceRoot};

    #[test]
    fn dropped_content_writer_rolls_back_every_membership() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "chunk rollback",
            ProfileRef::default(),
            temp.path().join("out"),
            temp.path().join("audit"),
            "test",
        );
        crate::repository::create_project(
            &mut db,
            &project,
            &[SourceRoot::new(project.id, temp.path().join("source"))],
            Actor::Test,
        )
        .unwrap();
        let snapshot = SnapshotId::new();
        let content = ContentId::new();
        db.conn()
            .execute(
                "INSERT INTO snapshots (id, project_id, status, created_at)
                 VALUES (?1, ?2, 'COMPLETE', '2026-01-01T00:00:00.000Z')",
                params![snapshot.to_string(), project.id.to_string()],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO content_objects
                    (id, size_bytes, sha256, blake3, first_seen_snapshot,
                     hash_state, created_at)
                 VALUES (?1, 4, ?2, ?2, ?3, 'HASHED', 't')",
                params![content.to_string(), "a".repeat(64), snapshot.to_string()],
            )
            .unwrap();

        {
            let mut writer = begin_content_chunks(&mut db, content, "test-v1").unwrap();
            writer.write_chunk(0, 4, &"b".repeat(64)).unwrap();
            // Simulated crash: no completion call.
        }
        let memberships: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM chunk_memberships", [], |row| {
                row.get(0)
            })
            .unwrap();
        let chunks: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(memberships, 0);
        assert_eq!(chunks, 0);
    }

    #[test]
    fn content_writer_rejects_chunks_past_the_canonical_size() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "chunk bounds",
            ProfileRef::default(),
            temp.path().join("out"),
            temp.path().join("audit"),
            "test",
        );
        crate::repository::create_project(
            &mut db,
            &project,
            &[SourceRoot::new(project.id, temp.path().join("source"))],
            Actor::Test,
        )
        .unwrap();
        let snapshot = SnapshotId::new();
        let content = ContentId::new();
        db.conn()
            .execute(
                "INSERT INTO snapshots (id, project_id, status, created_at)
                 VALUES (?1, ?2, 'COMPLETE', '2026-01-01T00:00:00.000Z')",
                params![snapshot.to_string(), project.id.to_string()],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO content_objects
                    (id, size_bytes, sha256, blake3, first_seen_snapshot,
                     hash_state, created_at)
                 VALUES (?1, 4, ?2, ?2, ?3, 'HASHED', 't')",
                params![content.to_string(), "a".repeat(64), snapshot.to_string()],
            )
            .unwrap();

        let mut writer = begin_content_chunks(&mut db, content, "test-v1").unwrap();
        assert!(writer.write_chunk(0, 5, &"b".repeat(64)).is_err());
        drop(writer);
        let persisted: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM chunk_memberships", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[test]
    fn completed_similarity_run_seals_candidates_relations_and_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "similarity seal",
            ProfileRef::default(),
            temp.path().join("out"),
            temp.path().join("audit"),
            "test",
        );
        let source_root = SourceRoot::new(project.id, temp.path().join("source"));
        crate::repository::create_project(
            &mut db,
            &project,
            std::slice::from_ref(&source_root),
            Actor::Test,
        )
        .unwrap();
        let snapshot = SnapshotId::new();
        let first = ContentId::new();
        let second = ContentId::new();
        let (content_a, content_b) = if first.to_string() < second.to_string() {
            (first, second)
        } else {
            (second, first)
        };
        let run = SimilarityRunId::new();
        db.conn()
            .execute(
                "INSERT INTO snapshots (id, project_id, status, created_at)
                 VALUES (?1, ?2, 'COMPLETE', '2026-01-01T00:00:00.000Z')",
                params![snapshot.to_string(), project.id.to_string()],
            )
            .unwrap();
        for (content, digest) in [(content_a, "a".repeat(64)), (content_b, "b".repeat(64))] {
            db.conn()
                .execute(
                    "INSERT INTO content_objects
                        (id, size_bytes, sha256, blake3, first_seen_snapshot,
                         hash_state, created_at)
                     VALUES (?1, 100, ?2, ?2, ?3, 'HASHED', 't')",
                    params![content.to_string(), digest, snapshot.to_string()],
                )
                .unwrap();
        }
        for (index, (content, digest)) in [(content_a, "a".repeat(64)), (content_b, "b".repeat(64))]
            .into_iter()
            .enumerate()
        {
            let occurrence = df_domain::OccurrenceId::new();
            let relative = format!("file-{index}.bin");
            db.conn()
                .execute(
                    "INSERT INTO path_occurrences
                        (id, snapshot_id, source_root_id, relative_path,
                         parent_relative_path, file_name, normalized_name,
                         extension, size_bytes, attributes, path_length,
                         depth, fingerprint, scan_status, name_is_lossy,
                         created_at)
                     VALUES (?1, ?2, ?3, ?4, '', ?4, ?4, 'bin', 100, 0,
                             10, 1, 'v1:100:none', 'OK', 0, 't')",
                    params![
                        occurrence.to_string(),
                        snapshot.to_string(),
                        source_root.id.to_string(),
                        relative,
                    ],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO occurrence_content
                        (occurrence_id, content_id, created_at)
                     VALUES (?1, ?2, 't')",
                    params![occurrence.to_string(), content.to_string()],
                )
                .unwrap();
            let chunk = ChunkId::new();
            db.conn()
                .execute(
                    "INSERT INTO chunks
                        (id, algorithm_version, blake3, length_bytes, created_at)
                     VALUES (?1, 'test-v1', ?2, 100, 't')",
                    params![chunk.to_string(), digest],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO chunk_memberships
                        (content_id, algorithm_version, ordinal, offset_bytes,
                         chunk_id, created_at)
                     VALUES (?1, 'test-v1', 0, 0, ?2, 't')",
                    params![content.to_string(), chunk.to_string()],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO content_minhash
                        (content_id, algorithm_version, signature,
                         permutations, total_chunks, total_bytes,
                         source_sha256, created_at)
                     VALUES (?1, 'test-v1', ?2, 16, 1, 100, ?3, 't')",
                    params![content.to_string(), vec![0_u8; 128], digest],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO content_lsh_bands
                        (content_id, algorithm_version, band_index, band_hash,
                         created_at)
                     VALUES (?1, 'test-v1', 0, ?2, 't')",
                    params![content.to_string(), "d".repeat(64)],
                )
                .unwrap();
        }
        db.conn()
            .execute(
                "INSERT INTO similarity_runs
                    (id, project_id, snapshot_id, status, algorithm_version,
                     config_digest, config_json, min_chunk_bytes,
                     avg_chunk_bytes, max_chunk_bytes, min_file_bytes,
                     threshold, min_shared_chunks, min_shared_bytes,
                     minhash_permutations, lsh_bands, max_bucket_contents,
                     max_candidates, started_at, created_at)
                 VALUES (?1, ?2, ?3, 'RUNNING', 'test-v1', ?4, '{}', 64,
                         256, 1024, 1, 0.5, 1, 1, 16, 4, 8, 10,
                         '2026-01-01T00:00:00.000Z',
                         '2026-01-01T00:00:00.000Z')",
                params![
                    run.to_string(),
                    project.id.to_string(),
                    snapshot.to_string(),
                    "c".repeat(64),
                ],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO similarity_candidates
                    (run_id, content_a, content_b, shared_bands,
                     rare_chunk_hits, estimated_similarity, exact_similarity,
                     shared_chunks, shared_bytes, union_bytes, status,
                     created_at)
                 VALUES (?1, ?2, ?3, 0, 0, 0.5, 0.5, 1, 50, 100,
                         'EVALUATED', 't')",
                params![
                    run.to_string(),
                    content_a.to_string(),
                    content_b.to_string()
                ],
            )
            .unwrap();
        refresh_candidate_band_evidence(&mut db, run).unwrap();
        let shared_bands: i64 = db
            .conn()
            .query_row(
                "SELECT shared_bands FROM similarity_candidates WHERE run_id = ?1",
                [run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(shared_bands, 1);
        db.conn()
            .execute(
                "INSERT INTO content_relationships
                    (id, run_id, snapshot_id, content_a, content_b, kind,
                     direction, similarity, shared_chunks, shared_bytes,
                     union_bytes, estimated_similarity, confidence,
                     evidence_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'SIMILAR_CONTENT', 'UNKNOWN',
                         0.5, 1, 50, 100, 0.5, 0.75, '{}', 't')",
                params![
                    SimilarityRelationId::new().to_string(),
                    run.to_string(),
                    snapshot.to_string(),
                    content_a.to_string(),
                    content_b.to_string(),
                ],
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE similarity_runs
                 SET status = 'COMPLETED', finished_at = 't2',
                     candidates_total = 1, relations_total = 1
                 WHERE id = ?1",
                [run.to_string()],
            )
            .unwrap();

        for sql in [
            "UPDATE similarity_candidates SET exact_similarity = exact_similarity WHERE run_id = ?1",
            "DELETE FROM similarity_candidates WHERE run_id = ?1",
            "UPDATE content_relationships SET confidence = confidence WHERE run_id = ?1",
            "DELETE FROM content_relationships WHERE run_id = ?1",
            "UPDATE similarity_runs SET error = error WHERE id = ?1",
        ] {
            let error = db.conn().execute(sql, [run.to_string()]).unwrap_err();
            assert!(
                error.to_string().contains("sealed")
                    || error.to_string().contains("immutable"),
                "sealed evidence escaped via `{sql}`: {error}"
            );
        }
    }
}
