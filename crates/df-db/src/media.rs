//! Persistence boundary for media intelligence evidence (Milestone 0.5).
//!
//! Runs are configuration-addressed and sealed on completion; per-content
//! analyses and review relations are append-only. The engine crate never
//! obtains a raw connection: everything crosses this module as typed rows
//! or serialized engine contracts.

use std::path::PathBuf;
use std::str::FromStr;

use df_domain::{
    Actor, ContentId, MediaEvidenceId, MediaRelationId, MediaRelationKind, MediaRun,
    MediaRunCounters, MediaRunId, MediaRunStatus, ProjectId, RawPath, SnapshotId,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension};

use crate::repository::{append_event, parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

pub const EVENT_MEDIA_STARTED: &str = "MEDIA_ANALYSIS_STARTED";
pub const EVENT_MEDIA_COMPLETED: &str = "MEDIA_ANALYSIS_COMPLETED";

/// Fully expanded, immutable identity of a media run.
#[derive(Debug, Clone)]
pub struct MediaRunSpec {
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub contract_version: String,
    pub config_digest: String,
    pub config_json: String,
}

/// Stable representative used to reopen one unique content from the source.
#[derive(Debug, Clone)]
pub struct MediaContentSource {
    pub content_id: ContentId,
    pub size_bytes: u64,
    pub sha256: String,
    pub root_path: PathBuf,
    pub raw_relative_path: Option<RawPath>,
    pub relative_path: String,
    pub fingerprint: String,
    pub extension: String,
}

/// One computed evidence row ready to persist. Kind and status use the SQL
/// vocabulary (`IMAGE`/`AUDIO`/`VIDEO`, `EXTRACTED`/`LIMITED`/`FAILED`);
/// `analysis_json` is the full serialized engine contract.
#[derive(Debug, Clone)]
pub struct MediaEvidenceInput {
    pub content_id: ContentId,
    pub media_kind: String,
    pub status: String,
    pub analysis_json: String,
    pub failure_code: Option<String>,
}

/// One review relation ready to persist; `content_a < content_b` textually.
#[derive(Debug, Clone)]
pub struct MediaRelationInput {
    pub content_a: ContentId,
    pub content_b: ContentId,
    pub kind: MediaRelationKind,
    pub score_millionths: u32,
    pub evidence_json: String,
}

/// A sealed relation joined with a display path for each side.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MediaRelationView {
    pub relation: String,
    pub score_millionths: u32,
    pub content_a: String,
    pub content_b: String,
    pub path_a: Option<String>,
    pub path_b: Option<String>,
    pub evidence: serde_json::Value,
}

type StoredRunRow = (
    String,         // 0 id
    String,         // 1 project_id
    String,         // 2 snapshot_id
    String,         // 3 status
    String,         // 4 contract_version
    String,         // 5 config_digest
    String,         // 6 config_json
    i64,            // 7 contents_total
    i64,            // 8 contents_analyzed
    i64,            // 9 contents_limited
    i64,            // 10 contents_failed
    i64,            // 11 pairs_compared
    i64,            // 12 pair_cap_reached
    i64,            // 13 relations_total
    Option<String>, // 14 error
    String,         // 15 started_at
    Option<String>, // 16 finished_at
);

const RUN_COLUMNS: &str = "id, project_id, snapshot_id, status, contract_version, config_digest,
     config_json, contents_total, contents_analyzed, contents_limited,
     contents_failed, pairs_compared, pair_cap_reached, relations_total,
     error, started_at, finished_at";

fn run_from_stored(row: StoredRunRow) -> DfResult<MediaRun> {
    let (
        id,
        project_id,
        snapshot_id,
        status,
        contract_version,
        config_digest,
        config_json,
        contents_total,
        contents_analyzed,
        contents_limited,
        contents_failed,
        pairs_compared,
        pair_cap_reached,
        relations_total,
        error,
        started_at,
        finished_at,
    ) = row;
    Ok(MediaRun {
        id: MediaRunId::from_str(&id)?,
        project_id: ProjectId::from_str(&project_id)?,
        snapshot_id: SnapshotId::from_str(&snapshot_id)?,
        status: MediaRunStatus::parse(&status)?,
        contract_version,
        config_digest,
        config: serde_json::from_str(&config_json)
            .map_err(|error| DfError::Validation(format!("stored media config: {error}")))?,
        counters: MediaRunCounters {
            contents_total: contents_total as u64,
            contents_analyzed: contents_analyzed as u64,
            contents_limited: contents_limited as u64,
            contents_failed: contents_failed as u64,
            pairs_compared: pairs_compared as u64,
            relations_total: relations_total as u64,
        },
        pair_cap_reached: pair_cap_reached != 0,
        error,
        started_at: parse_stored_timestamp(&started_at)?,
        finished_at: finished_at
            .as_deref()
            .map(parse_stored_timestamp)
            .transpose()?,
    })
}

fn map_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRunRow> {
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
}

fn load_run_by_id(db: &Db, run_id: MediaRunId) -> DfResult<MediaRun> {
    let row = db
        .conn()
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM media_runs WHERE id = ?1"),
            [run_id.to_string()],
            map_run_row,
        )
        .map_err(db_err)?;
    run_from_stored(row)
}

/// Open a new run or return the existing one with the same configuration
/// digest for this snapshot. Requires the snapshot's structural analysis to
/// be complete, like similarity: media evidence sits on top of a stable
/// inventory, never a partial one.
pub fn start_or_resume_run(db: &mut Db, spec: &MediaRunSpec, actor: Actor) -> DfResult<MediaRun> {
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
            "media analysis requires a completed, structurally analysed snapshot".to_string(),
        ));
    }

    // Compare the raw stored text, never a JSON round-trip: `Value` sorts
    // object keys and would break byte-exact digest verification.
    let existing: Option<(String, String, String)> = db
        .conn()
        .query_row(
            "SELECT id, contract_version, config_json FROM media_runs
             WHERE snapshot_id = ?1 AND config_digest = ?2",
            params![spec.snapshot_id.to_string(), spec.config_digest],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((id, contract_version, config_json)) = existing {
        if contract_version != spec.contract_version || config_json != spec.config_json {
            return Err(DfError::Conflict(format!(
                "media run `{id}` does not match the digest-addressed configuration"
            )));
        }
        let run = load_run_by_id(db, MediaRunId::from_str(&id)?)?;
        if run.status == MediaRunStatus::Failed {
            return Err(DfError::Conflict(format!(
                "media run `{}` failed; use a new configuration digest",
                run.id
            )));
        }
        return Ok(run);
    }

    let run_id = MediaRunId::new();
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO media_runs
            (id, project_id, snapshot_id, status, contract_version,
             config_digest, config_json, started_at, created_at)
         VALUES (?1, ?2, ?3, 'RUNNING', ?4, ?5, ?6, ?7, ?7)",
        params![
            run_id.to_string(),
            spec.project_id.to_string(),
            spec.snapshot_id.to_string(),
            spec.contract_version,
            spec.config_digest,
            spec.config_json,
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        spec.project_id,
        EVENT_MEDIA_STARTED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": spec.snapshot_id.to_string(),
            "contract_version": spec.contract_version,
            "config_digest": spec.config_digest,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run_by_id(db, run_id)
}

/// Page through unique media-typed contents that this run has not analysed
/// yet. The extension filter is provided by the caller and is part of the
/// run's serialized configuration.
pub fn media_sources_after(
    db: &Db,
    run_id: MediaRunId,
    snapshot_id: SnapshotId,
    extensions: &[&str],
    after_content_id: Option<&str>,
    limit: u32,
) -> DfResult<Vec<MediaContentSource>> {
    if limit == 0 || extensions.is_empty() {
        return Ok(Vec::new());
    }
    // Extensions are bound as one JSON array parameter to keep the SQL
    // static; they come from the engine's own constant tables.
    let extension_json = serde_json::to_string(extensions)
        .map_err(|error| DfError::Validation(format!("extension list: {error}")))?;
    let mut stmt = db
        .conn()
        .prepare(
            "WITH ranked AS (
                SELECT c.id AS content_id, c.size_bytes, c.sha256,
                       r.absolute_path, o.raw_relative_path, o.relative_path,
                       o.fingerprint, o.extension,
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
                  AND o.extension IN (SELECT value FROM json_each(?2))
                  AND (?3 IS NULL OR c.id > ?3)
             )
             SELECT content_id, size_bytes, sha256, absolute_path,
                    raw_relative_path, relative_path, fingerprint, extension
             FROM ranked
             WHERE rn = 1
               AND NOT EXISTS (
                   SELECT 1 FROM media_evidence e
                   WHERE e.run_id = ?4 AND e.content_id = ranked.content_id
               )
             ORDER BY content_id LIMIT ?5",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(
            params![
                snapshot_id.to_string(),
                extension_json,
                after_content_id,
                run_id.to_string(),
                limit as i64
            ],
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
                ))
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(id, size, sha, root, raw_path, relative, fingerprint, extension)| {
                Ok(MediaContentSource {
                    content_id: ContentId::from_str(&id)?,
                    size_bytes: size as u64,
                    sha256: sha,
                    root_path: PathBuf::from(root),
                    raw_relative_path: raw_path.as_deref().map(RawPath::from_blob).transpose()?,
                    relative_path: relative,
                    fingerprint,
                    extension,
                })
            },
        )
        .collect()
}

/// Persist a batch of per-content analyses — one transaction.
pub fn record_media_evidence(
    db: &mut Db,
    run_id: MediaRunId,
    snapshot_id: SnapshotId,
    batch: &[MediaEvidenceInput],
) -> DfResult<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    for input in batch {
        tx.prepare_cached(
            "INSERT INTO media_evidence
                (id, run_id, snapshot_id, content_id, media_kind, status,
                 analysis_json, failure_code, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(db_err)?
        .execute(params![
            MediaEvidenceId::new().to_string(),
            run_id.to_string(),
            snapshot_id.to_string(),
            input.content_id.to_string(),
            input.media_kind,
            input.status,
            input.analysis_json,
            input.failure_code,
            to_stored_timestamp(chrono::Utc::now()),
        ])
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Drop run-scoped relations so an interrupted comparison phase rebuilds
/// deterministically. Rejected by trigger once the run is sealed.
pub fn reset_run_relations(db: &mut Db, run_id: MediaRunId) -> DfResult<()> {
    db.conn()
        .execute(
            "DELETE FROM media_relations WHERE run_id = ?1",
            [run_id.to_string()],
        )
        .map_err(db_err)?;
    Ok(())
}

/// All successfully extracted analyses of one kind, ordered by content id so
/// pair enumeration is deterministic.
pub fn extracted_analyses(
    db: &Db,
    run_id: MediaRunId,
    media_kind: &str,
) -> DfResult<Vec<(ContentId, String)>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT content_id, analysis_json FROM media_evidence
             WHERE run_id = ?1 AND media_kind = ?2 AND status = 'EXTRACTED'
             ORDER BY content_id",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(params![run_id.to_string(), media_kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(|(id, json)| Ok((ContentId::from_str(&id)?, json)))
        .collect()
}

/// Persist a batch of review relations — one transaction.
pub fn record_media_relations(
    db: &mut Db,
    run_id: MediaRunId,
    snapshot_id: SnapshotId,
    batch: &[MediaRelationInput],
) -> DfResult<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    for input in batch {
        if input.content_a.to_string() >= input.content_b.to_string() {
            return Err(DfError::Validation(
                "media relations must order content_a < content_b".to_string(),
            ));
        }
        tx.prepare_cached(
            "INSERT INTO media_relations
                (id, run_id, snapshot_id, content_a, content_b, relation,
                 score_millionths, evidence_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(db_err)?
        .execute(params![
            MediaRelationId::new().to_string(),
            run_id.to_string(),
            snapshot_id.to_string(),
            input.content_a.to_string(),
            input.content_b.to_string(),
            input.kind.as_str(),
            input.score_millionths as i64,
            input.evidence_json,
            to_stored_timestamp(chrono::Utc::now()),
        ])
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Seal the run: counters must match the evidence rows (enforced again by
/// trigger) and the audit event commits in the same transaction.
pub fn complete_run(
    db: &mut Db,
    run_id: MediaRunId,
    counters: MediaRunCounters,
    pair_cap_reached: bool,
    actor: Actor,
) -> DfResult<MediaRun> {
    let run = load_run_by_id(db, run_id)?;
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE media_runs
         SET status = 'COMPLETED', contents_total = ?2, contents_analyzed = ?3,
             contents_limited = ?4, contents_failed = ?5, pairs_compared = ?6,
             pair_cap_reached = ?7, relations_total = ?8, finished_at = ?9
         WHERE id = ?1",
        params![
            run_id.to_string(),
            counters.contents_total as i64,
            counters.contents_analyzed as i64,
            counters.contents_limited as i64,
            counters.contents_failed as i64,
            counters.pairs_compared as i64,
            pair_cap_reached as i64,
            counters.relations_total as i64,
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        run.project_id,
        EVENT_MEDIA_COMPLETED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "contents_total": counters.contents_total,
            "contents_analyzed": counters.contents_analyzed,
            "contents_limited": counters.contents_limited,
            "contents_failed": counters.contents_failed,
            "pairs_compared": counters.pairs_compared,
            "pair_cap_reached": pair_cap_reached,
            "relations_total": counters.relations_total,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run_by_id(db, run_id)
}

/// Evidence row counts of one run: (total, extracted, limited, failed).
pub fn evidence_counters(db: &Db, run_id: MediaRunId) -> DfResult<(u64, u64, u64, u64)> {
    db.conn()
        .query_row(
            "SELECT COUNT(*),
                    COUNT(*) FILTER (WHERE status = 'EXTRACTED'),
                    COUNT(*) FILTER (WHERE status = 'LIMITED'),
                    COUNT(*) FILTER (WHERE status = 'FAILED')
             FROM media_evidence WHERE run_id = ?1",
            [run_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                ))
            },
        )
        .map_err(db_err)
}

/// Latest sealed run of the project, if any.
pub fn latest_completed_run(db: &Db, project_id: ProjectId) -> DfResult<Option<MediaRun>> {
    let row = db
        .conn()
        .query_row(
            &format!(
                "SELECT {RUN_COLUMNS} FROM media_runs
                 WHERE project_id = ?1 AND status = 'COMPLETED'
                 ORDER BY created_at DESC, id DESC LIMIT 1"
            ),
            [project_id.to_string()],
            map_run_row,
        )
        .optional()
        .map_err(db_err)?;
    row.map(run_from_stored).transpose()
}

/// Sealed relations with a representative display path per side.
pub fn list_media_relations(
    db: &Db,
    run_id: MediaRunId,
    limit: u32,
) -> DfResult<Vec<MediaRelationView>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT m.relation, m.score_millionths, m.content_a, m.content_b,
                    m.evidence_json,
                    (SELECT o.relative_path
                     FROM occurrence_content oc
                     JOIN path_occurrences o ON o.id = oc.occurrence_id
                     WHERE oc.content_id = m.content_a AND o.snapshot_id = m.snapshot_id
                     ORDER BY o.relative_path LIMIT 1),
                    (SELECT o.relative_path
                     FROM occurrence_content oc
                     JOIN path_occurrences o ON o.id = oc.occurrence_id
                     WHERE oc.content_id = m.content_b AND o.snapshot_id = m.snapshot_id
                     ORDER BY o.relative_path LIMIT 1)
             FROM media_relations m
             WHERE m.run_id = ?1
             ORDER BY m.score_millionths DESC, m.content_a, m.content_b
             LIMIT ?2",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map(params![run_id.to_string(), limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(relation, score, content_a, content_b, evidence, path_a, path_b)| {
                Ok(MediaRelationView {
                    relation,
                    score_millionths: score as u32,
                    content_a,
                    content_b,
                    path_a,
                    path_b,
                    evidence: serde_json::from_str(&evidence).map_err(|error| {
                        DfError::Validation(format!("stored media evidence: {error}"))
                    })?,
                })
            },
        )
        .collect()
}
