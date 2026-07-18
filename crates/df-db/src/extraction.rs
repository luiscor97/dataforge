//! Transactional persistence for document-intelligence evidence (M0.4).
//!
//! Extraction output is immutable and configuration-addressed. SQLite keeps
//! the bounded, segmented normalized text required to rebuild disposable
//! Tantivy and Parquet artifacts; source and attachment bytes never enter the
//! database.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use df_domain::{
    Actor, AnalyticalSnapshotId, ArchiveEntry, ContentId, DocumentFormat, DocumentRepresentation,
    ExtractionRun, ExtractionRunCounters, ExtractionRunId, ExtractionRunStatus, ExtractionStatus,
    MailAttachment, MailMessage, MailThreadId, ProjectId, RawPath, RepresentationId, SearchIndexId,
    SnapshotId, TextSubject, TextSubjectId, TextSubjectKind, Timestamp,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

use crate::repository::{append_event, parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

pub const EVENT_CONTENT_EXTRACTION_STARTED: &str = "CONTENT_EXTRACTION_STARTED";
pub const EVENT_CONTENT_EXTRACTION_COMPLETED: &str = "CONTENT_EXTRACTION_COMPLETED";
pub const EVENT_CONTENT_EXTRACTION_FAILED: &str = "CONTENT_EXTRACTION_FAILED";
pub const EVENT_MAIL_THREADS_BUILT: &str = "MAIL_THREADS_BUILT";
pub const EVENT_SEARCH_INDEX_BUILT: &str = "SEARCH_INDEX_BUILT";
pub const EVENT_ANALYTICAL_SNAPSHOT_BUILT: &str = "ANALYTICAL_SNAPSHOT_BUILT";

/// Fully expanded identity and resource limits of one extraction run.
#[derive(Debug, Clone)]
pub struct ExtractionRunSpec {
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub extractor_version: String,
    pub config_digest: String,
    pub config_json: String,
    pub max_input_bytes: u64,
    pub max_text_chars: u64,
    pub text_segment_chars: u64,
    pub max_archive_entries: u32,
    pub max_archive_entry_bytes: u64,
    pub max_archive_total_bytes: u64,
    pub max_archive_ratio: f64,
    pub max_archive_depth: u32,
}

#[derive(Debug)]
struct StoredRunSpec {
    project_id: String,
    extractor_version: String,
    config_json: String,
    max_input_bytes: i64,
    max_text_chars: i64,
    text_segment_chars: i64,
    max_archive_entries: i64,
    max_archive_entry_bytes: i64,
    max_archive_total_bytes: i64,
    max_archive_ratio: f64,
    max_archive_depth: i64,
}

#[derive(Debug, Clone, Copy)]
struct PersistenceLimits {
    max_text_chars: u64,
    text_segment_chars: u64,
}

/// Deterministic representative used to reopen one pending physical content.
#[derive(Debug, Clone)]
pub struct ExtractionContentSource {
    pub content_id: ContentId,
    pub size_bytes: u64,
    pub sha256: String,
    pub root_path: PathBuf,
    pub raw_relative_path: Option<RawPath>,
    pub relative_path: String,
    pub file_name: String,
    pub fingerprint: String,
    pub modified_at: Option<Timestamp>,
    /// Existing evidence with the exact extractor/configuration identity.
    pub reusable_representation_id: Option<RepresentationId>,
}

/// One bounded normalized-text segment. Offsets count Unicode scalar values,
/// matching SQLite `length(TEXT)`, rather than UTF-8 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSegmentInput {
    pub ordinal: u32,
    pub char_start: u64,
    pub char_end: u64,
    pub text: String,
    pub text_sha256: String,
}

#[derive(Debug, Clone)]
pub struct TextSubjectInput {
    pub subject: TextSubject,
    pub segments: Vec<TextSegmentInput>,
}

/// Complete evidence for one content. It is committed atomically with the
/// run/content binding; a malformed child rolls the whole representation back.
#[derive(Debug, Clone)]
pub struct ContentExtractionInput {
    pub representation: DocumentRepresentation,
    pub source_sha256: String,
    pub subjects: Vec<TextSubjectInput>,
    pub mail_message: Option<MailMessage>,
    pub mail_attachments: Vec<MailAttachment>,
    pub archive_entries: Vec<ArchiveEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailThreadMemberInput {
    pub representation_id: RepresentationId,
    pub parent_representation_id: Option<RepresentationId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailThreadInput {
    pub id: MailThreadId,
    pub root_message_id: Option<String>,
    pub normalized_subject: Option<String>,
    /// Parent messages must occur before their children.
    pub members: Vec<MailThreadMemberInput>,
}

/// Immutable message metadata consumed by deterministic thread reconstruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMessageRow {
    pub representation_id: RepresentationId,
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub sent_at: Option<String>,
    pub subject: Option<String>,
    pub normalized_subject: Option<String>,
    pub body_sha256: Option<String>,
}

/// Bounded row consumed by both the search-index and Parquet builders.
#[derive(Debug, Clone)]
pub struct IndexSubjectRow {
    pub run_id: ExtractionRunId,
    pub subject_id: TextSubjectId,
    pub content_id: ContentId,
    pub kind: TextSubjectKind,
    pub display_name: String,
    pub virtual_path: Option<String>,
    pub mime: String,
    pub metadata: serde_json::Value,
    pub size_bytes: u64,
    pub normalized_chars: u64,
    pub text_truncated: bool,
    pub document_format: DocumentFormat,
    pub extraction_status: ExtractionStatus,
    pub representation_error: Option<String>,
    pub file_name: String,
    pub relative_path: String,
    pub representative_path: String,
    pub context: String,
    pub title: Option<String>,
    pub mail_subject: Option<String>,
    pub mail_from: Vec<String>,
    pub mail_to: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexRecord {
    pub id: SearchIndexId,
    pub run_id: ExtractionRunId,
    pub snapshot_id: SnapshotId,
    pub schema_version: String,
    pub relative_path: String,
    pub content_digest: String,
    pub documents: u64,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticalSnapshotRecord {
    pub id: AnalyticalSnapshotId,
    pub run_id: ExtractionRunId,
    pub snapshot_id: SnapshotId,
    pub schema_version: String,
    pub relative_path: String,
    pub sha256: String,
    pub rows: u64,
    pub created_at: Timestamp,
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
    i64,
    i64,
    i64,
    Option<String>,
    String,
    Option<String>,
);

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn checked_i64(value: u64, field: &str) -> DfResult<i64> {
    i64::try_from(value)
        .map_err(|_| DfError::Validation(format!("{field} exceeds SQLite INTEGER bounds")))
}

fn run_from_stored(row: StoredRunRow) -> DfResult<ExtractionRun> {
    let (
        id,
        project,
        snapshot,
        status,
        extractor_version,
        config_digest,
        config_json,
        contents_total,
        extracted,
        unsupported,
        limited,
        failed,
        text_subjects,
        text_segments,
        mail_messages,
        mail_threads,
        mail_attachments,
        error,
        started_at,
        finished_at,
    ) = row;
    let archive_entries = 0;
    // Kept separate below: the SQL loader appends archive_entries to the JSON
    // query as part of `load_run`; this helper receives the compact tuple used
    // by rusqlite's tuple implementations.
    Ok(ExtractionRun {
        id: ExtractionRunId::from_str(&id)?,
        project_id: ProjectId::from_str(&project)?,
        snapshot_id: SnapshotId::from_str(&snapshot)?,
        status: ExtractionRunStatus::parse(&status)?,
        extractor_version,
        config_digest,
        config: serde_json::from_str(&config_json).map_err(|error| {
            DfError::Serialization(format!("stored extraction config: {error}"))
        })?,
        counters: ExtractionRunCounters {
            contents_total: contents_total as u64,
            extracted: extracted as u64,
            unsupported: unsupported as u64,
            limited: limited as u64,
            failed: failed as u64,
            text_subjects: text_subjects as u64,
            text_segments: text_segments as u64,
            mail_messages: mail_messages as u64,
            mail_threads: mail_threads as u64,
            mail_attachments: mail_attachments as u64,
            archive_entries,
        },
        error,
        started_at: parse_stored_timestamp(&started_at)?,
        finished_at: finished_at
            .as_deref()
            .map(parse_stored_timestamp)
            .transpose()?,
    })
}

/// Load one extraction run and its sealed counters.
pub fn load_run(db: &Db, run_id: ExtractionRunId) -> DfResult<ExtractionRun> {
    type Raw = (StoredRunRow, i64);
    let raw: Raw = db
        .conn()
        .query_row(
            "SELECT id, project_id, snapshot_id, status, extractor_version,
                    config_digest, config_json, contents_total, extracted,
                    unsupported, limited, failed, text_subjects, text_segments,
                    mail_messages, mail_threads, mail_attachments, error,
                    started_at, finished_at, archive_entries
             FROM extraction_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| {
                Ok((
                    (
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
                        row.get(17)?,
                        row.get(18)?,
                        row.get(19)?,
                    ),
                    row.get(20)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| DfError::NotFound(format!("extraction run `{run_id}`")))?;
    let mut run = run_from_stored(raw.0)?;
    run.counters.archive_entries = raw.1 as u64;
    Ok(run)
}

/// Resolve the newest completed extraction for a project, optionally scoped
/// to one snapshot. Artifact commands use this instead of issuing SQL outside
/// `df-db`.
pub fn latest_completed_run(
    db: &Db,
    project_id: ProjectId,
    snapshot_id: Option<SnapshotId>,
) -> DfResult<Option<ExtractionRun>> {
    let id: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM extraction_runs
             WHERE project_id = ?1 AND status = 'COMPLETED'
               AND (?2 IS NULL OR snapshot_id = ?2)
             ORDER BY finished_at DESC, id DESC LIMIT 1",
            params![
                project_id.to_string(),
                snapshot_id.map(|value| value.to_string())
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    id.as_deref()
        .map(ExtractionRunId::from_str)
        .transpose()?
        .map(|id| load_run(db, id))
        .transpose()
}

fn validate_spec(spec: &ExtractionRunSpec) -> DfResult<()> {
    if spec.extractor_version.trim().is_empty() || !is_sha256(&spec.config_digest) {
        return Err(DfError::Validation(
            "extractor version and lowercase SHA-256 config digest are required".to_string(),
        ));
    }
    let config: serde_json::Value = serde_json::from_str(&spec.config_json)
        .map_err(|error| DfError::Validation(format!("invalid extraction config JSON: {error}")))?;
    let computed_digest = sha256_hex(spec.config_json.as_bytes());
    if computed_digest != spec.config_digest {
        return Err(DfError::Validation(format!(
            "extraction config digest mismatch: expected {computed_digest}"
        )));
    }
    for (value, field) in [
        (spec.max_input_bytes, "max_input_bytes"),
        (spec.max_text_chars, "max_text_chars"),
        (spec.text_segment_chars, "text_segment_chars"),
        (spec.max_archive_entry_bytes, "max_archive_entry_bytes"),
        (spec.max_archive_total_bytes, "max_archive_total_bytes"),
    ] {
        if value == 0 {
            return Err(DfError::Validation(format!("{field} must be positive")));
        }
        checked_i64(value, field)?;
    }
    if spec.text_segment_chars > spec.max_text_chars
        || spec.max_archive_entries == 0
        || spec.max_archive_entry_bytes > spec.max_archive_total_bytes
        || !spec.max_archive_ratio.is_finite()
        || spec.max_archive_ratio < 1.0
        || spec.max_archive_depth == 0
    {
        return Err(DfError::Validation(
            "invalid extraction resource limits".to_string(),
        ));
    }
    let object = config.as_object().ok_or_else(|| {
        DfError::Validation("extraction config JSON must be an object".to_string())
    })?;
    let exact_u64 = |name: &str, expected: u64| -> DfResult<()> {
        let actual = object.get(name).and_then(serde_json::Value::as_u64);
        if actual != Some(expected) {
            return Err(DfError::Validation(format!(
                "extraction config field `{name}` does not match persisted limit {expected}"
            )));
        }
        Ok(())
    };
    exact_u64("max_input_bytes", spec.max_input_bytes)?;
    exact_u64("max_text_chars", spec.max_text_chars)?;
    exact_u64("text_segment_chars", spec.text_segment_chars)?;
    exact_u64("max_archive_entries", u64::from(spec.max_archive_entries))?;
    exact_u64("max_archive_entry_bytes", spec.max_archive_entry_bytes)?;
    exact_u64("max_archive_total_bytes", spec.max_archive_total_bytes)?;
    exact_u64(
        "max_archive_compression_ratio",
        spec.max_archive_ratio as u64,
    )?;
    exact_u64(
        "max_archive_nesting_depth",
        u64::from(spec.max_archive_depth),
    )?;
    if spec.max_archive_ratio.fract() != 0.0 {
        return Err(DfError::Validation(
            "max_archive_ratio must be an integer-valued ratio".to_string(),
        ));
    }
    Ok(())
}

fn ensure_run_matches_spec(
    db: &Db,
    run_id: ExtractionRunId,
    spec: &ExtractionRunSpec,
) -> DfResult<()> {
    let stored = db
        .conn()
        .query_row(
            "SELECT project_id, extractor_version, config_json,
                    max_input_bytes, max_text_chars, text_segment_chars,
                    max_archive_entries, max_archive_entry_bytes,
                    max_archive_total_bytes, max_archive_ratio,
                    max_archive_depth
             FROM extraction_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| {
                Ok(StoredRunSpec {
                    project_id: row.get(0)?,
                    extractor_version: row.get(1)?,
                    config_json: row.get(2)?,
                    max_input_bytes: row.get(3)?,
                    max_text_chars: row.get(4)?,
                    text_segment_chars: row.get(5)?,
                    max_archive_entries: row.get(6)?,
                    max_archive_entry_bytes: row.get(7)?,
                    max_archive_total_bytes: row.get(8)?,
                    max_archive_ratio: row.get(9)?,
                    max_archive_depth: row.get(10)?,
                })
            },
        )
        .map_err(db_err)?;
    let matches = stored.project_id == spec.project_id.to_string()
        && stored.extractor_version == spec.extractor_version
        && stored.config_json == spec.config_json
        && stored.max_input_bytes == spec.max_input_bytes as i64
        && stored.max_text_chars == spec.max_text_chars as i64
        && stored.text_segment_chars == spec.text_segment_chars as i64
        && stored.max_archive_entries == i64::from(spec.max_archive_entries)
        && stored.max_archive_entry_bytes == spec.max_archive_entry_bytes as i64
        && stored.max_archive_total_bytes == spec.max_archive_total_bytes as i64
        && stored.max_archive_ratio.to_bits() == spec.max_archive_ratio.to_bits()
        && stored.max_archive_depth == i64::from(spec.max_archive_depth);
    if !matches {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` does not match the configuration addressed by its digest"
        )));
    }
    Ok(())
}

fn load_persistence_limits(db: &Db, run_id: ExtractionRunId) -> DfResult<PersistenceLimits> {
    db.conn()
        .query_row(
            "SELECT max_text_chars, text_segment_chars
             FROM extraction_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| {
                Ok(PersistenceLimits {
                    max_text_chars: row.get::<_, i64>(0)? as u64,
                    text_segment_chars: row.get::<_, i64>(1)? as u64,
                })
            },
        )
        .map_err(db_err)
}

/// Start a run or resume the identical configuration. Completed replay is a
/// no-op; failed evidence must receive a new configuration digest.
pub fn start_or_resume_run(
    db: &mut Db,
    spec: &ExtractionRunSpec,
    actor: Actor,
) -> DfResult<ExtractionRun> {
    validate_spec(spec)?;
    let eligible: bool = db
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
    if !eligible {
        return Err(DfError::Validation(
            "content extraction requires a completed, analysed snapshot".to_string(),
        ));
    }
    let existing: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM extraction_runs
             WHERE snapshot_id = ?1 AND extractor_version = ?2
               AND config_digest = ?3",
            params![
                spec.snapshot_id.to_string(),
                spec.extractor_version,
                spec.config_digest
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if let Some(id) = existing {
        let run_id = ExtractionRunId::from_str(&id)?;
        ensure_run_matches_spec(db, run_id, spec)?;
        let run = load_run(db, run_id)?;
        if run.status == ExtractionRunStatus::Failed {
            return Err(DfError::Conflict(format!(
                "extraction run `{}` failed; use a new configuration digest",
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
    let run_id = ExtractionRunId::new();
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO extraction_runs
            (id, project_id, snapshot_id, status, extractor_version,
             config_digest, config_json, max_input_bytes, max_text_chars,
             text_segment_chars, max_archive_entries,
             max_archive_entry_bytes, max_archive_total_bytes,
             max_archive_ratio, max_archive_depth, contents_total,
             started_at, created_at)
         VALUES (?1, ?2, ?3, 'RUNNING', ?4, ?5, ?6, ?7, ?8, ?9,
                 ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
        params![
            run_id.to_string(),
            spec.project_id.to_string(),
            spec.snapshot_id.to_string(),
            spec.extractor_version,
            spec.config_digest,
            spec.config_json,
            spec.max_input_bytes as i64,
            spec.max_text_chars as i64,
            spec.text_segment_chars as i64,
            i64::from(spec.max_archive_entries),
            spec.max_archive_entry_bytes as i64,
            spec.max_archive_total_bytes as i64,
            spec.max_archive_ratio,
            i64::from(spec.max_archive_depth),
            contents_total,
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        spec.project_id,
        EVENT_CONTENT_EXTRACTION_STARTED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": spec.snapshot_id.to_string(),
            "extractor_version": spec.extractor_version,
            "config_digest": spec.config_digest,
            "contents_total": contents_total,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run(db, run_id)
}

/// Page through still-unbound unique contents. Passing `None` after a crash
/// safely restarts at the first gap because already committed rows are omitted.
pub fn pending_content_sources_after(
    db: &Db,
    run_id: ExtractionRunId,
    after_content_id: Option<&str>,
    limit: u32,
) -> DfResult<Vec<ExtractionContentSource>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` is sealed"
        )));
    }
    let mut stmt = db
        .conn()
        .prepare(
            "WITH ranked AS (
                SELECT c.id AS content_id, c.size_bytes, c.sha256,
                       root.absolute_path, o.raw_relative_path, o.relative_path,
                       o.file_name, o.fingerprint, o.modified_at_fs,
                       d.id AS reusable_representation_id,
                       ROW_NUMBER() OVER (
                           PARTITION BY c.id
                           ORDER BY (o.raw_relative_path IS NULL),
                                    o.source_root_id, o.relative_path, o.id
                       ) AS rn
                FROM extraction_runs run
                JOIN path_occurrences o ON o.snapshot_id = run.snapshot_id
                JOIN occurrence_content oc ON oc.occurrence_id = o.id
                JOIN content_objects c ON c.id = oc.content_id
                JOIN source_roots root ON root.id = o.source_root_id
                LEFT JOIN document_representations d
                  ON d.content_id = c.id
                 AND d.extractor_version = run.extractor_version
                 AND d.config_digest = run.config_digest
                WHERE run.id = ?1 AND run.status = 'RUNNING'
                  AND o.scan_status = 'OK' AND c.hash_state = 'HASHED'
                  AND c.sha256 IS NOT NULL
                  AND (?2 IS NULL OR c.id > ?2)
                  AND NOT EXISTS (
                      SELECT 1 FROM extraction_run_contents done
                      WHERE done.run_id = run.id AND done.content_id = c.id
                  )
             )
             SELECT content_id, size_bytes, sha256, absolute_path,
                    raw_relative_path, relative_path, file_name, fingerprint,
                    modified_at_fs, reusable_representation_id
             FROM ranked WHERE rn = 1 ORDER BY content_id LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(
            params![run_id.to_string(), after_content_id, i64::from(limit)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    rows.into_iter()
        .map(
            |(id, size, sha, root, raw, relative, name, fingerprint, modified, reusable)| {
                Ok(ExtractionContentSource {
                    content_id: ContentId::from_str(&id)?,
                    size_bytes: size as u64,
                    sha256: sha,
                    root_path: PathBuf::from(root),
                    raw_relative_path: raw.as_deref().map(RawPath::from_blob).transpose()?,
                    relative_path: relative,
                    file_name: name,
                    fingerprint,
                    modified_at: modified
                        .as_deref()
                        .map(parse_stored_timestamp)
                        .transpose()?,
                    reusable_representation_id: reusable
                        .as_deref()
                        .map(RepresentationId::from_str)
                        .transpose()?,
                })
            },
        )
        .collect()
}

type StoredRepresentation = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
    Option<String>,
    String,
);

fn representation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRepresentation> {
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
    ))
}

fn representation_from_stored(raw: StoredRepresentation) -> DfResult<DocumentRepresentation> {
    let (
        id,
        content,
        extractor_version,
        config_digest,
        format,
        mime,
        status,
        title,
        normalized_text_sha256,
        normalized_chars,
        text_truncated,
        metadata_json,
        error,
        created_at,
    ) = raw;
    Ok(DocumentRepresentation {
        id: RepresentationId::from_str(&id)?,
        content_id: ContentId::from_str(&content)?,
        extractor_version,
        config_digest,
        format: DocumentFormat::parse(&format)?,
        mime,
        status: ExtractionStatus::parse(&status)?,
        title,
        normalized_text_sha256,
        normalized_chars: normalized_chars as u64,
        text_truncated: text_truncated != 0,
        metadata: serde_json::from_str(&metadata_json).map_err(|error| {
            DfError::Serialization(format!("stored representation metadata: {error}"))
        })?,
        error,
        created_at: parse_stored_timestamp(&created_at)?,
    })
}

/// Load an immutable representation by identifier.
pub fn load_representation(
    db: &Db,
    representation_id: RepresentationId,
) -> DfResult<DocumentRepresentation> {
    let raw = db
        .conn()
        .query_row(
            "SELECT id, content_id, extractor_version, config_digest, format,
                    mime, status, title, normalized_text_sha256,
                    normalized_chars, text_truncated, metadata_json, error,
                    created_at
             FROM document_representations WHERE id = ?1",
            [representation_id.to_string()],
            representation_from_row,
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| DfError::NotFound(format!("representation `{representation_id}`")))?;
    representation_from_stored(raw)
}

fn existing_representation_for_run(
    db: &Db,
    run: &ExtractionRun,
    content_id: ContentId,
) -> DfResult<Option<DocumentRepresentation>> {
    let raw = db
        .conn()
        .query_row(
            "SELECT id, content_id, extractor_version, config_digest, format,
                    mime, status, title, normalized_text_sha256,
                    normalized_chars, text_truncated, metadata_json, error,
                    created_at
             FROM document_representations
             WHERE content_id = ?1 AND extractor_version = ?2
               AND config_digest = ?3",
            params![
                content_id.to_string(),
                run.extractor_version,
                run.config_digest
            ],
            representation_from_row,
        )
        .optional()
        .map_err(db_err)?;
    raw.map(representation_from_stored).transpose()
}

fn mapped_representation(
    db: &Db,
    run_id: ExtractionRunId,
    content_id: ContentId,
) -> DfResult<Option<RepresentationId>> {
    let id: Option<String> = db
        .conn()
        .query_row(
            "SELECT representation_id FROM extraction_run_contents
             WHERE run_id = ?1 AND content_id = ?2",
            params![run_id.to_string(), content_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    id.as_deref().map(RepresentationId::from_str).transpose()
}

/// Bind already-extracted global evidence to a later snapshot/run. Returns
/// `None` when this exact extractor/configuration has no reusable result.
pub fn bind_reusable_representation(
    db: &mut Db,
    run_id: ExtractionRunId,
    content_id: ContentId,
) -> DfResult<Option<RepresentationId>> {
    if let Some(id) = mapped_representation(db, run_id, content_id)? {
        return Ok(Some(id));
    }
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` is sealed"
        )));
    }
    let Some(representation) = existing_representation_for_run(db, &run, content_id)? else {
        return Ok(None);
    };
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO extraction_run_contents
            (run_id, content_id, representation_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            run_id.to_string(),
            content_id.to_string(),
            representation.id.to_string(),
            representation.status.as_str(),
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(Some(representation.id))
}

fn validate_representation_shape(
    input: &ContentExtractionInput,
    run: &ExtractionRun,
    limits: PersistenceLimits,
) -> DfResult<()> {
    let representation = &input.representation;
    if representation.extractor_version != run.extractor_version
        || representation.config_digest != run.config_digest
        || !is_sha256(&input.source_sha256)
        || representation.mime.trim().is_empty()
        || representation.normalized_chars > i64::MAX as u64
        || representation
            .normalized_text_sha256
            .as_deref()
            .is_some_and(|digest| !is_sha256(digest))
    {
        return Err(DfError::Validation(
            "representation identity, MIME or normalized-text bounds are invalid".to_string(),
        ));
    }
    match representation.status {
        ExtractionStatus::Extracted
            if representation.error.is_some()
                || representation.format == DocumentFormat::Unsupported =>
        {
            return Err(DfError::Validation(
                "an extracted representation must be supported and error-free".to_string(),
            ));
        }
        ExtractionStatus::Unsupported
            if representation.error.is_some()
                || representation.format != DocumentFormat::Unsupported =>
        {
            return Err(DfError::Validation(
                "an unsupported representation must use the unsupported format".to_string(),
            ));
        }
        ExtractionStatus::Limited | ExtractionStatus::Failed
            if representation
                .error
                .as_deref()
                .is_none_or(|error| error.trim().is_empty()) =>
        {
            return Err(DfError::Validation(
                "limited and failed representations require an error".to_string(),
            ));
        }
        _ => {}
    }

    let mut ids = HashSet::new();
    let mut documents = Vec::new();
    let mut subjects_by_id = HashMap::new();
    let mut total_normalized_chars = 0_u64;
    for input_subject in &input.subjects {
        let subject = &input_subject.subject;
        if subject.representation_id != representation.id
            || !ids.insert(subject.id)
            || subject.display_name.trim().is_empty()
            || subject.mime.trim().is_empty()
            || subject.size_bytes > i64::MAX as u64
            || subject.normalized_chars > i64::MAX as u64
            || subject
                .normalized_text_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
        {
            return Err(DfError::Validation(
                "invalid or duplicate text subject".to_string(),
            ));
        }
        total_normalized_chars = total_normalized_chars
            .checked_add(subject.normalized_chars)
            .ok_or_else(|| {
                DfError::Validation("normalized text character total overflowed".to_string())
            })?;
        if total_normalized_chars > limits.max_text_chars {
            return Err(DfError::Validation(
                "representation normalized text exceeds its configured bound".to_string(),
            ));
        }
        if subject.kind == TextSubjectKind::Document {
            if subject.parent_subject_id.is_some() || subject.virtual_path.is_some() {
                return Err(DfError::Validation(
                    "the document subject cannot have a parent or virtual path".to_string(),
                ));
            }
            documents.push(subject);
        } else if subject.parent_subject_id.is_none()
            || subject
                .virtual_path
                .as_deref()
                .is_none_or(|path| path.trim().is_empty())
        {
            return Err(DfError::Validation(
                "embedded subjects require a parent and virtual path".to_string(),
            ));
        }
        subjects_by_id.insert(subject.id, subject);

        let mut next_char = 0_u64;
        let mut hasher = Sha256::new();
        for (expected_ordinal, segment) in input_subject.segments.iter().enumerate() {
            let chars = segment.text.chars().count() as u64;
            if segment.ordinal as usize != expected_ordinal
                || segment.char_start != next_char
                || segment.char_end != segment.char_start.saturating_add(chars)
                || segment.text.is_empty()
                || chars > limits.text_segment_chars
                || !is_sha256(&segment.text_sha256)
                || sha256_hex(segment.text.as_bytes()) != segment.text_sha256
                || segment.char_end > i64::MAX as u64
            {
                return Err(DfError::Validation(format!(
                    "invalid text segment {} for subject `{}`",
                    segment.ordinal, subject.id
                )));
            }
            hasher.update(segment.text.as_bytes());
            next_char = segment.char_end;
        }
        if next_char != subject.normalized_chars {
            return Err(DfError::Validation(format!(
                "subject `{}` normalized character count does not match its segments",
                subject.id
            )));
        }
        match &subject.normalized_text_sha256 {
            Some(expected) if hex::encode(hasher.finalize()) != *expected => {
                return Err(DfError::Validation(format!(
                    "subject `{}` normalized text digest does not match its segments",
                    subject.id
                )));
            }
            None if !input_subject.segments.is_empty() || subject.normalized_chars != 0 => {
                return Err(DfError::Validation(format!(
                    "subject `{}` has text without a normalized digest",
                    subject.id
                )));
            }
            _ => {}
        }
    }
    for subject in input.subjects.iter().map(|item| &item.subject) {
        if let Some(parent) = subject.parent_subject_id {
            if !ids.contains(&parent) {
                return Err(DfError::Validation(format!(
                    "subject `{}` references a parent outside its representation",
                    subject.id
                )));
            }
        }
    }

    match representation.status {
        ExtractionStatus::Extracted => {
            if documents.len() != 1 {
                return Err(DfError::Validation(
                    "extracted and limited results require exactly one document subject"
                        .to_string(),
                ));
            }
            let document = documents[0];
            if representation.normalized_text_sha256 != document.normalized_text_sha256
                || representation.normalized_chars != document.normalized_chars
                || representation.text_truncated != document.text_truncated
            {
                return Err(DfError::Validation(
                    "representation text summary must match its document subject".to_string(),
                ));
            }
        }
        ExtractionStatus::Limited => {
            if documents.len() > 1 {
                return Err(DfError::Validation(
                    "a limited result can have at most one document subject".to_string(),
                ));
            }
            if let Some(document) = documents.first() {
                if representation.normalized_text_sha256 != document.normalized_text_sha256
                    || representation.normalized_chars != document.normalized_chars
                    || representation.text_truncated != document.text_truncated
                {
                    return Err(DfError::Validation(
                        "representation text summary must match its document subject".to_string(),
                    ));
                }
            } else if representation.normalized_text_sha256.is_some()
                || representation.normalized_chars != 0
                || representation.text_truncated
            {
                return Err(DfError::Validation(
                    "a limited result without a subject cannot claim normalized text".to_string(),
                ));
            }
        }
        ExtractionStatus::Unsupported | ExtractionStatus::Failed
            if !input.subjects.is_empty()
                || representation.normalized_text_sha256.is_some()
                || representation.normalized_chars != 0
                || representation.text_truncated =>
        {
            return Err(DfError::Validation(
                "unsupported and failed results cannot persist normalized text".to_string(),
            ));
        }
        _ => {}
    }

    if let Some(message) = &input.mail_message {
        if representation.format != DocumentFormat::Eml
            || message.representation_id != representation.id
            || message
                .body_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
        {
            return Err(DfError::Validation("invalid EML metadata".to_string()));
        }
    } else if representation.format == DocumentFormat::Eml
        && matches!(
            representation.status,
            ExtractionStatus::Extracted | ExtractionStatus::Limited
        )
    {
        return Err(DfError::Validation(
            "an extracted EML representation requires message metadata".to_string(),
        ));
    }
    if !input.mail_attachments.is_empty() && representation.format != DocumentFormat::Eml {
        return Err(DfError::Validation(
            "mail attachments require an EML representation".to_string(),
        ));
    }
    let mut attachment_ids = HashSet::new();
    for (ordinal, attachment) in input.mail_attachments.iter().enumerate() {
        let attachment_subject = subjects_by_id.get(&attachment.subject_id).copied();
        let parent_is_document = attachment_subject
            .and_then(|subject| subject.parent_subject_id)
            .and_then(|parent| subjects_by_id.get(&parent).copied())
            .is_some_and(|parent| parent.kind == TextSubjectKind::Document);
        if attachment.representation_id != representation.id
            || attachment.ordinal as usize != ordinal
            || !attachment_ids.insert(attachment.id)
            || attachment.file_name.trim().is_empty()
            || attachment.mime.trim().is_empty()
            || attachment.size_bytes > i64::MAX as u64
            || !is_sha256(&attachment.sha256)
            || attachment_subject.is_none_or(|subject| {
                subject.kind != TextSubjectKind::MailAttachment || !parent_is_document
            })
        {
            return Err(DfError::Validation(
                "invalid mail attachment evidence".to_string(),
            ));
        }
    }
    if !input.archive_entries.is_empty() && representation.format != DocumentFormat::Zip {
        return Err(DfError::Validation(
            "archive entries require a ZIP representation".to_string(),
        ));
    }
    let mut archive_ids = HashSet::new();
    let mut archive_paths = HashSet::new();
    for (ordinal, entry) in input.archive_entries.iter().enumerate() {
        let subject_ok = entry.subject_id.is_none_or(|subject_id| {
            subjects_by_id.get(&subject_id).is_some_and(|subject| {
                subject.kind == TextSubjectKind::ArchiveEntry
                    && subject.virtual_path.as_deref() == Some(entry.virtual_path.as_str())
            })
        });
        if entry.representation_id != representation.id
            || entry.ordinal as usize != ordinal
            || !archive_ids.insert(entry.id)
            || !archive_paths.insert(entry.virtual_path.as_str())
            || entry.virtual_path.trim().is_empty()
            || entry.compressed_bytes > i64::MAX as u64
            || entry.size_bytes > i64::MAX as u64
            || entry
                .sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
            || !subject_ok
            || (entry.directory && (entry.subject_id.is_some() || entry.sha256.is_some()))
        {
            return Err(DfError::Validation(
                "invalid archive entry evidence".to_string(),
            ));
        }
    }
    Ok(())
}

fn representation_equivalent(
    stored: &DocumentRepresentation,
    proposed: &DocumentRepresentation,
) -> bool {
    stored.content_id == proposed.content_id
        && stored.extractor_version == proposed.extractor_version
        && stored.config_digest == proposed.config_digest
        && stored.format == proposed.format
        && stored.mime == proposed.mime
        && stored.status == proposed.status
        && stored.title == proposed.title
        && stored.normalized_text_sha256 == proposed.normalized_text_sha256
        && stored.normalized_chars == proposed.normalized_chars
        && stored.text_truncated == proposed.text_truncated
        && stored.metadata == proposed.metadata
        && stored.error == proposed.error
}

fn insert_subjects(
    tx: &Transaction<'_>,
    representation_id: RepresentationId,
    subjects: &[TextSubjectInput],
) -> DfResult<()> {
    let mut inserted = HashSet::new();
    let mut pending: Vec<&TextSubjectInput> = subjects.iter().collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut deferred = Vec::new();
        for input in pending {
            let subject = &input.subject;
            if subject
                .parent_subject_id
                .is_some_and(|parent| !inserted.contains(&parent))
            {
                deferred.push(input);
                continue;
            }
            let metadata_json = serde_json::to_string(&subject.metadata).map_err(|error| {
                DfError::Serialization(format!("text subject metadata: {error}"))
            })?;
            tx.execute(
                "INSERT INTO text_subjects
                    (id, representation_id, kind, parent_subject_id,
                     display_name, virtual_path, mime, size_bytes,
                     normalized_text_sha256, normalized_chars, text_truncated,
                     metadata_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         ?11, ?12, ?13)",
                params![
                    subject.id.to_string(),
                    representation_id.to_string(),
                    subject.kind.as_str(),
                    subject.parent_subject_id.map(|id| id.to_string()),
                    subject.display_name,
                    subject.virtual_path,
                    subject.mime,
                    subject.size_bytes as i64,
                    subject.normalized_text_sha256,
                    subject.normalized_chars as i64,
                    i64::from(subject.text_truncated),
                    metadata_json,
                    to_stored_timestamp(subject.created_at),
                ],
            )
            .map_err(db_err)?;
            for segment in &input.segments {
                tx.execute(
                    "INSERT INTO text_segments
                        (subject_id, ordinal, char_start, char_end, text,
                         text_sha256, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        subject.id.to_string(),
                        i64::from(segment.ordinal),
                        segment.char_start as i64,
                        segment.char_end as i64,
                        segment.text,
                        segment.text_sha256,
                        to_stored_timestamp(subject.created_at),
                    ],
                )
                .map_err(db_err)?;
            }
            inserted.insert(subject.id);
        }
        if deferred.len() == before {
            return Err(DfError::Validation(
                "text subject parent graph contains a cycle".to_string(),
            ));
        }
        pending = deferred;
    }
    Ok(())
}

/// Persist one new representation and every child row atomically. Replaying a
/// committed content returns its existing identifier. If another snapshot
/// already produced equivalent evidence, only the run binding is appended.
pub fn persist_content_result(
    db: &mut Db,
    run_id: ExtractionRunId,
    input: &ContentExtractionInput,
) -> DfResult<RepresentationId> {
    let content_id = input.representation.content_id;
    let run = load_run(db, run_id)?;
    let limits = load_persistence_limits(db, run_id)?;
    let canonical_sha256: Option<String> = db
        .conn()
        .query_row(
            "SELECT sha256 FROM content_objects
             WHERE id = ?1 AND hash_state = 'HASHED'",
            [content_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .flatten();
    if canonical_sha256.as_deref() != Some(input.source_sha256.as_str()) {
        return Err(DfError::Conflict(format!(
            "content `{content_id}` source digest is not canonical"
        )));
    }
    if let Some(id) = mapped_representation(db, run_id, content_id)? {
        validate_representation_shape(input, &run, limits)?;
        let stored = load_representation(db, id)?;
        if !representation_equivalent(&stored, &input.representation) {
            return Err(DfError::Conflict(format!(
                "content `{content_id}` replay differs from its committed representation"
            )));
        }
        return Ok(id);
    }
    if run.status != ExtractionRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` is sealed"
        )));
    }
    validate_representation_shape(input, &run, limits)?;

    if let Some(existing) = existing_representation_for_run(db, &run, content_id)? {
        if !representation_equivalent(&existing, &input.representation) {
            return Err(DfError::Conflict(format!(
                "extractor/configuration produced non-deterministic evidence for content `{content_id}`"
            )));
        }
        let tx = db.conn_mut().transaction().map_err(db_err)?;
        tx.execute(
            "INSERT INTO extraction_run_contents
                (run_id, content_id, representation_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id.to_string(),
                content_id.to_string(),
                existing.id.to_string(),
                existing.status.as_str(),
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        return Ok(existing.id);
    }

    let representation = &input.representation;
    let metadata_json = serde_json::to_string(&representation.metadata).map_err(|error| {
        DfError::Serialization(format!("document representation metadata: {error}"))
    })?;
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO document_representations
            (id, content_id, extractor_version, config_digest, format, mime,
             status, title, normalized_text_sha256, normalized_chars,
             text_truncated, metadata_json, error, source_sha256, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                 ?12, ?13, ?14, ?15)",
        params![
            representation.id.to_string(),
            representation.content_id.to_string(),
            representation.extractor_version,
            representation.config_digest,
            representation.format.as_str(),
            representation.mime,
            representation.status.as_str(),
            representation.title,
            representation.normalized_text_sha256,
            representation.normalized_chars as i64,
            i64::from(representation.text_truncated),
            metadata_json,
            representation.error,
            input.source_sha256,
            to_stored_timestamp(representation.created_at),
        ],
    )
    .map_err(db_err)?;
    insert_subjects(&tx, representation.id, &input.subjects)?;

    if let Some(message) = &input.mail_message {
        let in_reply_to = serde_json::to_string(&message.in_reply_to)
            .map_err(|error| DfError::Serialization(format!("in-reply-to: {error}")))?;
        let references = serde_json::to_string(&message.references)
            .map_err(|error| DfError::Serialization(format!("mail references: {error}")))?;
        let from = serde_json::to_string(&message.from)
            .map_err(|error| DfError::Serialization(format!("mail from: {error}")))?;
        let to = serde_json::to_string(&message.to)
            .map_err(|error| DfError::Serialization(format!("mail to: {error}")))?;
        let cc = serde_json::to_string(&message.cc)
            .map_err(|error| DfError::Serialization(format!("mail cc: {error}")))?;
        tx.execute(
            "INSERT INTO mail_messages
                (representation_id, message_id, in_reply_to_json,
                 references_json, from_json, to_json, cc_json, sent_at,
                 subject, normalized_subject, body_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12)",
            params![
                representation.id.to_string(),
                message.message_id,
                in_reply_to,
                references,
                from,
                to,
                cc,
                message.sent_at,
                message.subject,
                message.normalized_subject,
                message.body_sha256,
                to_stored_timestamp(representation.created_at),
            ],
        )
        .map_err(db_err)?;
    }
    for attachment in &input.mail_attachments {
        tx.execute(
            "INSERT INTO mail_attachments
                (id, representation_id, subject_id, ordinal, file_name, mime,
                 size_bytes, sha256, extraction_status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                attachment.id.to_string(),
                representation.id.to_string(),
                attachment.subject_id.to_string(),
                i64::from(attachment.ordinal),
                attachment.file_name,
                attachment.mime,
                attachment.size_bytes as i64,
                attachment.sha256,
                attachment.extraction_status.as_str(),
                to_stored_timestamp(attachment.created_at),
            ],
        )
        .map_err(db_err)?;
    }
    for entry in &input.archive_entries {
        tx.execute(
            "INSERT INTO archive_entries
                (id, representation_id, subject_id, ordinal, virtual_path,
                 compressed_bytes, size_bytes, crc32, encrypted, directory,
                 sha256, extraction_status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13)",
            params![
                entry.id.to_string(),
                representation.id.to_string(),
                entry.subject_id.map(|id| id.to_string()),
                i64::from(entry.ordinal),
                entry.virtual_path,
                entry.compressed_bytes as i64,
                entry.size_bytes as i64,
                i64::from(entry.crc32),
                i64::from(entry.encrypted),
                i64::from(entry.directory),
                entry.sha256,
                entry.extraction_status.as_str(),
                to_stored_timestamp(entry.created_at),
            ],
        )
        .map_err(db_err)?;
    }
    tx.execute(
        "INSERT INTO extraction_run_contents
            (run_id, content_id, representation_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            run_id.to_string(),
            content_id.to_string(),
            representation.id.to_string(),
            representation.status.as_str(),
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(representation.id)
}

fn stored_threads_match(
    db: &Db,
    run_id: ExtractionRunId,
    threads: &[MailThreadInput],
) -> DfResult<bool> {
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM mail_threads WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if count as usize != threads.len() {
        return Ok(false);
    }
    for thread in threads {
        let stored: Option<(Option<String>, Option<String>, i64)> = db
            .conn()
            .query_row(
                "SELECT root_message_id, normalized_subject, message_count
                 FROM mail_threads WHERE id = ?1 AND run_id = ?2",
                params![thread.id.to_string(), run_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(db_err)?;
        if stored
            != Some((
                thread.root_message_id.clone(),
                thread.normalized_subject.clone(),
                thread.members.len() as i64,
            ))
        {
            return Ok(false);
        }
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT representation_id, parent_representation_id
                 FROM mail_thread_members
                 WHERE thread_id = ?1 AND run_id = ?2 ORDER BY ordinal",
            )
            .map_err(db_err)?;
        let stored_members = stmt
            .query_map(params![thread.id.to_string(), run_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let expected = thread
            .members
            .iter()
            .map(|member| {
                (
                    member.representation_id.to_string(),
                    member.parent_representation_id.map(|id| id.to_string()),
                )
            })
            .collect::<Vec<_>>();
        if stored_members != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn stored_string_list(value: String, field: &str) -> DfResult<Vec<String>> {
    serde_json::from_str(&value)
        .map_err(|error| DfError::Serialization(format!("stored {field}: {error}")))
}

/// Load every EML message bound to a run in a stable order. The caller may
/// reconstruct basic threads, but cannot inspect arbitrary database tables.
pub fn mail_messages_for_threading(
    db: &Db,
    run_id: ExtractionRunId,
) -> DfResult<Vec<ThreadMessageRow>> {
    load_run(db, run_id)?;
    let mut statement = db
        .conn()
        .prepare(
            "SELECT m.representation_id, m.message_id, m.in_reply_to_json,
                    m.references_json, m.from_json, m.to_json, m.cc_json,
                    m.sent_at, m.subject, m.normalized_subject, m.body_sha256
             FROM mail_messages m
             JOIN extraction_run_contents rc
               ON rc.representation_id = m.representation_id
             WHERE rc.run_id = ?1
             ORDER BY (m.sent_at IS NULL), m.sent_at, m.representation_id",
        )
        .map_err(db_err)?;
    let raw = statement
        .query_map([run_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(
                representation,
                message_id,
                in_reply_to,
                references,
                from,
                to,
                cc,
                sent_at,
                subject,
                normalized_subject,
                body_sha256,
            )| {
                Ok(ThreadMessageRow {
                    representation_id: RepresentationId::from_str(&representation)?,
                    message_id,
                    in_reply_to: stored_string_list(in_reply_to, "In-Reply-To JSON")?,
                    references: stored_string_list(references, "References JSON")?,
                    from: stored_string_list(from, "From JSON")?,
                    to: stored_string_list(to, "To JSON")?,
                    cc: stored_string_list(cc, "Cc JSON")?,
                    sent_at,
                    subject,
                    normalized_subject,
                    body_sha256,
                })
            },
        )
        .collect()
}

/// Current immutable thread/member counts for replay diagnostics.
pub fn mail_thread_counts(db: &Db, run_id: ExtractionRunId) -> DfResult<(u64, u64)> {
    load_run(db, run_id)?;
    let counts: (i64, i64) = db
        .conn()
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM mail_threads WHERE run_id = ?1),
                (SELECT COUNT(*) FROM mail_thread_members WHERE run_id = ?1)",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_err)?;
    Ok((counts.0 as u64, counts.1 as u64))
}

/// Persist all basic EML threads for a run in one transaction. Each message
/// belongs to at most one thread, and parent rows must precede child rows.
pub fn persist_mail_threads(
    db: &mut Db,
    run_id: ExtractionRunId,
    threads: &[MailThreadInput],
    actor: Actor,
) -> DfResult<u64> {
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Running {
        if run.status == ExtractionRunStatus::Completed
            && stored_threads_match(db, run_id, threads)?
        {
            return Ok(threads.len() as u64);
        }
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` is sealed"
        )));
    }
    let existing: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM mail_threads WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if existing != 0 {
        if stored_threads_match(db, run_id, threads)? {
            return Ok(existing as u64);
        }
        return Err(DfError::Conflict(
            "mail thread replay differs from already persisted evidence".to_string(),
        ));
    }
    if threads.is_empty() {
        return Ok(0);
    }
    let mut thread_ids = HashSet::new();
    let mut all_messages = HashSet::new();
    for thread in threads {
        if !thread_ids.insert(thread.id) || thread.members.is_empty() {
            return Err(DfError::Validation(
                "mail threads require unique IDs and at least one member".to_string(),
            ));
        }
        let mut earlier = HashSet::new();
        for member in &thread.members {
            if !all_messages.insert(member.representation_id)
                || member.parent_representation_id == Some(member.representation_id)
                || member
                    .parent_representation_id
                    .is_some_and(|parent| !earlier.contains(&parent))
            {
                return Err(DfError::Validation(
                    "mail thread members are duplicated, cyclic or not parent-first".to_string(),
                ));
            }
            earlier.insert(member.representation_id);
        }
    }

    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    for thread in threads {
        tx.execute(
            "INSERT INTO mail_threads
                (id, run_id, snapshot_id, root_message_id,
                 normalized_subject, message_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                thread.id.to_string(),
                run_id.to_string(),
                run.snapshot_id.to_string(),
                thread.root_message_id,
                thread.normalized_subject,
                thread.members.len() as i64,
                now,
            ],
        )
        .map_err(db_err)?;
        for (ordinal, member) in thread.members.iter().enumerate() {
            tx.execute(
                "INSERT INTO mail_thread_members
                    (thread_id, run_id, representation_id,
                     parent_representation_id, ordinal, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    thread.id.to_string(),
                    run_id.to_string(),
                    member.representation_id.to_string(),
                    member.parent_representation_id.map(|id| id.to_string()),
                    ordinal as i64,
                    now,
                ],
            )
            .map_err(db_err)?;
        }
    }
    append_event(
        &tx,
        run.project_id,
        EVENT_MAIL_THREADS_BUILT,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "threads": threads.len(),
            "messages": all_messages.len(),
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    Ok(threads.len() as u64)
}

fn evidence_counters(db: &Db, run_id: ExtractionRunId) -> DfResult<ExtractionRunCounters> {
    let raw: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = ?1),
                (SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = ?1 AND rc.status = 'EXTRACTED'),
                (SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = ?1 AND rc.status = 'UNSUPPORTED'),
                (SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = ?1 AND rc.status = 'LIMITED'),
                (SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = ?1 AND rc.status = 'FAILED'),
                (SELECT COUNT(*) FROM text_subjects s JOIN extraction_run_contents rc ON rc.representation_id = s.representation_id WHERE rc.run_id = ?1),
                (SELECT COUNT(*) FROM text_segments g JOIN text_subjects s ON s.id = g.subject_id JOIN extraction_run_contents rc ON rc.representation_id = s.representation_id WHERE rc.run_id = ?1),
                (SELECT COUNT(*) FROM mail_messages m JOIN extraction_run_contents rc ON rc.representation_id = m.representation_id WHERE rc.run_id = ?1),
                (SELECT COUNT(*) FROM mail_threads t WHERE t.run_id = ?1),
                (SELECT COUNT(*) FROM mail_attachments a JOIN extraction_run_contents rc ON rc.representation_id = a.representation_id WHERE rc.run_id = ?1),
                (SELECT COUNT(*) FROM archive_entries a JOIN extraction_run_contents rc ON rc.representation_id = a.representation_id WHERE rc.run_id = ?1)",
            [run_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                    row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
                    row.get(8)?, row.get(9)?, row.get(10)?,
                ))
            },
        )
        .map_err(db_err)?;
    Ok(ExtractionRunCounters {
        contents_total: raw.0 as u64,
        extracted: raw.1 as u64,
        unsupported: raw.2 as u64,
        limited: raw.3 as u64,
        failed: raw.4 as u64,
        text_subjects: raw.5 as u64,
        text_segments: raw.6 as u64,
        mail_messages: raw.7 as u64,
        mail_threads: raw.8 as u64,
        mail_attachments: raw.9 as u64,
        archive_entries: raw.10 as u64,
    })
}

/// Seal a fully processed run and its exact evidence summary together with the
/// completion audit event.
pub fn complete_run(db: &mut Db, run_id: ExtractionRunId, actor: Actor) -> DfResult<ExtractionRun> {
    let run = load_run(db, run_id)?;
    if run.status == ExtractionRunStatus::Completed {
        return Ok(run);
    }
    if run.status != ExtractionRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` cannot complete"
        )));
    }
    let counters = evidence_counters(db, run_id)?;
    if counters.contents_total != run.counters.contents_total {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` still has {} pending contents",
            run.counters
                .contents_total
                .saturating_sub(counters.contents_total)
        )));
    }
    let finished_at = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let changed = tx
        .execute(
            "UPDATE extraction_runs SET
                status = 'COMPLETED', contents_total = ?1, extracted = ?2,
                unsupported = ?3, limited = ?4, failed = ?5,
                text_subjects = ?6, text_segments = ?7, mail_messages = ?8,
                mail_threads = ?9, mail_attachments = ?10,
                archive_entries = ?11, finished_at = ?12
             WHERE id = ?13 AND status = 'RUNNING'",
            params![
                counters.contents_total as i64,
                counters.extracted as i64,
                counters.unsupported as i64,
                counters.limited as i64,
                counters.failed as i64,
                counters.text_subjects as i64,
                counters.text_segments as i64,
                counters.mail_messages as i64,
                counters.mail_threads as i64,
                counters.mail_attachments as i64,
                counters.archive_entries as i64,
                finished_at,
                run_id.to_string(),
            ],
        )
        .map_err(db_err)?;
    if changed != 1 {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` was sealed concurrently"
        )));
    }
    append_event(
        &tx,
        run.project_id,
        EVENT_CONTENT_EXTRACTION_COMPLETED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "extractor_version": run.extractor_version,
            "config_digest": run.config_digest,
            "counters": counters,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run(db, run_id)
}

/// Seal a run as failed without pretending its partial evidence is complete.
pub fn fail_run(
    db: &mut Db,
    run_id: ExtractionRunId,
    error: &str,
    actor: Actor,
) -> DfResult<ExtractionRun> {
    if error.trim().is_empty() {
        return Err(DfError::Validation(
            "a failed extraction requires an error".to_string(),
        ));
    }
    let run = load_run(db, run_id)?;
    if run.status == ExtractionRunStatus::Failed {
        if run.error.as_deref() != Some(error) {
            return Err(DfError::Conflict(format!(
                "failed extraction run `{run_id}` already records a different error"
            )));
        }
        return Ok(run);
    }
    if run.status != ExtractionRunStatus::Running {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` cannot fail after completion"
        )));
    }
    let finished_at = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE extraction_runs
         SET status = 'FAILED', error = ?1, finished_at = ?2
         WHERE id = ?3 AND status = 'RUNNING'",
        params![error, finished_at, run_id.to_string()],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        run.project_id,
        EVENT_CONTENT_EXTRACTION_FAILED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "error": error,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run(db, run_id)
}

/// Page deterministic subject metadata for rebuilding search/analytical
/// artifacts without materialising corpus-sized state in memory.
pub fn index_subjects_after(
    db: &Db,
    run_id: ExtractionRunId,
    after_subject_id: Option<&str>,
    limit: u32,
) -> DfResult<Vec<IndexSubjectRow>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::Conflict(format!(
            "derived artifacts require completed extraction run `{run_id}`"
        )));
    }
    let mut stmt = db
        .conn()
        .prepare(
            "WITH physical AS (
                SELECT rc.content_id, root.absolute_path,
                       o.raw_relative_path, o.relative_path, o.file_name,
                       COALESCE(fc.kind, 'NEUTRAL') AS context,
                       ROW_NUMBER() OVER (
                           PARTITION BY rc.content_id
                           ORDER BY (o.raw_relative_path IS NULL),
                                    o.source_root_id, o.relative_path, o.id
                       ) AS rn
                FROM extraction_run_contents rc
                JOIN extraction_runs run ON run.id = rc.run_id
                JOIN occurrence_content oc ON oc.content_id = rc.content_id
                JOIN path_occurrences o
                  ON o.id = oc.occurrence_id AND o.snapshot_id = run.snapshot_id
                 AND o.scan_status = 'OK'
                JOIN source_roots root ON root.id = o.source_root_id
                LEFT JOIN folders f
                  ON f.snapshot_id = o.snapshot_id
                 AND f.source_root_id = o.source_root_id
                 AND f.relative_path = o.parent_relative_path
                LEFT JOIN folder_contexts fc ON fc.folder_id = f.id
                WHERE rc.run_id = ?1
             )
             SELECT s.id, rc.content_id, s.kind, s.display_name,
                    s.virtual_path, s.mime, s.metadata_json, s.size_bytes,
                    s.normalized_chars, s.text_truncated, d.format, d.status,
                    d.error, p.absolute_path, p.raw_relative_path,
                    p.relative_path, p.file_name, p.context, d.title,
                    m.subject, m.from_json, m.to_json
             FROM extraction_run_contents rc
             JOIN document_representations d ON d.id = rc.representation_id
             JOIN text_subjects s ON s.representation_id = d.id
             JOIN physical p ON p.content_id = rc.content_id AND p.rn = 1
             LEFT JOIN mail_messages m ON m.representation_id = d.id
             WHERE rc.run_id = ?1 AND (?2 IS NULL OR s.id > ?2)
             ORDER BY s.id LIMIT ?3",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(
            params![run_id.to_string(), after_subject_id, i64::from(limit)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<Vec<u8>>>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, Option<String>>(21)?,
                ))
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(
                subject,
                content,
                kind,
                display_name,
                virtual_path,
                mime,
                metadata,
                size,
                chars,
                truncated,
                format,
                status,
                representation_error,
                root,
                raw_relative,
                relative_path,
                file_name,
                context,
                title,
                mail_subject,
                mail_from,
                mail_to,
            )| {
                let relative_for_display = raw_relative
                    .as_deref()
                    .map(RawPath::from_blob)
                    .transpose()?
                    .map(|path| path.to_os_string())
                    .unwrap_or_else(|| relative_path.clone().into());
                let representative_path = PathBuf::from(root)
                    .join(relative_for_display)
                    .display()
                    .to_string();
                Ok(IndexSubjectRow {
                    run_id,
                    subject_id: TextSubjectId::from_str(&subject)?,
                    content_id: ContentId::from_str(&content)?,
                    kind: TextSubjectKind::parse(&kind)?,
                    display_name,
                    virtual_path,
                    mime,
                    metadata: serde_json::from_str(&metadata).map_err(|error| {
                        DfError::Serialization(format!("stored subject metadata: {error}"))
                    })?,
                    size_bytes: size as u64,
                    normalized_chars: chars as u64,
                    text_truncated: truncated != 0,
                    document_format: DocumentFormat::parse(&format)?,
                    extraction_status: ExtractionStatus::parse(&status)?,
                    representation_error,
                    file_name,
                    relative_path,
                    representative_path,
                    context,
                    title,
                    mail_subject,
                    mail_from: mail_from
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|error| {
                            DfError::Serialization(format!("stored mail from: {error}"))
                        })?
                        .unwrap_or_default(),
                    mail_to: mail_to
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|error| {
                            DfError::Serialization(format!("stored mail to: {error}"))
                        })?
                        .unwrap_or_default(),
                })
            },
        )
        .collect()
}

/// Reassemble one bounded normalized text after proving it belongs to the
/// requested completed run.
pub fn load_subject_text(
    db: &Db,
    run_id: ExtractionRunId,
    subject_id: TextSubjectId,
) -> DfResult<String> {
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::Conflict(format!(
            "subject text requires completed extraction run `{run_id}`"
        )));
    }
    let belongs: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM text_subjects s
                JOIN extraction_run_contents rc
                  ON rc.representation_id = s.representation_id
                WHERE rc.run_id = ?1 AND s.id = ?2
             )",
            params![run_id.to_string(), subject_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !belongs {
        return Err(DfError::NotFound(format!(
            "subject `{subject_id}` in extraction run `{run_id}`"
        )));
    }
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT text FROM text_segments WHERE subject_id = ?1
             ORDER BY ordinal",
        )
        .map_err(db_err)?;
    let segments = stmt
        .query_map([subject_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(segments.concat())
}

fn validate_artifact_path(path: &str) -> DfResult<()> {
    let candidate = Path::new(path);
    if path.trim().is_empty()
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(DfError::Validation(
            "artifact path must be a non-empty project-relative path without traversal".to_string(),
        ));
    }
    Ok(())
}

fn load_search_index_by_id(db: &Db, id: SearchIndexId) -> DfResult<SearchIndexRecord> {
    let raw = db
        .conn()
        .query_row(
            "SELECT run_id, snapshot_id, schema_version, relative_path,
                    content_digest, documents, created_at
             FROM search_indexes WHERE id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| DfError::NotFound(format!("search index `{id}`")))?;
    Ok(SearchIndexRecord {
        id,
        run_id: ExtractionRunId::from_str(&raw.0)?,
        snapshot_id: SnapshotId::from_str(&raw.1)?,
        schema_version: raw.2,
        relative_path: raw.3,
        content_digest: raw.4,
        documents: raw.5 as u64,
        created_at: parse_stored_timestamp(&raw.6)?,
    })
}

/// Register an immutable rebuildable Tantivy index and its audit event.
pub fn register_search_index(
    db: &mut Db,
    run_id: ExtractionRunId,
    schema_version: &str,
    relative_path: &str,
    content_digest: &str,
    documents: u64,
    actor: Actor,
) -> DfResult<SearchIndexRecord> {
    if schema_version.trim().is_empty() || !is_sha256(content_digest) || documents > i64::MAX as u64
    {
        return Err(DfError::Validation(
            "invalid search-index schema, digest or document count".to_string(),
        ));
    }
    validate_artifact_path(relative_path)?;
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::Conflict(
            "search indexes can only register against completed extraction".to_string(),
        ));
    }
    let existing: Option<(String, String, i64)> = db
        .conn()
        .query_row(
            "SELECT id, relative_path, documents FROM search_indexes
             WHERE run_id = ?1 AND schema_version = ?2 AND content_digest = ?3",
            params![run_id.to_string(), schema_version, content_digest],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((id, stored_path, stored_documents)) = existing {
        if stored_path != relative_path || stored_documents != documents as i64 {
            return Err(DfError::Conflict(
                "search-index digest replay changed path or document count".to_string(),
            ));
        }
        return load_search_index_by_id(db, SearchIndexId::from_str(&id)?);
    }
    let id = SearchIndexId::new();
    let created_at = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO search_indexes
            (id, run_id, snapshot_id, schema_version, relative_path,
             content_digest, documents, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            run_id.to_string(),
            run.snapshot_id.to_string(),
            schema_version,
            relative_path,
            content_digest,
            documents as i64,
            created_at,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        run.project_id,
        EVENT_SEARCH_INDEX_BUILT,
        &serde_json::json!({
            "index_id": id.to_string(),
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "schema_version": schema_version,
            "relative_path": relative_path,
            "content_digest": content_digest,
            "documents": documents,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_search_index_by_id(db, id)
}

pub fn latest_search_index(
    db: &Db,
    run_id: ExtractionRunId,
) -> DfResult<Option<SearchIndexRecord>> {
    let id: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM search_indexes WHERE run_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT 1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    id.as_deref()
        .map(SearchIndexId::from_str)
        .transpose()?
        .map(|id| load_search_index_by_id(db, id))
        .transpose()
}

fn load_analytical_snapshot_by_id(
    db: &Db,
    id: AnalyticalSnapshotId,
) -> DfResult<AnalyticalSnapshotRecord> {
    let raw = db
        .conn()
        .query_row(
            "SELECT run_id, snapshot_id, schema_version, relative_path,
                    sha256, rows, created_at
             FROM analytical_snapshots WHERE id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| DfError::NotFound(format!("analytical snapshot `{id}`")))?;
    Ok(AnalyticalSnapshotRecord {
        id,
        run_id: ExtractionRunId::from_str(&raw.0)?,
        snapshot_id: SnapshotId::from_str(&raw.1)?,
        schema_version: raw.2,
        relative_path: raw.3,
        sha256: raw.4,
        rows: raw.5 as u64,
        created_at: parse_stored_timestamp(&raw.6)?,
    })
}

/// Register one immutable Parquet analytical snapshot and its audit event.
pub fn register_analytical_snapshot(
    db: &mut Db,
    run_id: ExtractionRunId,
    schema_version: &str,
    relative_path: &str,
    sha256: &str,
    rows: u64,
    actor: Actor,
) -> DfResult<AnalyticalSnapshotRecord> {
    if schema_version.trim().is_empty() || !is_sha256(sha256) || rows > i64::MAX as u64 {
        return Err(DfError::Validation(
            "invalid analytical schema, digest or row count".to_string(),
        ));
    }
    validate_artifact_path(relative_path)?;
    let run = load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::Conflict(
            "analytical snapshots require completed extraction".to_string(),
        ));
    }
    let existing: Option<(String, String, i64)> = db
        .conn()
        .query_row(
            "SELECT id, relative_path, rows FROM analytical_snapshots
             WHERE run_id = ?1 AND schema_version = ?2 AND sha256 = ?3",
            params![run_id.to_string(), schema_version, sha256],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((id, stored_path, stored_rows)) = existing {
        if stored_path != relative_path || stored_rows != rows as i64 {
            return Err(DfError::Conflict(
                "analytical digest replay changed path or row count".to_string(),
            ));
        }
        return load_analytical_snapshot_by_id(db, AnalyticalSnapshotId::from_str(&id)?);
    }
    let id = AnalyticalSnapshotId::new();
    let created_at = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO analytical_snapshots
            (id, run_id, snapshot_id, schema_version, relative_path,
             sha256, rows, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            run_id.to_string(),
            run.snapshot_id.to_string(),
            schema_version,
            relative_path,
            sha256,
            rows as i64,
            created_at,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        run.project_id,
        EVENT_ANALYTICAL_SNAPSHOT_BUILT,
        &serde_json::json!({
            "analytical_snapshot_id": id.to_string(),
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "schema_version": schema_version,
            "relative_path": relative_path,
            "sha256": sha256,
            "rows": rows,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_analytical_snapshot_by_id(db, id)
}

pub fn latest_analytical_snapshot(
    db: &Db,
    run_id: ExtractionRunId,
) -> DfResult<Option<AnalyticalSnapshotRecord>> {
    let id: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM analytical_snapshots WHERE run_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT 1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    id.as_deref()
        .map(AnalyticalSnapshotId::from_str)
        .transpose()?
        .map(|id| load_analytical_snapshot_by_id(db, id))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{OccurrenceId, ProfileRef, Project, SourceRoot, SourceRootId};

    struct Fixture {
        _temp: tempfile::TempDir,
        db: Db,
        project_id: ProjectId,
        profile: String,
        root_id: SourceRootId,
        snapshot_id: SnapshotId,
        contents: Vec<(ContentId, String)>,
    }

    fn seed_fixture(entries: &[(&str, char)]) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "extraction test",
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
        db.conn()
            .execute(
                "INSERT INTO snapshots (id, project_id, status, created_at)
                 VALUES (?1, ?2, 'COMPLETE', '2026-01-01T00:00:00.000Z')",
                params![snapshot.to_string(), project.id.to_string()],
            )
            .unwrap();
        let mut contents = Vec::new();
        for (index, (name, digest_char)) in entries.iter().enumerate() {
            let digest = digest_char.to_string().repeat(64);
            let content = ContentId::new();
            let occurrence = OccurrenceId::new();
            db.conn()
                .execute(
                    "INSERT INTO content_objects
                        (id, size_bytes, sha256, blake3, first_seen_snapshot,
                         hash_state, created_at)
                     VALUES (?1, 6, ?2, ?2, ?3, 'HASHED',
                             '2026-01-01T00:00:00.000Z')",
                    params![content.to_string(), digest, snapshot.to_string()],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO path_occurrences
                        (id, snapshot_id, source_root_id, relative_path,
                         parent_relative_path, file_name, normalized_name,
                         extension, size_bytes, attributes, path_length, depth,
                         fingerprint, scan_status, name_is_lossy, created_at)
                     VALUES (?1, ?2, ?3, ?4, '', ?4, ?4, 'txt', 6, 0,
                             ?5, 1, 'v1:6:none', 'OK', 0,
                             '2026-01-01T00:00:00.000Z')",
                    params![
                        occurrence.to_string(),
                        snapshot.to_string(),
                        source_root.id.to_string(),
                        name,
                        name.len() as i64,
                    ],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO occurrence_content
                        (occurrence_id, content_id, created_at)
                     VALUES (?1, ?2, '2026-01-01T00:00:00.000Z')",
                    params![occurrence.to_string(), content.to_string()],
                )
                .unwrap();
            contents.push((content, digest_char.to_string().repeat(64)));
            assert_eq!(index + 1, contents.len());
        }
        db.conn()
            .execute(
                "INSERT INTO analysis_completions
                    (snapshot_id, project_id, analysis_version, profile_id,
                     profile_sha256, summary_json, created_at)
                 VALUES (?1, ?2, 1, ?3, ?4, '{}',
                         '2026-01-01T00:00:00.000Z')",
                params![
                    snapshot.to_string(),
                    project.id.to_string(),
                    project.profile.as_str(),
                    "f".repeat(64),
                ],
            )
            .unwrap();
        Fixture {
            _temp: temp,
            db,
            project_id: project.id,
            profile: project.profile.as_str().to_string(),
            root_id: source_root.id,
            snapshot_id: snapshot,
            contents,
        }
    }

    fn spec(fixture: &Fixture) -> ExtractionRunSpec {
        let config_json = serde_json::json!({
            "max_input_bytes": 1_000_000,
            "max_text_chars": 100_000,
            "text_segment_chars": 1_000,
            "max_archive_entries": 100,
            "max_archive_entry_bytes": 100_000,
            "max_archive_total_bytes": 1_000_000,
            "max_archive_compression_ratio": 100,
            "max_archive_nesting_depth": 4,
        })
        .to_string();
        ExtractionRunSpec {
            project_id: fixture.project_id,
            snapshot_id: fixture.snapshot_id,
            extractor_version: "extractor-test-v1".to_string(),
            config_digest: sha256_hex(config_json.as_bytes()),
            config_json,
            max_input_bytes: 1_000_000,
            max_text_chars: 100_000,
            text_segment_chars: 1_000,
            max_archive_entries: 100,
            max_archive_entry_bytes: 100_000,
            max_archive_total_bytes: 1_000_000,
            max_archive_ratio: 100.0,
            max_archive_depth: 4,
        }
    }

    fn text_input(
        content_id: ContentId,
        source_sha256: &str,
        run: &ExtractionRun,
    ) -> ContentExtractionInput {
        let text = "héllo";
        let text_digest = sha256_hex(text.as_bytes());
        let representation_id = RepresentationId::new();
        let subject_id = TextSubjectId::new();
        let now = chrono::Utc::now();
        ContentExtractionInput {
            representation: DocumentRepresentation {
                id: representation_id,
                content_id,
                extractor_version: run.extractor_version.clone(),
                config_digest: run.config_digest.clone(),
                format: DocumentFormat::Text,
                mime: "text/plain".to_string(),
                status: ExtractionStatus::Extracted,
                title: Some("Greeting".to_string()),
                normalized_text_sha256: Some(text_digest.clone()),
                normalized_chars: 5,
                text_truncated: false,
                metadata: serde_json::json!({"encoding": "utf-8"}),
                error: None,
                created_at: now,
            },
            source_sha256: source_sha256.to_string(),
            subjects: vec![TextSubjectInput {
                subject: TextSubject {
                    id: subject_id,
                    representation_id,
                    kind: TextSubjectKind::Document,
                    parent_subject_id: None,
                    display_name: "greeting.txt".to_string(),
                    virtual_path: None,
                    mime: "text/plain".to_string(),
                    size_bytes: 6,
                    normalized_text_sha256: Some(text_digest.clone()),
                    normalized_chars: 5,
                    text_truncated: false,
                    metadata: serde_json::json!({}),
                    created_at: now,
                },
                segments: vec![TextSegmentInput {
                    ordinal: 0,
                    char_start: 0,
                    char_end: 5,
                    text: text.to_string(),
                    text_sha256: text_digest,
                }],
            }],
            mail_message: None,
            mail_attachments: Vec::new(),
            archive_entries: Vec::new(),
        }
    }

    #[test]
    fn run_resume_is_fully_configuration_addressed() {
        let mut fixture = seed_fixture(&[("greeting.txt", 'a')]);
        let run_spec = spec(&fixture);
        let run = start_or_resume_run(&mut fixture.db, &run_spec, Actor::Test).unwrap();
        let replay = start_or_resume_run(&mut fixture.db, &run_spec, Actor::Test).unwrap();
        assert_eq!(replay.id, run.id);
        assert_eq!(run.counters.contents_total, 1);

        let mut forged = spec(&fixture);
        forged.max_text_chars += 1;
        let error = start_or_resume_run(&mut fixture.db, &forged, Actor::Test).unwrap_err();
        assert!(matches!(error, DfError::Validation(_)));
        let starts: i64 = fixture
            .db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE event_type = ?1",
                [EVENT_CONTENT_EXTRACTION_STARTED],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(starts, 1);
    }

    #[test]
    fn extractor_versions_do_not_alias_the_same_limits_digest() {
        let mut fixture = seed_fixture(&[("greeting.txt", 'a')]);
        let first_spec = spec(&fixture);
        let first = start_or_resume_run(&mut fixture.db, &first_spec, Actor::Test).unwrap();

        let mut upgraded_spec = first_spec.clone();
        upgraded_spec.extractor_version = "extractor-test-v2".to_string();
        let upgraded = start_or_resume_run(&mut fixture.db, &upgraded_spec, Actor::Test).unwrap();

        assert_ne!(first.id, upgraded.id);
        assert_eq!(first.config_digest, upgraded.config_digest);
        assert_ne!(first.extractor_version, upgraded.extractor_version);
    }

    #[test]
    fn content_commit_is_atomic_and_rebuild_inputs_round_trip() {
        let mut fixture = seed_fixture(&[("greeting.txt", 'a')]);
        let run_spec = spec(&fixture);
        let run = start_or_resume_run(&mut fixture.db, &run_spec, Actor::Test).unwrap();
        assert!(complete_run(&mut fixture.db, run.id, Actor::Test).is_err());
        let (content, source_sha) = fixture.contents[0].clone();
        let mut invalid = text_input(content, &source_sha, &run);
        invalid.subjects[0].segments[0].text_sha256 = "0".repeat(64);
        assert!(persist_content_result(&mut fixture.db, run.id, &invalid).is_err());
        let representations: i64 = fixture
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM document_representations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            representations, 0,
            "validation must precede the transaction"
        );

        let input = text_input(content, &source_sha, &run);
        let id = persist_content_result(&mut fixture.db, run.id, &input).unwrap();
        assert_eq!(
            persist_content_result(&mut fixture.db, run.id, &input).unwrap(),
            id
        );
        assert!(pending_content_sources_after(&fixture.db, run.id, None, 10)
            .unwrap()
            .is_empty());
        let completed = complete_run(&mut fixture.db, run.id, Actor::Test).unwrap();
        assert_eq!(completed.counters.extracted, 1);
        assert_eq!(completed.counters.text_subjects, 1);
        assert_eq!(completed.counters.text_segments, 1);

        let subjects = index_subjects_after(&fixture.db, run.id, None, 10).unwrap();
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].context, "NEUTRAL");
        assert_eq!(subjects[0].normalized_chars, 5);
        assert_eq!(
            load_subject_text(&fixture.db, run.id, subjects[0].subject_id).unwrap(),
            "héllo"
        );

        let index = register_search_index(
            &mut fixture.db,
            run.id,
            "tantivy-v1",
            "indexes/run/index",
            &"c".repeat(64),
            1,
            Actor::Test,
        )
        .unwrap();
        let replay = register_search_index(
            &mut fixture.db,
            run.id,
            "tantivy-v1",
            "indexes/run/index",
            &"c".repeat(64),
            1,
            Actor::Test,
        )
        .unwrap();
        assert_eq!(replay.id, index.id);
        let analytical = register_analytical_snapshot(
            &mut fixture.db,
            run.id,
            "parquet-v1",
            "analytics/run/subjects.parquet",
            &"d".repeat(64),
            1,
            Actor::Test,
        )
        .unwrap();
        assert_eq!(
            latest_analytical_snapshot(&fixture.db, run.id)
                .unwrap()
                .unwrap()
                .id,
            analytical.id
        );
        assert!(register_search_index(
            &mut fixture.db,
            run.id,
            "tantivy-v2",
            "../escape",
            &"e".repeat(64),
            1,
            Actor::Test,
        )
        .is_err());
        assert!(fixture
            .db
            .conn()
            .execute(
                "UPDATE document_representations SET title = title WHERE id = ?1",
                [id.to_string()],
            )
            .is_err());
    }

    fn add_snapshot_for_content(fixture: &mut Fixture, content_id: ContentId) -> SnapshotId {
        let snapshot = SnapshotId::new();
        let occurrence = OccurrenceId::new();
        fixture
            .db
            .conn()
            .execute(
                "INSERT INTO snapshots (id, project_id, status, created_at)
                 VALUES (?1, ?2, 'COMPLETE', '2026-02-01T00:00:00.000Z')",
                params![snapshot.to_string(), fixture.project_id.to_string()],
            )
            .unwrap();
        fixture
            .db
            .conn()
            .execute(
                "INSERT INTO path_occurrences
                    (id, snapshot_id, source_root_id, relative_path,
                     parent_relative_path, file_name, normalized_name,
                     extension, size_bytes, attributes, path_length, depth,
                     fingerprint, scan_status, name_is_lossy, created_at)
                 VALUES (?1, ?2, ?3, 'copy.txt', '', 'copy.txt', 'copy.txt',
                         'txt', 6, 0, 8, 1, 'v1:6:none', 'OK', 0,
                         '2026-02-01T00:00:00.000Z')",
                params![
                    occurrence.to_string(),
                    snapshot.to_string(),
                    fixture.root_id.to_string(),
                ],
            )
            .unwrap();
        fixture
            .db
            .conn()
            .execute(
                "INSERT INTO occurrence_content
                    (occurrence_id, content_id, created_at)
                 VALUES (?1, ?2, '2026-02-01T00:00:00.000Z')",
                params![occurrence.to_string(), content_id.to_string()],
            )
            .unwrap();
        fixture
            .db
            .conn()
            .execute(
                "INSERT INTO analysis_completions
                    (snapshot_id, project_id, analysis_version, profile_id,
                     profile_sha256, summary_json, created_at)
                 VALUES (?1, ?2, 1, ?3, ?4, '{}',
                         '2026-02-01T00:00:00.000Z')",
                params![
                    snapshot.to_string(),
                    fixture.project_id.to_string(),
                    fixture.profile,
                    "f".repeat(64),
                ],
            )
            .unwrap();
        snapshot
    }

    #[test]
    fn immutable_representation_is_reused_across_snapshots() {
        let mut fixture = seed_fixture(&[("original.txt", 'a')]);
        let first_spec = spec(&fixture);
        let first_run = start_or_resume_run(&mut fixture.db, &first_spec, Actor::Test).unwrap();
        let (content, digest) = fixture.contents[0].clone();
        let input = text_input(content, &digest, &first_run);
        let representation = persist_content_result(&mut fixture.db, first_run.id, &input).unwrap();
        complete_run(&mut fixture.db, first_run.id, Actor::Test).unwrap();

        let second_snapshot = add_snapshot_for_content(&mut fixture, content);
        let mut second_spec = spec(&fixture);
        second_spec.snapshot_id = second_snapshot;
        let second_run = start_or_resume_run(&mut fixture.db, &second_spec, Actor::Test).unwrap();
        let sources = pending_content_sources_after(&fixture.db, second_run.id, None, 10).unwrap();
        assert_eq!(sources[0].reusable_representation_id, Some(representation));
        assert_eq!(
            bind_reusable_representation(&mut fixture.db, second_run.id, content).unwrap(),
            Some(representation)
        );
        complete_run(&mut fixture.db, second_run.id, Actor::Test).unwrap();
        let count: i64 = fixture
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM document_representations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn zip_entry_lineage_and_completion_counters_are_transactional() {
        let mut fixture = seed_fixture(&[("archive.zip", 'a')]);
        let run_spec = spec(&fixture);
        let run = start_or_resume_run(&mut fixture.db, &run_spec, Actor::Test).unwrap();
        let (content, source_sha256) = fixture.contents[0].clone();
        let representation_id = RepresentationId::new();
        let document_id = TextSubjectId::new();
        let entry_subject_id = TextSubjectId::new();
        let empty_digest = sha256_hex(b"");
        let entry_text = "entry";
        let entry_digest = sha256_hex(entry_text.as_bytes());
        let now = chrono::Utc::now();
        let input = ContentExtractionInput {
            representation: DocumentRepresentation {
                id: representation_id,
                content_id: content,
                extractor_version: run.extractor_version.clone(),
                config_digest: run.config_digest.clone(),
                format: DocumentFormat::Zip,
                mime: "application/zip".to_string(),
                status: ExtractionStatus::Extracted,
                title: None,
                normalized_text_sha256: Some(empty_digest.clone()),
                normalized_chars: 0,
                text_truncated: false,
                metadata: serde_json::json!({}),
                error: None,
                created_at: now,
            },
            source_sha256,
            subjects: vec![
                TextSubjectInput {
                    subject: TextSubject {
                        id: document_id,
                        representation_id,
                        kind: TextSubjectKind::Document,
                        parent_subject_id: None,
                        display_name: "archive.zip".to_string(),
                        virtual_path: None,
                        mime: "application/zip".to_string(),
                        size_bytes: 6,
                        normalized_text_sha256: Some(empty_digest),
                        normalized_chars: 0,
                        text_truncated: false,
                        metadata: serde_json::json!({}),
                        created_at: now,
                    },
                    segments: vec![],
                },
                TextSubjectInput {
                    subject: TextSubject {
                        id: entry_subject_id,
                        representation_id,
                        kind: TextSubjectKind::ArchiveEntry,
                        parent_subject_id: Some(document_id),
                        display_name: "inside.txt".to_string(),
                        virtual_path: Some("inside.txt".to_string()),
                        mime: "text/plain".to_string(),
                        size_bytes: 5,
                        normalized_text_sha256: Some(entry_digest.clone()),
                        normalized_chars: 5,
                        text_truncated: false,
                        metadata: serde_json::json!({}),
                        created_at: now,
                    },
                    segments: vec![TextSegmentInput {
                        ordinal: 0,
                        char_start: 0,
                        char_end: 5,
                        text: entry_text.to_string(),
                        text_sha256: entry_digest,
                    }],
                },
            ],
            mail_message: None,
            mail_attachments: vec![],
            archive_entries: vec![ArchiveEntry {
                id: df_domain::ArchiveEntryId::new(),
                representation_id,
                subject_id: Some(entry_subject_id),
                ordinal: 0,
                virtual_path: "inside.txt".to_string(),
                compressed_bytes: 4,
                size_bytes: 5,
                crc32: 42,
                encrypted: false,
                directory: false,
                sha256: Some("e".repeat(64)),
                extraction_status: ExtractionStatus::Extracted,
                created_at: now,
            }],
        };
        persist_content_result(&mut fixture.db, run.id, &input).unwrap();
        let completed = complete_run(&mut fixture.db, run.id, Actor::Test).unwrap();
        assert_eq!(completed.counters.archive_entries, 1);
        assert_eq!(completed.counters.text_subjects, 2);
        assert_eq!(completed.counters.text_segments, 1);
    }

    #[test]
    fn eml_metadata_attachments_and_parent_first_threads_are_sealed_together() {
        let mut fixture = seed_fixture(&[("root.eml", 'a'), ("reply.eml", 'c')]);
        let run_spec = spec(&fixture);
        let run = start_or_resume_run(&mut fixture.db, &run_spec, Actor::Test).unwrap();
        let mut representations = Vec::new();
        for (index, (content, source_sha)) in fixture.contents.clone().into_iter().enumerate() {
            let mut input = text_input(content, &source_sha, &run);
            input.representation.format = DocumentFormat::Eml;
            input.representation.mime = "message/rfc822".to_string();
            let representation_id = input.representation.id;
            input.mail_message = Some(MailMessage {
                representation_id,
                message_id: Some(format!("<message-{index}@example.test>")),
                in_reply_to: if index == 0 {
                    vec![]
                } else {
                    vec!["<message-0@example.test>".to_string()]
                },
                references: if index == 0 {
                    vec![]
                } else {
                    vec!["<message-0@example.test>".to_string()]
                },
                from: vec!["sender@example.test".to_string()],
                to: vec!["recipient@example.test".to_string()],
                cc: vec![],
                sent_at: Some(format!("2026-01-0{}T00:00:00Z", index + 1)),
                subject: Some(if index == 0 {
                    "Project".to_string()
                } else {
                    "Re: Project".to_string()
                }),
                normalized_subject: Some("project".to_string()),
                body_sha256: input.representation.normalized_text_sha256.clone(),
            });
            if index == 0 {
                let attachment_subject_id = TextSubjectId::new();
                let now = chrono::Utc::now();
                input.subjects.push(TextSubjectInput {
                    subject: TextSubject {
                        id: attachment_subject_id,
                        representation_id,
                        kind: TextSubjectKind::MailAttachment,
                        parent_subject_id: Some(input.subjects[0].subject.id),
                        display_name: "evidence.bin".to_string(),
                        virtual_path: Some("attachments/evidence.bin".to_string()),
                        mime: "application/octet-stream".to_string(),
                        size_bytes: 3,
                        normalized_text_sha256: Some(sha256_hex(b"")),
                        normalized_chars: 0,
                        text_truncated: false,
                        metadata: serde_json::json!({}),
                        created_at: now,
                    },
                    segments: vec![],
                });
                input.mail_attachments.push(MailAttachment {
                    id: df_domain::MailAttachmentId::new(),
                    representation_id,
                    subject_id: attachment_subject_id,
                    ordinal: 0,
                    file_name: "evidence.bin".to_string(),
                    mime: "application/octet-stream".to_string(),
                    size_bytes: 3,
                    sha256: "e".repeat(64),
                    extraction_status: ExtractionStatus::Unsupported,
                    created_at: now,
                });
            }
            representations.push(persist_content_result(&mut fixture.db, run.id, &input).unwrap());
        }
        let thread = MailThreadInput {
            id: MailThreadId::new(),
            root_message_id: Some("<message-0@example.test>".to_string()),
            normalized_subject: Some("project".to_string()),
            members: vec![
                MailThreadMemberInput {
                    representation_id: representations[0],
                    parent_representation_id: None,
                },
                MailThreadMemberInput {
                    representation_id: representations[1],
                    parent_representation_id: Some(representations[0]),
                },
            ],
        };
        assert_eq!(
            persist_mail_threads(
                &mut fixture.db,
                run.id,
                std::slice::from_ref(&thread),
                Actor::Test
            )
            .unwrap(),
            1
        );
        assert_eq!(
            persist_mail_threads(
                &mut fixture.db,
                run.id,
                std::slice::from_ref(&thread),
                Actor::Test
            )
            .unwrap(),
            1
        );
        let completed = complete_run(&mut fixture.db, run.id, Actor::Test).unwrap();
        assert_eq!(completed.counters.mail_messages, 2);
        assert_eq!(completed.counters.mail_threads, 1);
        assert_eq!(completed.counters.mail_attachments, 1);
        assert!(fixture
            .db
            .conn()
            .execute(
                "DELETE FROM mail_thread_members WHERE thread_id = ?1",
                [thread.id.to_string()],
            )
            .is_err());
    }

    #[test]
    fn sql_triggers_reject_wrong_config_and_early_artifacts() {
        let mut fixture = seed_fixture(&[("source.txt", 'a')]);
        let run_spec = spec(&fixture);
        let run = start_or_resume_run(&mut fixture.db, &run_spec, Actor::Test).unwrap();
        let (content, source_sha) = fixture.contents[0].clone();
        let error = fixture
            .db
            .conn()
            .execute(
                "INSERT INTO document_representations
                    (id, content_id, extractor_version, config_digest, format,
                     mime, status, normalized_chars, text_truncated,
                     metadata_json, source_sha256, created_at)
                 VALUES (?1, ?2, 'wrong-version', ?3, 'TXT', 'text/plain',
                         'EXTRACTED', 0, 0, '{}', ?4,
                         '2026-01-01T00:00:00.000Z')",
                params![
                    RepresentationId::new().to_string(),
                    content.to_string(),
                    run.config_digest,
                    source_sha,
                ],
            )
            .unwrap_err();
        assert!(error.to_string().contains("canonical content"));

        let error = fixture
            .db
            .conn()
            .execute(
                "INSERT INTO search_indexes
                    (id, run_id, snapshot_id, schema_version, relative_path,
                     content_digest, documents, created_at)
                 VALUES (?1, ?2, ?3, 'v1', 'index/early', ?4, 0,
                         '2026-01-01T00:00:00.000Z')",
                params![
                    SearchIndexId::new().to_string(),
                    run.id.to_string(),
                    run.snapshot_id.to_string(),
                    "d".repeat(64),
                ],
            )
            .unwrap_err();
        assert!(error.to_string().contains("completed matching extraction"));
    }
}
