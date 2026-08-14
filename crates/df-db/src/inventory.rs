//! Persistence for the inventory pipeline: scan runs, folders, occurrences,
//! content objects and the resumable hash queue (Milestone 0.1).
//!
//! Same contract as [`crate::repository`]: every mutation is transactional
//! and the audit event that describes it commits in the same transaction.

use std::path::PathBuf;
use std::str::FromStr;

use df_domain::{
    Actor, ContentId, ContentObject, FolderRecord, HashJobId, HashState, OccurrenceId,
    PathOccurrence, ProjectId, ScanCounters, ScanRun, ScanRunId, ScanRunStatus, Snapshot,
    SnapshotId, SnapshotStatus, SourceRootId,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::repository::{append_event, parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

/// Event types emitted by the inventory pipeline.
pub const EVENT_SCAN_STARTED: &str = "SCAN_STARTED";
pub const EVENT_SCAN_COMPLETED: &str = "SCAN_COMPLETED";
pub const EVENT_SCAN_CANCELLED: &str = "SCAN_CANCELLED";
pub const EVENT_SCAN_FAILED: &str = "SCAN_FAILED";
pub const EVENT_HASH_STARTED: &str = "HASH_STARTED";
pub const EVENT_HASH_COMPLETED: &str = "HASH_COMPLETED";
pub const EVENT_HASH_PAUSED: &str = "HASH_PAUSED";

/// Folders and occurrences accumulated by the walker between two commits.
#[derive(Debug, Default)]
pub struct ScanBatch {
    pub folders: Vec<FolderRecord>,
    pub occurrences: Vec<PathOccurrence>,
}

impl ScanBatch {
    pub fn len(&self) -> usize {
        self.folders.len() + self.occurrences.len()
    }

    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.occurrences.is_empty()
    }
}

/// Open a snapshot and its scan run, emitting `SCAN_STARTED` — one tx.
pub fn start_scan(
    db: &mut Db,
    project_id: ProjectId,
    actor: Actor,
) -> DfResult<(Snapshot, ScanRun)> {
    let mut snapshot = Snapshot::new(project_id);
    snapshot.status = SnapshotStatus::Capturing;
    let run = ScanRun::new(project_id, snapshot.id);

    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO snapshots (id, project_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            snapshot.id.to_string(),
            snapshot.project_id.to_string(),
            snapshot.status.as_str(),
            to_stored_timestamp(snapshot.created_at),
        ],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO scan_runs
            (id, project_id, snapshot_id, status, started_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            run.id.to_string(),
            run.project_id.to_string(),
            run.snapshot_id.to_string(),
            run.status.as_str(),
            to_stored_timestamp(run.started_at),
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    let payload = serde_json::json!({
        "snapshot_id": snapshot.id.to_string(),
        "scan_run_id": run.id.to_string(),
    });
    append_event(&tx, project_id, EVENT_SCAN_STARTED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok((snapshot, run))
}

fn insert_folder(tx: &Transaction<'_>, folder: &FolderRecord) -> DfResult<()> {
    tx.execute(
        "INSERT INTO folders
            (id, snapshot_id, source_root_id, relative_path, parent_relative_path,
             name, normalized_name, depth, status, error, raw_relative_path,
             created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            folder.id.to_string(),
            folder.snapshot_id.to_string(),
            folder.source_root_id.to_string(),
            folder.relative_path,
            folder.parent_relative_path,
            folder.name,
            folder.normalized_name,
            folder.depth as i64,
            folder.status.as_str(),
            folder.error,
            folder.raw_relative_path.as_ref().map(|r| r.to_blob()),
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

fn insert_occurrence(tx: &Transaction<'_>, occ: &PathOccurrence) -> DfResult<()> {
    tx.execute(
        "INSERT INTO path_occurrences
            (id, snapshot_id, source_root_id, relative_path, parent_relative_path,
             file_name, normalized_name, extension, size_bytes, created_at_fs,
             modified_at_fs, attributes, path_length, depth, fingerprint,
             scan_status, error, name_is_lossy, raw_relative_path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            occ.id.to_string(),
            occ.snapshot_id.to_string(),
            occ.source_root_id.to_string(),
            occ.relative_path,
            occ.parent_relative_path,
            occ.file_name,
            occ.normalized_name,
            occ.extension,
            occ.size_bytes as i64,
            occ.created_at_fs.map(to_stored_timestamp),
            occ.modified_at_fs.map(to_stored_timestamp),
            occ.attributes as i64,
            occ.path_length as i64,
            occ.depth as i64,
            occ.fingerprint,
            occ.scan_status.as_str(),
            occ.error,
            occ.name_is_lossy as i64,
            occ.raw_relative_path.as_ref().map(|r| r.to_blob()),
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Persist one walker batch and refresh the run counters — one tx.
pub fn insert_scan_batch(
    db: &mut Db,
    run_id: ScanRunId,
    batch: &ScanBatch,
    counters: ScanCounters,
) -> DfResult<()> {
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    for folder in &batch.folders {
        insert_folder(&tx, folder)?;
    }
    for occurrence in &batch.occurrences {
        insert_occurrence(&tx, occurrence)?;
    }
    tx.execute(
        "UPDATE scan_runs
         SET files = ?1, folders = ?2, bytes = ?3, errors = ?4, reparse_points = ?5
         WHERE id = ?6",
        params![
            counters.files as i64,
            counters.folders as i64,
            counters.bytes as i64,
            counters.errors as i64,
            counters.reparse_points as i64,
            run_id.to_string(),
        ],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Close a scan run and its snapshot with the given verdict — one tx.
///
/// `COMPLETED` marks the snapshot `COMPLETE`; `CANCELLED` and `FAILED` mark
/// it `FAILED` (a partial inventory is never a valid snapshot, RFC-0001
/// rule 4). Emits the matching audit event.
pub fn finish_scan(
    db: &mut Db,
    run: &ScanRun,
    status: ScanRunStatus,
    counters: ScanCounters,
    actor: Actor,
) -> DfResult<()> {
    let (snapshot_status, event_type) = match status {
        ScanRunStatus::Completed => (SnapshotStatus::Complete, EVENT_SCAN_COMPLETED),
        ScanRunStatus::Cancelled => (SnapshotStatus::Failed, EVENT_SCAN_CANCELLED),
        ScanRunStatus::Failed => (SnapshotStatus::Failed, EVENT_SCAN_FAILED),
        ScanRunStatus::Running => {
            return Err(DfError::Validation(
                "finish_scan requires a terminal scan run status".to_string(),
            ));
        }
    };

    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE scan_runs
         SET status = ?1, files = ?2, folders = ?3, bytes = ?4, errors = ?5,
             reparse_points = ?6, finished_at = ?7
         WHERE id = ?8",
        params![
            status.as_str(),
            counters.files as i64,
            counters.folders as i64,
            counters.bytes as i64,
            counters.errors as i64,
            counters.reparse_points as i64,
            to_stored_timestamp(chrono::Utc::now()),
            run.id.to_string(),
        ],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE snapshots SET status = ?1 WHERE id = ?2",
        params![snapshot_status.as_str(), run.snapshot_id.to_string()],
    )
    .map_err(db_err)?;
    let payload = serde_json::json!({
        "snapshot_id": run.snapshot_id.to_string(),
        "scan_run_id": run.id.to_string(),
        "files": counters.files,
        "folders": counters.folders,
        "bytes": counters.bytes,
        "errors": counters.errors,
        "reparse_points": counters.reparse_points,
    });
    append_event(&tx, run.project_id, event_type, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// The most recent `COMPLETE` snapshot of a project, if any.
pub fn latest_complete_snapshot(db: &Db, project_id: ProjectId) -> DfResult<Option<Snapshot>> {
    db.conn()
        .query_row(
            "SELECT id, project_id, status, created_at FROM snapshots
             WHERE project_id = ?1 AND status = 'COMPLETE'
             ORDER BY created_at DESC, id DESC LIMIT 1",
            [project_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .map(|(id, project, status, created)| {
            Ok(Snapshot {
                id: SnapshotId::from_str(&id)?,
                project_id: ProjectId::from_str(&project)?,
                status: SnapshotStatus::parse(&status)?,
                created_at: parse_stored_timestamp(&created)?,
            })
        })
        .transpose()
}

/// Counters of what a snapshot contains, for status views and reports.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct InventorySummary {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub scan_errors: u64,
    pub reparse_points: u64,
    pub hash_pending: u64,
    pub hash_done: u64,
    pub hash_failed: u64,
    pub hash_source_changed: u64,
}

/// Aggregate the inventory of one snapshot.
pub fn inventory_summary(db: &Db, snapshot_id: SnapshotId) -> DfResult<InventorySummary> {
    let id = snapshot_id.to_string();
    let mut summary = InventorySummary::default();

    db.conn()
        .query_row(
            "SELECT
                COUNT(*) FILTER (WHERE scan_status = 'OK'),
                COALESCE(SUM(size_bytes) FILTER (WHERE scan_status = 'OK'), 0),
                COUNT(*) FILTER (WHERE scan_status = 'ERROR'),
                COUNT(*) FILTER (WHERE scan_status = 'REPARSE_NOT_FOLLOWED')
             FROM path_occurrences WHERE snapshot_id = ?1",
            [&id],
            |row| {
                summary.files = row.get::<_, i64>(0)? as u64;
                summary.bytes = row.get::<_, i64>(1)? as u64;
                summary.scan_errors = row.get::<_, i64>(2)? as u64;
                summary.reparse_points = row.get::<_, i64>(3)? as u64;
                Ok(())
            },
        )
        .map_err(db_err)?;

    db.conn()
        .query_row(
            "SELECT
                COUNT(*) FILTER (WHERE status = 'OK'),
                COUNT(*) FILTER (WHERE status = 'ERROR'),
                COUNT(*) FILTER (WHERE status = 'REPARSE_NOT_FOLLOWED')
             FROM folders WHERE snapshot_id = ?1",
            [&id],
            |row| {
                summary.folders = row.get::<_, i64>(0)? as u64;
                summary.scan_errors += row.get::<_, i64>(1)? as u64;
                summary.reparse_points += row.get::<_, i64>(2)? as u64;
                Ok(())
            },
        )
        .map_err(db_err)?;

    db.conn()
        .query_row(
            "SELECT
                COUNT(*) FILTER (WHERE status = 'PENDING'),
                COUNT(*) FILTER (WHERE status = 'HASHED'),
                COUNT(*) FILTER (WHERE status = 'FAILED'),
                COUNT(*) FILTER (WHERE status = 'SOURCE_CHANGED')
             FROM hash_jobs WHERE snapshot_id = ?1",
            [&id],
            |row| {
                summary.hash_pending = row.get::<_, i64>(0)? as u64;
                summary.hash_done = row.get::<_, i64>(1)? as u64;
                summary.hash_failed = row.get::<_, i64>(2)? as u64;
                summary.hash_source_changed = row.get::<_, i64>(3)? as u64;
                Ok(())
            },
        )
        .map_err(db_err)?;

    Ok(summary)
}

/// Every folder of a snapshot, ordered by depth then path (parents first).
pub fn list_folders(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<FolderRecord>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, snapshot_id, source_root_id, relative_path,
                    parent_relative_path, name, normalized_name, depth, status,
                    error, raw_relative_path
             FROM folders
             WHERE snapshot_id = ?1
             ORDER BY depth, relative_path",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<FolderRecord>> = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<Vec<u8>>>(10)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (
                id,
                snapshot,
                root,
                relative,
                parent,
                name,
                normalized,
                depth,
                status,
                error,
                raw_relative_path,
            ) = raw.map_err(db_err)?;
            Ok(FolderRecord {
                id: df_domain::FolderId::from_str(&id)?,
                snapshot_id: SnapshotId::from_str(&snapshot)?,
                source_root_id: SourceRootId::from_str(&root)?,
                relative_path: relative,
                raw_relative_path: raw_relative_path
                    .as_deref()
                    .map(df_domain::RawPath::from_blob)
                    .transpose()?,
                parent_relative_path: parent,
                name,
                normalized_name: normalized,
                depth: depth as u32,
                status: df_domain::ScanEntryStatus::parse(&status)?,
                error,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Every occurrence of a snapshot, ordered by relative path.
pub fn list_occurrences(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<PathOccurrence>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, snapshot_id, source_root_id, relative_path,
                    parent_relative_path, file_name, normalized_name, extension,
                    size_bytes, created_at_fs, modified_at_fs, attributes,
                    path_length, depth, fingerprint, scan_status, error,
                    name_is_lossy, raw_relative_path
             FROM path_occurrences
             WHERE snapshot_id = ?1
             ORDER BY relative_path",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<PathOccurrence>> = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, Option<String>>(16)?,
                row.get::<_, i64>(17)?,
                row.get::<_, Option<Vec<u8>>>(18)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (
                id,
                snapshot,
                root,
                relative_path,
                parent_relative_path,
                file_name,
                normalized_name,
                extension,
                size,
                created_fs,
                modified_fs,
                attributes,
                path_length,
                depth,
                fingerprint,
                scan_status,
                error,
                name_is_lossy,
                raw_relative_path,
            ) = raw.map_err(db_err)?;
            Ok(PathOccurrence {
                id: OccurrenceId::from_str(&id)?,
                snapshot_id: SnapshotId::from_str(&snapshot)?,
                source_root_id: SourceRootId::from_str(&root)?,
                relative_path,
                raw_relative_path: raw_relative_path
                    .as_deref()
                    .map(df_domain::RawPath::from_blob)
                    .transpose()?,
                parent_relative_path,
                file_name,
                normalized_name,
                extension,
                size_bytes: size as u64,
                created_at_fs: created_fs
                    .as_deref()
                    .map(parse_stored_timestamp)
                    .transpose()?,
                modified_at_fs: modified_fs
                    .as_deref()
                    .map(parse_stored_timestamp)
                    .transpose()?,
                attributes: attributes as u32,
                path_length: path_length as u32,
                depth: depth as u32,
                fingerprint,
                scan_status: df_domain::ScanEntryStatus::parse(&scan_status)?,
                error,
                name_is_lossy: name_is_lossy != 0,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// What one enqueue pass decided.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct EnqueueOutcome {
    /// Occurrences given a hash job.
    pub enqueued: u64,
    /// Occurrences the profile declined to read, each with its reason stored.
    pub excluded: u64,
}

/// One occurrence the profile declined to hash, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HashExclusionView {
    pub relative_path: String,
    pub rule_id: String,
    pub reason: String,
}

/// Record the exclusions an operator declared for a project.
///
/// Written once, at creation. The trigger refuses an update: a project whose
/// rules changed halfway would have a snapshot no single set of rules
/// explains.
pub fn set_project_hash_exclusions(
    db: &mut Db,
    project_id: ProjectId,
    exclusions: &[df_domain::HashExclusion],
) -> DfResult<()> {
    if exclusions.is_empty() {
        return Ok(());
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let now = to_stored_timestamp(chrono::Utc::now());
    for exclusion in exclusions {
        let matcher = serde_json::to_string(&exclusion.r#match).map_err(|error| {
            DfError::Serialization(format!("hash exclusion `{}`: {error}", exclusion.id))
        })?;
        tx.execute(
            "INSERT INTO project_hash_exclusions
                (project_id, id, reason, match_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                project_id.to_string(),
                exclusion.id,
                exclusion.reason,
                matcher,
                now
            ],
        )
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// The exclusions a project declared, if any.
pub fn project_hash_exclusions(
    db: &Db,
    project_id: ProjectId,
) -> DfResult<Vec<df_domain::HashExclusion>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, reason, match_json FROM project_hash_exclusions
             WHERE project_id = ?1 ORDER BY id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([project_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    rows.into_iter()
        .map(|(id, reason, matcher)| {
            Ok(df_domain::HashExclusion {
                id,
                reason,
                r#match: serde_json::from_str(&matcher).map_err(|error| {
                    DfError::Serialization(format!("stored hash exclusion: {error}"))
                })?,
            })
        })
        .collect()
}

/// Queue a hash job for every scanned-OK occurrence that has none yet.
/// Idempotent: rerunning after an interruption only fills the gaps.
///
/// Occurrences matching the project profile's `hash_exclusions` are recorded
/// as excluded instead of queued — inventoried, never read, reason kept.
pub fn enqueue_hash_jobs(
    db: &mut Db,
    snapshot_id: SnapshotId,
    actor: Actor,
) -> DfResult<EnqueueOutcome> {
    let project_id: String = db
        .conn()
        .query_row(
            "SELECT project_id FROM snapshots WHERE id = ?1",
            [snapshot_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    // Loaded before the transaction, so a profile that fails to validate stops
    // the stage rather than half-enqueuing it.
    let configured: String = db
        .conn()
        .query_row(
            "SELECT profile FROM projects WHERE id = ?1",
            [&project_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    // The profile's rules and the operator's, together. A profile ships what
    // is true of a domain; a project declares what is true of this archive,
    // and only the second is reachable without recompiling.
    let mut exclusions = df_domain::Profile::load(&configured)?.hash_exclusions;
    exclusions.extend(project_hash_exclusions(
        db,
        ProjectId::from_str(&project_id)?,
    )?);
    enqueue_hash_jobs_with(db, snapshot_id, actor, &exclusions)
}

/// [`enqueue_hash_jobs`] with the exclusions supplied rather than loaded.
///
/// Separate so that what to read is a decision a caller can see and a test can
/// state, instead of something the profile does invisibly.
pub fn enqueue_hash_jobs_with(
    db: &mut Db,
    snapshot_id: SnapshotId,
    actor: Actor,
    exclusions: &[df_domain::HashExclusion],
) -> DfResult<EnqueueOutcome> {
    let snapshot = snapshot_id.to_string();
    let project_id: String = db
        .conn()
        .query_row(
            "SELECT project_id FROM snapshots WHERE id = ?1",
            [&snapshot],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let project_id = ProjectId::from_str(&project_id)?;

    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let now = to_stored_timestamp(chrono::Utc::now());
    // UUIDs cannot be generated by SQLite, so enqueue row by row.
    // Candidates carry what an exclusion matches on, so the decision is taken
    // once here rather than per rule.
    let candidates: Vec<(String, String, String, u64)> = {
        let mut stmt = tx
            .prepare(
                "SELECT o.id, o.relative_path, o.file_name, o.size_bytes
                 FROM path_occurrences o
                 WHERE o.snapshot_id = ?1 AND o.scan_status = 'OK'
                   AND NOT EXISTS (SELECT 1 FROM hash_jobs j WHERE j.occurrence_id = o.id)
                   AND NOT EXISTS
                       (SELECT 1 FROM hash_exclusions e WHERE e.occurrence_id = o.id)",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? as u64,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    // Split before enqueuing anything: a file the profile declines to read is
    // recorded as excluded, with the rule's own wording, and never given a
    // job. The occurrence itself is untouched — the inventory still describes
    // the origin in full, which is what makes this an exclusion and not a
    // silent omission.
    let mut pending: Vec<String> = Vec::with_capacity(candidates.len());
    let mut excluded = 0u64;
    for (occurrence_id, relative_path, file_name, size_bytes) in candidates {
        match exclusions
            .iter()
            .find(|rule| rule.r#match.matches(&relative_path, &file_name, size_bytes))
        {
            Some(rule) => {
                tx.execute(
                    "INSERT INTO hash_exclusions
                        (occurrence_id, snapshot_id, rule_id, reason, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![occurrence_id, snapshot, rule.id, rule.reason, now],
                )
                .map_err(db_err)?;
                excluded += 1;
            }
            None => pending.push(occurrence_id),
        }
    }
    for occurrence_id in &pending {
        tx.execute(
            "INSERT INTO hash_jobs
                (id, snapshot_id, occurrence_id, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                HashJobId::new().to_string(),
                snapshot,
                occurrence_id,
                HashState::Pending.as_str(),
                now,
            ],
        )
        .map_err(db_err)?;
    }
    let enqueued = pending.len() as u64;
    if enqueued > 0 {
        let payload = serde_json::json!({
            "snapshot_id": snapshot,
            "jobs_enqueued": enqueued,
            // In the ledger, so the record of the run says how much of the
            // origin it chose not to read.
            "hash_excluded": excluded,
        });
        append_event(&tx, project_id, EVENT_HASH_STARTED, &payload, actor)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(EnqueueOutcome { enqueued, excluded })
}

/// What the profile declined to read in this snapshot, and why.
///
/// The answer to "was anything skipped?" — which an operator is entitled to
/// ask before trusting a delivery, and which an inventory alone cannot give.
pub fn hash_exclusions(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<HashExclusionView>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT o.relative_path, e.rule_id, e.reason
             FROM hash_exclusions e
             JOIN path_occurrences o ON o.id = e.occurrence_id
             WHERE e.snapshot_id = ?1
             ORDER BY e.rule_id, o.relative_path",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok(HashExclusionView {
                relative_path: row.get(0)?,
                rule_id: row.get(1)?,
                reason: row.get(2)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// Everything the hasher needs to process one pending job.
#[derive(Debug, Clone)]
pub struct PendingHashJob {
    pub job_id: HashJobId,
    pub occurrence_id: OccurrenceId,
    pub snapshot_id: SnapshotId,
    /// Absolute path of the source root that contains the file.
    pub root_path: PathBuf,
    /// Exact path captured by the scanner. This is authoritative whenever it
    /// exists; `relative_path` below is display/legacy evidence only.
    pub raw_relative_path: Option<df_domain::RawPath>,
    pub relative_path: String,
    pub size_bytes: u64,
    /// Fingerprint captured at scan time (RFC-0001 §14.5 pre-check).
    pub fingerprint: String,
}

/// Fetch up to `limit` pending jobs of a snapshot, oldest first.
pub fn pending_hash_jobs(
    db: &Db,
    snapshot_id: SnapshotId,
    limit: u32,
) -> DfResult<Vec<PendingHashJob>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT j.id, j.occurrence_id, o.relative_path, o.size_bytes,
                    o.fingerprint, r.absolute_path, o.raw_relative_path
             FROM hash_jobs j
             JOIN path_occurrences o ON o.id = j.occurrence_id
             JOIN source_roots r ON r.id = o.source_root_id
             WHERE j.snapshot_id = ?1 AND j.status = 'PENDING'
             ORDER BY j.created_at, j.id
             LIMIT ?2",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<PendingHashJob>> = stmt
        .query_map(params![snapshot_id.to_string(), limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (job, occurrence, relative, size, fingerprint, root, raw_relative) =
                raw.map_err(db_err)?;
            Ok(PendingHashJob {
                job_id: HashJobId::from_str(&job)?,
                occurrence_id: OccurrenceId::from_str(&occurrence)?,
                snapshot_id,
                root_path: PathBuf::from(root),
                raw_relative_path: raw_relative
                    .as_deref()
                    .map(df_domain::RawPath::from_blob)
                    .transpose()?,
                relative_path: relative,
                size_bytes: size as u64,
                fingerprint,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Carry content bindings forward from the previous complete snapshot for
/// every occurrence whose v2 fingerprint is byte-identical with all fields
/// present (size, mtime, ctime, attributes, volume and file id) — the
/// strongest identity available without reading bytes (ADR-0019). Weaker
/// tokens (v1, or any `none` field) never qualify. Closes the matching
/// hash jobs and returns how many bindings were reused; one transaction.
pub fn reuse_previous_hash_bindings(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
) -> DfResult<u64> {
    let previous: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM snapshots
             WHERE project_id = ?1 AND status = 'COMPLETE' AND id <> ?2
             ORDER BY created_at DESC, id DESC LIMIT 1",
            params![project_id.to_string(), snapshot_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(previous) = previous else {
        return Ok(0);
    };

    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let reused = tx
        .execute(
            "INSERT INTO occurrence_content
                (occurrence_id, content_id, created_at, reused_from_snapshot)
             SELECT o_new.id, oc_prev.content_id, ?3, ?2
             FROM path_occurrences o_new
             JOIN path_occurrences o_prev
               ON o_prev.snapshot_id = ?2
              AND o_prev.source_root_id = o_new.source_root_id
              AND o_prev.relative_path = o_new.relative_path
             JOIN occurrence_content oc_prev ON oc_prev.occurrence_id = o_prev.id
             JOIN content_objects c
               ON c.id = oc_prev.content_id
              AND c.hash_state = 'HASHED' AND c.sha256 IS NOT NULL
             WHERE o_new.snapshot_id = ?1
               AND o_new.scan_status = 'OK'
               AND o_new.fingerprint = o_prev.fingerprint
               AND o_new.fingerprint LIKE 'v2:%'
               AND o_new.fingerprint NOT LIKE '%none%'
               AND NOT EXISTS (
                   SELECT 1 FROM occurrence_content x WHERE x.occurrence_id = o_new.id
               )",
            params![snapshot_id.to_string(), previous, now],
        )
        .map_err(db_err)?;
    tx.execute(
        "UPDATE hash_jobs SET status = 'HASHED', updated_at = ?2
         WHERE snapshot_id = ?1 AND status = 'PENDING'
           AND occurrence_id IN (
               SELECT occurrence_id FROM occurrence_content
               WHERE reused_from_snapshot IS NOT NULL
           )",
        params![snapshot_id.to_string(), now],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(reused as u64)
}

/// Outcome of hashing one queued job, computed before touching the
/// database.
#[derive(Debug)]
pub enum HashJobOutcome {
    /// Both digests computed with the fingerprint stable around the read.
    Hashed { sha256: String, blake3: String },
    /// The job closes without digests: `FAILED` or `SOURCE_CHANGED`.
    Closed { state: HashState, error: String },
}

/// One queued job together with its computed outcome.
#[derive(Debug)]
pub struct HashJobResult<'a> {
    pub job: &'a PendingHashJob,
    pub outcome: HashJobOutcome,
}

/// Persist a batch of hash results — one transaction for the whole batch.
///
/// Per-file commits are what made large runs crawl, and the queue makes
/// them unnecessary: results not yet committed simply stay `PENDING` and
/// are recomputed on resume (RFC-0001 §14, rule 13). Job failures are data,
/// not errors; a batch aborts only on infrastructure failures, including a
/// stored-content size conflict, which signals a corrupted inventory rather
/// than a bad file.
pub fn record_hash_results(db: &mut Db, results: &[HashJobResult<'_>]) -> DfResult<()> {
    if results.is_empty() {
        return Ok(());
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    for result in results {
        match &result.outcome {
            HashJobOutcome::Hashed { sha256, blake3 } => {
                apply_hash_success(&tx, result.job, sha256, blake3)?;
            }
            HashJobOutcome::Closed { state, error } => {
                apply_hash_closure(&tx, result.job.job_id, *state, error)?;
            }
        }
    }
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Bind one occurrence to its (possibly already known) content object and
/// close the job inside the batch transaction.
fn apply_hash_success(
    tx: &Transaction<'_>,
    job: &PendingHashJob,
    sha256: &str,
    blake3: &str,
) -> DfResult<ContentId> {
    let existing: Option<(String, i64)> = tx
        .prepare_cached("SELECT id, size_bytes FROM content_objects WHERE sha256 = ?1")
        .map_err(db_err)?
        .query_row([sha256], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .map_err(db_err)?;

    let content_id = match existing {
        Some((id, size)) => {
            if size as u64 != job.size_bytes {
                return Err(DfError::Conflict(format!(
                    "content {sha256} already stored with size {size}, \
                     but occurrence `{}` has size {}",
                    job.relative_path, job.size_bytes
                )));
            }
            ContentId::from_str(&id)?
        }
        None => {
            let content = ContentObject {
                id: ContentId::new(),
                size_bytes: job.size_bytes,
                sha256: Some(sha256.to_string()),
                blake3: Some(blake3.to_string()),
                mime_type: None,
                first_seen_snapshot: job.snapshot_id,
                hash_state: HashState::Hashed,
            };
            tx.prepare_cached(
                "INSERT INTO content_objects
                    (id, size_bytes, sha256, blake3, mime_type,
                     first_seen_snapshot, hash_state, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(db_err)?
            .execute(params![
                content.id.to_string(),
                content.size_bytes as i64,
                content.sha256,
                content.blake3,
                content.mime_type,
                content.first_seen_snapshot.to_string(),
                content.hash_state.as_str(),
                to_stored_timestamp(chrono::Utc::now()),
            ])
            .map_err(db_err)?;
            content.id
        }
    };

    tx.prepare_cached(
        "INSERT INTO occurrence_content (occurrence_id, content_id, created_at)
         VALUES (?1, ?2, ?3)",
    )
    .map_err(db_err)?
    .execute(params![
        job.occurrence_id.to_string(),
        content_id.to_string(),
        to_stored_timestamp(chrono::Utc::now()),
    ])
    .map_err(db_err)?;
    tx.prepare_cached("UPDATE hash_jobs SET status = ?1, updated_at = ?2 WHERE id = ?3")
        .map_err(db_err)?
        .execute(params![
            HashState::Hashed.as_str(),
            to_stored_timestamp(chrono::Utc::now()),
            job.job_id.to_string(),
        ])
        .map_err(db_err)?;
    Ok(content_id)
}

/// Close a job as `FAILED` or `SOURCE_CHANGED` inside the batch
/// transaction.
fn apply_hash_closure(
    tx: &Transaction<'_>,
    job_id: HashJobId,
    state: HashState,
    error: &str,
) -> DfResult<()> {
    if !matches!(state, HashState::Failed | HashState::SourceChanged) {
        return Err(DfError::Validation(
            "hash failures must be FAILED or SOURCE_CHANGED".to_string(),
        ));
    }
    tx.prepare_cached(
        "UPDATE hash_jobs SET status = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
    )
    .map_err(db_err)?
    .execute(params![
        state.as_str(),
        error,
        to_stored_timestamp(chrono::Utc::now()),
        job_id.to_string(),
    ])
    .map_err(db_err)?;
    Ok(())
}

/// Emit the audit event that closes (or pauses) a hash run.
pub fn record_hash_outcome(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    event_type: &str,
    summary: &InventorySummary,
    reused: u64,
    actor: Actor,
) -> DfResult<()> {
    let payload = serde_json::json!({
        "snapshot_id": snapshot_id.to_string(),
        "hashed": summary.hash_done,
        "failed": summary.hash_failed,
        "source_changed": summary.hash_source_changed,
        "pending": summary.hash_pending,
        "reused_from_previous_snapshot": reused,
    });
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    append_event(&tx, project_id, event_type, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// One set of exact duplicates: same size, same SHA-256 (RFC-0001 §15.1).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DuplicateSet {
    pub sha256: String,
    pub size_bytes: u64,
    /// Absolute paths of every occurrence, reconstructed root + relative.
    pub occurrences: Vec<String>,
    /// Absolute path of the logical representative — the best canonical
    /// location of this content (RFC-0001 §15.5), or `None` when the snapshot
    /// has not been analysed yet. Naming a representative never implies that
    /// the other occurrences are dispensable (§15.5, rule 8).
    pub representative: Option<String>,
    /// Why that occurrence was chosen (§5.3 explainable-by-design).
    pub representative_reason: Option<String>,
}

/// Exact duplicate sets of a snapshot, largest waste first.
///
/// Report only (RFC-0001 §15.2): a duplicate is never automatically
/// dispensable, so this function proposes nothing — it lists evidence.
pub fn exact_duplicates(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<DuplicateSet>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT c.sha256, c.size_bytes, r.absolute_path, o.relative_path
             FROM occurrence_content oc
             JOIN content_objects c ON c.id = oc.content_id
             JOIN path_occurrences o ON o.id = oc.occurrence_id
             JOIN source_roots r ON r.id = o.source_root_id
             WHERE o.snapshot_id = ?1
               AND c.sha256 IS NOT NULL
               AND oc.content_id IN (
                   SELECT oc2.content_id FROM occurrence_content oc2
                   JOIN path_occurrences o2 ON o2.id = oc2.occurrence_id
                   WHERE o2.snapshot_id = ?1
                   GROUP BY oc2.content_id HAVING COUNT(*) > 1
               )
             ORDER BY c.size_bytes DESC, c.sha256, r.absolute_path, o.relative_path",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut sets: Vec<DuplicateSet> = Vec::new();
    for (sha256, size, root, relative) in rows {
        let absolute = if relative.is_empty() {
            root
        } else {
            format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
        };
        match sets.last_mut() {
            Some(set) if set.sha256 == sha256 => set.occurrences.push(absolute),
            _ => sets.push(DuplicateSet {
                sha256,
                size_bytes: size as u64,
                occurrences: vec![absolute],
                representative: None,
                representative_reason: None,
            }),
        }
    }
    attach_representatives(db, snapshot_id, &mut sets)?;
    Ok(sets)
}

/// One file name that means different things in different places.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NameCollision {
    /// The name as normalised for comparison, so case and width folding do
    /// not hide a collision.
    pub normalized_name: String,
    /// Distinct contents sharing it. Always at least 2 — that is the finding.
    pub contents: u64,
    /// Files carrying it, counting every occurrence.
    pub occurrences: u64,
    /// Distinct folders those files sit in.
    pub folders: u64,
    pub largest_bytes: u64,
    /// A few absolute paths, to make the collision concrete in a report.
    pub sample_paths: Vec<String>,
}

/// Names carrying more than one content, worst first.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NameCollisionReport {
    pub snapshot_id: String,
    /// Names that mean more than one thing.
    pub colliding_names: u64,
    /// Files involved in some collision.
    pub occurrences_involved: u64,
    /// Contents the worst single name stands for.
    pub worst_name_contents: u64,
    pub collisions: Vec<NameCollision>,
}

/// Report file names that stand for different content in different places.
///
/// This is the evidence behind a rule the engine already follows and could
/// not previously show: **never deduplicate by name**. On an evidential
/// archive the failure is not theoretical — scanned exhibits are numbered per
/// matter, so `00000001.JPG` legitimately names a different photograph in
/// every one of them. Collapsing those by name would not tidy anything, it
/// would destroy evidence and leave a plausible-looking tree behind.
///
/// Evidence only, like every other report here (RFC-0001 §15.2): it proposes
/// nothing and consolidates nothing. Its purpose is to let an operator — or an
/// agent writing a report for one — demonstrate why a name is not an identity,
/// which is what turns a refusal into a guarantee someone can check.
pub fn name_collisions(
    db: &Db,
    snapshot_id: SnapshotId,
    samples_per_name: usize,
) -> DfResult<NameCollisionReport> {
    // Grouped by content id rather than by hash so a snapshot that is only
    // partially hashed reports what it knows instead of failing: an unhashed
    // occurrence has no content row and simply does not take part.
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT o.normalized_name,
                    COUNT(DISTINCT oc.content_id),
                    COUNT(*),
                    COUNT(DISTINCT o.parent_relative_path),
                    MAX(o.size_bytes)
             FROM path_occurrences o
             JOIN occurrence_content oc ON oc.occurrence_id = o.id
             WHERE o.snapshot_id = ?1
             GROUP BY o.normalized_name
             HAVING COUNT(DISTINCT oc.content_id) > 1
             ORDER BY COUNT(DISTINCT oc.content_id) DESC,
                      COUNT(*) DESC,
                      o.normalized_name",
        )
        .map_err(db_err)?;

    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, i64>(4)? as u64,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let colliding_names = rows.len() as u64;
    let occurrences_involved = rows.iter().map(|row| row.2).sum();
    let worst_name_contents = rows.iter().map(|row| row.1).max().unwrap_or(0);

    let mut collisions = Vec::with_capacity(rows.len());
    for (normalized_name, contents, occurrences, folders, largest_bytes) in rows {
        let sample_paths = collision_samples(db, snapshot_id, &normalized_name, samples_per_name)?;
        collisions.push(NameCollision {
            normalized_name,
            contents,
            occurrences,
            folders,
            largest_bytes,
            sample_paths,
        });
    }

    Ok(NameCollisionReport {
        snapshot_id: snapshot_id.to_string(),
        colliding_names,
        occurrences_involved,
        worst_name_contents,
        collisions,
    })
}

/// A few absolute paths for one colliding name, one per distinct content so
/// the sample shows the disagreement rather than repeating one side of it.
fn collision_samples(
    db: &Db,
    snapshot_id: SnapshotId,
    normalized_name: &str,
    limit: usize,
) -> DfResult<Vec<String>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT r.absolute_path, o.relative_path
             FROM path_occurrences o
             JOIN occurrence_content oc ON oc.occurrence_id = o.id
             JOIN source_roots r ON r.id = o.source_root_id
             WHERE o.snapshot_id = ?1 AND o.normalized_name = ?2
             GROUP BY oc.content_id
             ORDER BY o.relative_path
             LIMIT ?3",
        )
        .map_err(db_err)?;
    let paths = stmt
        .query_map(
            rusqlite::params![snapshot_id.to_string(), normalized_name, limit as i64],
            |row| {
                let root = row.get::<_, String>(0)?;
                let relative = row.get::<_, String>(1)?;
                Ok(if relative.is_empty() {
                    root
                } else {
                    format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
                })
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(paths)
}

/// Fill in the logical representative of each set from the rows recorded by
/// `dedup::score_duplicate_representatives` (RFC-0001 §15.5). Sets analysed
/// before that step existed simply keep `None`.
fn attach_representatives(
    db: &Db,
    snapshot_id: SnapshotId,
    sets: &mut [DuplicateSet],
) -> DfResult<()> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT c.sha256, r.absolute_path, o.relative_path, dr.reason
             FROM duplicate_representatives dr
             JOIN duplicate_sets ds ON ds.id = dr.duplicate_set_id
             JOIN content_objects c ON c.id = ds.content_id
             JOIN path_occurrences o ON o.id = dr.occurrence_id
             JOIN source_roots r ON r.id = o.source_root_id
             WHERE dr.snapshot_id = ?1 AND c.sha256 IS NOT NULL",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            let sha256: String = row.get(0)?;
            let root: String = row.get(1)?;
            let relative: String = row.get(2)?;
            let reason: String = row.get(3)?;
            let absolute = if relative.is_empty() {
                root
            } else {
                format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
            };
            Ok((sha256, absolute, reason))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let by_sha: std::collections::HashMap<String, (String, String)> = rows
        .into_iter()
        .map(|(sha, path, reason)| (sha, (path, reason)))
        .collect();
    for set in sets.iter_mut() {
        if let Some((path, reason)) = by_sha.get(&set.sha256) {
            set.representative = Some(path.clone());
            set.representative_reason = Some(reason.clone());
        }
    }
    Ok(())
}
