//! Persistence boundary for the plugin ecosystem (Milestone 0.6).
//!
//! This module stores signed registrations, configuration-addressed runs
//! and append-only findings. It knows nothing about WebAssembly: signature
//! and component verification belong to the plugin host, which re-verifies
//! everything read back from here before executing it.

use std::str::FromStr;

use df_domain::{
    Actor, PluginFindingId, PluginRegistrationId, PluginRun, PluginRunCounters, PluginRunId,
    PluginRunStatus, ProjectId, SnapshotId,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension};

use crate::repository::{append_event, parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

pub const EVENT_PLUGIN_REGISTERED: &str = "PLUGIN_REGISTERED";
pub const EVENT_PLUGIN_RUN_STARTED: &str = "PLUGIN_RUN_STARTED";
pub const EVENT_PLUGIN_RUN_COMPLETED: &str = "PLUGIN_RUN_COMPLETED";

/// A signed registration exactly as persisted; the host re-verifies the
/// signature and the component hash before compiling anything from it.
#[derive(Debug, Clone)]
pub struct StoredRegistration {
    pub id: PluginRegistrationId,
    pub plugin_id: String,
    pub plugin_version: String,
    pub manifest_json: String,
    pub component_sha256: String,
    pub component: Vec<u8>,
    pub publisher_public_key_hex: String,
    pub signature_hex: String,
}

/// One subject the orchestrator will offer to a plugin: a unique content of
/// the snapshot with its bounded metadata.
#[derive(Debug, Clone)]
pub struct PluginSubjectSource {
    pub content_id: String,
    pub sha256: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub extension: Option<String>,
}

/// One validated finding ready to persist.
#[derive(Debug, Clone)]
pub struct FindingInput {
    pub subject_id: String,
    pub code: String,
    pub severity: String,
    pub message: String,
    pub suggestions_json: String,
    pub evidence_json: String,
}

/// A sealed finding for reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginFindingView {
    pub plugin: String,
    pub subject_id: String,
    pub code: String,
    pub severity: String,
    pub message: String,
    pub suggestions: serde_json::Value,
    pub evidence: serde_json::Value,
}

/// Sealed run joined with its plugin identity, for status and reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginRunView {
    pub run_id: String,
    pub plugin: String,
    pub status: String,
    pub config_digest: String,
    pub subjects_total: u64,
    pub subjects_analyzed: u64,
    pub subjects_failed: u64,
    pub subject_cap_reached: bool,
    pub findings_total: u64,
}

/// Persist one verified registration — the caller (the host) has already
/// checked signature, hash, manifest and compilability.
pub fn insert_registration(
    db: &mut Db,
    project_id: ProjectId,
    registration: &StoredRegistration,
    actor: Actor,
) -> DfResult<PluginRegistrationId> {
    let existing: Option<String> = db
        .conn()
        .query_row(
            "SELECT id FROM plugin_registrations
             WHERE project_id = ?1 AND plugin_id = ?2 AND plugin_version = ?3",
            params![
                project_id.to_string(),
                registration.plugin_id,
                registration.plugin_version
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if existing.is_some() {
        return Err(DfError::Conflict(format!(
            "plugin `{}@{}` is already registered; a new version requires a new registration",
            registration.plugin_id, registration.plugin_version
        )));
    }

    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO plugin_registrations
            (id, project_id, plugin_id, plugin_version, manifest_json,
             component_sha256, component, publisher_public_key_hex,
             signature_hex, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            registration.id.to_string(),
            project_id.to_string(),
            registration.plugin_id,
            registration.plugin_version,
            registration.manifest_json,
            registration.component_sha256,
            registration.component,
            registration.publisher_public_key_hex,
            registration.signature_hex,
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        project_id,
        EVENT_PLUGIN_REGISTERED,
        &serde_json::json!({
            "registration_id": registration.id.to_string(),
            "plugin": format!("{}@{}", registration.plugin_id, registration.plugin_version),
            "component_sha256": registration.component_sha256,
            "publisher_public_key_hex": registration.publisher_public_key_hex,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    Ok(registration.id)
}

/// Every registration of the project, in registration order.
pub fn list_registrations(db: &Db, project_id: ProjectId) -> DfResult<Vec<StoredRegistration>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, plugin_id, plugin_version, manifest_json,
                    component_sha256, component, publisher_public_key_hex,
                    signature_hex
             FROM plugin_registrations
             WHERE project_id = ?1
             ORDER BY created_at, id",
        )
        .map_err(db_err)?;
    let raw = stmt
        .query_map([project_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Vec<u8>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(id, plugin_id, plugin_version, manifest, sha, component, key, signature)| {
                Ok(StoredRegistration {
                    id: PluginRegistrationId::from_str(&id)?,
                    plugin_id,
                    plugin_version,
                    manifest_json: manifest,
                    component_sha256: sha,
                    component,
                    publisher_public_key_hex: key,
                    signature_hex: signature,
                })
            },
        )
        .collect()
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
    Option<String>,
    String,
    Option<String>,
);

const RUN_COLUMNS: &str = "id, project_id, snapshot_id, registration_id, status, config_digest,
     config_json, subjects_total, subjects_analyzed, subjects_failed,
     subject_cap_reached, findings_total, error, started_at, finished_at";

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
    ))
}

fn run_from_stored(row: StoredRunRow) -> DfResult<PluginRun> {
    let (
        id,
        project_id,
        snapshot_id,
        registration_id,
        status,
        config_digest,
        config_json,
        subjects_total,
        subjects_analyzed,
        subjects_failed,
        subject_cap_reached,
        findings_total,
        error,
        started_at,
        finished_at,
    ) = row;
    Ok(PluginRun {
        id: PluginRunId::from_str(&id)?,
        project_id: ProjectId::from_str(&project_id)?,
        snapshot_id: SnapshotId::from_str(&snapshot_id)?,
        registration_id: PluginRegistrationId::from_str(&registration_id)?,
        status: PluginRunStatus::parse(&status)?,
        config_digest,
        config: serde_json::from_str(&config_json)
            .map_err(|error| DfError::Validation(format!("stored plugin config: {error}")))?,
        counters: PluginRunCounters {
            subjects_total: subjects_total as u64,
            subjects_analyzed: subjects_analyzed as u64,
            subjects_failed: subjects_failed as u64,
            findings_total: findings_total as u64,
        },
        subject_cap_reached: subject_cap_reached != 0,
        error,
        started_at: parse_stored_timestamp(&started_at)?,
        finished_at: finished_at
            .as_deref()
            .map(parse_stored_timestamp)
            .transpose()?,
    })
}

fn load_run_by_id(db: &Db, run_id: PluginRunId) -> DfResult<PluginRun> {
    let row = db
        .conn()
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM plugin_runs WHERE id = ?1"),
            [run_id.to_string()],
            map_run_row,
        )
        .map_err(db_err)?;
    run_from_stored(row)
}

/// Immutable identity of one plugin run.
#[derive(Debug, Clone)]
pub struct PluginRunSpec {
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub registration_id: PluginRegistrationId,
    pub config_digest: String,
    pub config_json: String,
}

/// Open a new run or return the digest-addressed existing one. Requires a
/// structurally analysed snapshot, like similarity and media.
pub fn start_or_resume_run(db: &mut Db, spec: &PluginRunSpec, actor: Actor) -> DfResult<PluginRun> {
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
            "plugin analysis requires a completed, structurally analysed snapshot".to_string(),
        ));
    }

    let existing: Option<(String, String)> = db
        .conn()
        .query_row(
            "SELECT id, config_json FROM plugin_runs
             WHERE snapshot_id = ?1 AND registration_id = ?2 AND config_digest = ?3",
            params![
                spec.snapshot_id.to_string(),
                spec.registration_id.to_string(),
                spec.config_digest
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((id, config_json)) = existing {
        if config_json != spec.config_json {
            return Err(DfError::Conflict(format!(
                "plugin run `{id}` does not match the digest-addressed configuration"
            )));
        }
        let run = load_run_by_id(db, PluginRunId::from_str(&id)?)?;
        if run.status == PluginRunStatus::Failed {
            return Err(DfError::Conflict(format!(
                "plugin run `{}` failed; use a new configuration digest",
                run.id
            )));
        }
        return Ok(run);
    }

    let run_id = PluginRunId::new();
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO plugin_runs
            (id, project_id, snapshot_id, registration_id, status,
             config_digest, config_json, started_at, created_at)
         VALUES (?1, ?2, ?3, ?4, 'RUNNING', ?5, ?6, ?7, ?7)",
        params![
            run_id.to_string(),
            spec.project_id.to_string(),
            spec.snapshot_id.to_string(),
            spec.registration_id.to_string(),
            spec.config_digest,
            spec.config_json,
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        spec.project_id,
        EVENT_PLUGIN_RUN_STARTED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": spec.snapshot_id.to_string(),
            "registration_id": spec.registration_id.to_string(),
            "config_digest": spec.config_digest,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run_by_id(db, run_id)
}

/// Page through unique contents of the snapshot as plugin subjects.
pub fn plugin_subjects_after(
    db: &Db,
    snapshot_id: SnapshotId,
    after_content_id: Option<&str>,
    limit: u32,
) -> DfResult<Vec<PluginSubjectSource>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = db
        .conn()
        .prepare(
            "WITH ranked AS (
                SELECT c.id AS content_id, c.sha256, c.size_bytes,
                       o.relative_path, o.extension,
                       ROW_NUMBER() OVER (
                           PARTITION BY c.id
                           ORDER BY o.source_root_id, o.relative_path, o.id
                       ) AS rn
                FROM occurrence_content oc
                JOIN path_occurrences o ON o.id = oc.occurrence_id
                JOIN content_objects c ON c.id = oc.content_id
                WHERE o.snapshot_id = ?1 AND o.scan_status = 'OK'
                  AND c.hash_state = 'HASHED' AND c.sha256 IS NOT NULL
                  AND (?2 IS NULL OR c.id > ?2)
             )
             SELECT content_id, sha256, size_bytes, relative_path, extension
             FROM ranked WHERE rn = 1
             ORDER BY content_id LIMIT ?3",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(
            params![snapshot_id.to_string(), after_content_id, limit as i64],
            |row| {
                Ok(PluginSubjectSource {
                    content_id: row.get(0)?,
                    sha256: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)? as u64,
                    relative_path: row.get(3)?,
                    extension: row.get(4)?,
                })
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// Drop run-scoped findings so an interrupted run rebuilds
/// deterministically. Rejected by trigger once the run is sealed.
pub fn reset_run_findings(db: &mut Db, run_id: PluginRunId) -> DfResult<()> {
    db.conn()
        .execute(
            "DELETE FROM plugin_findings WHERE run_id = ?1",
            [run_id.to_string()],
        )
        .map_err(db_err)?;
    Ok(())
}

/// Persist one batch of validated findings — one transaction.
pub fn record_findings(
    db: &mut Db,
    run_id: PluginRunId,
    snapshot_id: SnapshotId,
    batch: &[FindingInput],
) -> DfResult<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    for finding in batch {
        tx.prepare_cached(
            "INSERT INTO plugin_findings
                (id, run_id, snapshot_id, subject_id, code, severity,
                 message, suggestions_json, evidence_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .map_err(db_err)?
        .execute(params![
            PluginFindingId::new().to_string(),
            run_id.to_string(),
            snapshot_id.to_string(),
            finding.subject_id,
            finding.code,
            finding.severity,
            finding.message,
            finding.suggestions_json,
            finding.evidence_json,
            to_stored_timestamp(chrono::Utc::now()),
        ])
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Seal the run; the audit event commits in the same transaction.
pub fn complete_run(
    db: &mut Db,
    run_id: PluginRunId,
    counters: PluginRunCounters,
    subject_cap_reached: bool,
    actor: Actor,
) -> DfResult<PluginRun> {
    let run = load_run_by_id(db, run_id)?;
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE plugin_runs
         SET status = 'COMPLETED', subjects_total = ?2, subjects_analyzed = ?3,
             subjects_failed = ?4, subject_cap_reached = ?5,
             findings_total = ?6, finished_at = ?7
         WHERE id = ?1",
        params![
            run_id.to_string(),
            counters.subjects_total as i64,
            counters.subjects_analyzed as i64,
            counters.subjects_failed as i64,
            subject_cap_reached as i64,
            counters.findings_total as i64,
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        run.project_id,
        EVENT_PLUGIN_RUN_COMPLETED,
        &serde_json::json!({
            "run_id": run_id.to_string(),
            "snapshot_id": run.snapshot_id.to_string(),
            "subjects_total": counters.subjects_total,
            "subjects_analyzed": counters.subjects_analyzed,
            "subjects_failed": counters.subjects_failed,
            "subject_cap_reached": subject_cap_reached,
            "findings_total": counters.findings_total,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    load_run_by_id(db, run_id)
}

/// Latest sealed run per registration for one snapshot.
pub fn latest_completed_runs(
    db: &Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
) -> DfResult<Vec<PluginRunView>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT r.id, g.plugin_id || '@' || g.plugin_version, r.status,
                    r.config_digest, r.subjects_total, r.subjects_analyzed,
                    r.subjects_failed, r.subject_cap_reached, r.findings_total
             FROM plugin_runs r
             JOIN plugin_registrations g ON g.id = r.registration_id
             WHERE r.project_id = ?1 AND r.snapshot_id = ?2
               AND r.status = 'COMPLETED'
               AND r.created_at = (
                   SELECT MAX(r2.created_at) FROM plugin_runs r2
                   WHERE r2.registration_id = r.registration_id
                     AND r2.snapshot_id = r.snapshot_id
                     AND r2.status = 'COMPLETED'
               )
             ORDER BY g.plugin_id, g.plugin_version",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(
            params![project_id.to_string(), snapshot_id.to_string()],
            |row| {
                Ok(PluginRunView {
                    run_id: row.get(0)?,
                    plugin: row.get(1)?,
                    status: row.get(2)?,
                    config_digest: row.get(3)?,
                    subjects_total: row.get::<_, i64>(4)? as u64,
                    subjects_analyzed: row.get::<_, i64>(5)? as u64,
                    subjects_failed: row.get::<_, i64>(6)? as u64,
                    subject_cap_reached: row.get::<_, i64>(7)? != 0,
                    findings_total: row.get::<_, i64>(8)? as u64,
                })
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// Sealed findings of one run, most severe first.
pub fn list_findings(db: &Db, run_id: PluginRunId, limit: u32) -> DfResult<Vec<PluginFindingView>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT g.plugin_id || '@' || g.plugin_version, f.subject_id,
                    f.code, f.severity, f.message, f.suggestions_json,
                    f.evidence_json
             FROM plugin_findings f
             JOIN plugin_runs r ON r.id = f.run_id
             JOIN plugin_registrations g ON g.id = r.registration_id
             WHERE f.run_id = ?1
             ORDER BY CASE f.severity WHEN 'WARNING' THEN 0 ELSE 1 END,
                      f.subject_id, f.code
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
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    raw.into_iter()
        .map(
            |(plugin, subject_id, code, severity, message, suggestions, evidence)| {
                Ok(PluginFindingView {
                    plugin,
                    subject_id,
                    code,
                    severity,
                    message,
                    suggestions: serde_json::from_str(&suggestions).map_err(|error| {
                        DfError::Validation(format!("stored plugin suggestions: {error}"))
                    })?,
                    evidence: serde_json::from_str(&evidence).map_err(|error| {
                        DfError::Validation(format!("stored plugin evidence: {error}"))
                    })?,
                })
            },
        )
        .collect()
}
