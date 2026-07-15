//! Persistence for analysis, planning, execution and verification
//! (RFC-0001 §15, §26, §27, §28).
//!
//! Same contract as the other repositories: every mutation is transactional
//! and audit events commit with the data they describe.

use std::path::PathBuf;
use std::str::FromStr;

use df_domain::{
    Actor, ApprovalState, ContentId, DuplicateSetId, ExecutionState, FindingId, ManifestEntry,
    OccurrenceId, OperationErrorCode, OperationId, OperationType, Plan, PlanId, PlanOperation,
    PlanStatus, ProjectId, RiskLevel, SnapshotId, SourceRootId, VerificationRunId,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::repository::{append_event, parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

/// Event types emitted by the planning half of the pipeline.
pub const EVENT_ANALYSIS_COMPLETED: &str = "ANALYSIS_COMPLETED";
pub const EVENT_PLAN_CREATED: &str = "PLAN_CREATED";
pub const EVENT_PLAN_APPROVED: &str = "PLAN_APPROVED";
pub const EVENT_EXECUTION_COMPLETED: &str = "EXECUTION_COMPLETED";
pub const EVENT_EXECUTION_PAUSED: &str = "EXECUTION_PAUSED";
pub const EVENT_VERIFICATION_COMPLETED: &str = "VERIFICATION_COMPLETED";

/// Append a standalone audit event in its own transaction.
pub fn emit_event(
    db: &mut Db,
    project_id: ProjectId,
    event_type: &str,
    payload: &serde_json::Value,
    actor: Actor,
) -> DfResult<()> {
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    append_event(&tx, project_id, event_type, payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Materialise the exact duplicate sets of a snapshot (RFC-0001 §15.1) and
/// emit `ANALYSIS_COMPLETED` — one tx. Idempotent per (snapshot, content).
pub fn materialize_duplicate_sets(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    actor: Actor,
) -> DfResult<u64> {
    let snapshot = snapshot_id.to_string();
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let groups: Vec<(String, i64, i64)> = {
        let mut stmt = tx
            .prepare(
                "SELECT c.id, c.size_bytes, COUNT(*)
                 FROM occurrence_content oc
                 JOIN content_objects c ON c.id = oc.content_id
                 JOIN path_occurrences o ON o.id = oc.occurrence_id
                 WHERE o.snapshot_id = ?1 AND c.sha256 IS NOT NULL
                 GROUP BY oc.content_id HAVING COUNT(*) > 1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };
    for (content_id, size, count) in &groups {
        tx.execute(
            "INSERT INTO duplicate_sets
                (id, snapshot_id, content_id, occurrence_count, size_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (snapshot_id, content_id)
             DO UPDATE SET occurrence_count = excluded.occurrence_count",
            params![
                DuplicateSetId::new().to_string(),
                snapshot,
                content_id,
                count,
                size,
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
    }
    let payload = serde_json::json!({
        "snapshot_id": snapshot,
        "duplicate_sets": groups.len(),
    });
    append_event(&tx, project_id, EVENT_ANALYSIS_COMPLETED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(groups.len() as u64)
}

/// Everything the planner needs to know about one occurrence: its scan
/// verdict, its hash verdict and its content identity.
#[derive(Debug, Clone)]
pub struct PlanningOccurrence {
    pub occurrence_id: OccurrenceId,
    pub source_root_id: df_domain::SourceRootId,
    pub relative_path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub scan_status: df_domain::ScanEntryStatus,
    /// Hash job verdict (`HASHED`, `FAILED`, `SOURCE_CHANGED`, `PENDING`),
    /// `None` when the occurrence never got a job (scan errors, reparse).
    pub hash_status: Option<String>,
    pub hash_error: Option<String>,
    pub content_id: Option<ContentId>,
    pub sha256: Option<String>,
}

/// Planning view of every occurrence of a snapshot, ordered by path.
pub fn planning_occurrences(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<PlanningOccurrence>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT o.id, o.source_root_id, o.relative_path, o.file_name,
                    o.size_bytes, o.scan_status, j.status, j.error, c.id, c.sha256
             FROM path_occurrences o
             LEFT JOIN hash_jobs j ON j.occurrence_id = o.id
             LEFT JOIN occurrence_content oc ON oc.occurrence_id = o.id
             LEFT JOIN content_objects c ON c.id = oc.content_id
             WHERE o.snapshot_id = ?1
             ORDER BY o.relative_path",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<PlanningOccurrence>> = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (id, root, relative, name, size, scan, hash_status, hash_error, content, sha) =
                raw.map_err(db_err)?;
            Ok(PlanningOccurrence {
                occurrence_id: OccurrenceId::from_str(&id)?,
                source_root_id: df_domain::SourceRootId::from_str(&root)?,
                relative_path: relative,
                file_name: name,
                size_bytes: size as u64,
                scan_status: df_domain::ScanEntryStatus::parse(&scan)?,
                hash_status,
                hash_error,
                content_id: content.as_deref().map(ContentId::from_str).transpose()?,
                sha256: sha,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Next plan version for a project (1-based).
pub fn next_plan_version(db: &Db, project_id: ProjectId) -> DfResult<u32> {
    let max: Option<i64> = db
        .conn()
        .query_row(
            "SELECT MAX(version) FROM plans WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    Ok(max.unwrap_or(0) as u32 + 1)
}

/// Persist a freshly generated plan with its operations, superseding any
/// earlier non-approved plan, and emit `PLAN_CREATED` — one tx.
pub fn insert_plan(
    db: &mut Db,
    plan: &Plan,
    operations: &[PlanOperation],
    actor: Actor,
) -> DfResult<()> {
    if plan.status != PlanStatus::Ready {
        return Err(DfError::Validation(
            "plans are persisted in READY status after validation".to_string(),
        ));
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE plans SET status = 'SUPERSEDED'
         WHERE project_id = ?1 AND status IN ('DRAFT', 'READY')",
        [plan.project_id.to_string()],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO plans
            (id, project_id, snapshot_id, version, status, serialized_sha256,
             created_at, approved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            plan.id.to_string(),
            plan.project_id.to_string(),
            plan.snapshot_id.to_string(),
            plan.version as i64,
            plan.status.as_str(),
            plan.serialized_sha256,
            to_stored_timestamp(plan.created_at),
            plan.approved_at.map(to_stored_timestamp),
        ],
    )
    .map_err(db_err)?;
    let now = to_stored_timestamp(chrono::Utc::now());
    for op in operations {
        tx.execute(
            "INSERT INTO plan_operations
                (id, plan_id, sequence, operation_type, source_occurrence,
                 content_id, destination_relative_path, confidence, risk,
                 approval, execution_state, idempotency_key, reason,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
            params![
                op.id.to_string(),
                op.plan_id.to_string(),
                op.sequence as i64,
                op.operation_type.as_str(),
                op.source_occurrence.map(|id| id.to_string()),
                op.content_id.map(|id| id.to_string()),
                op.destination_relative_path,
                op.confidence,
                op.risk.as_str(),
                op.approval.as_str(),
                op.execution_state.as_str(),
                op.idempotency_key,
                op.reason,
                now,
            ],
        )
        .map_err(db_err)?;
    }
    let mut by_type = std::collections::BTreeMap::new();
    for op in operations {
        *by_type.entry(op.operation_type.as_str()).or_insert(0u64) += 1;
    }
    let payload = serde_json::json!({
        "plan_id": plan.id.to_string(),
        "snapshot_id": plan.snapshot_id.to_string(),
        "version": plan.version,
        "operations": operations.len(),
        "operations_by_type": by_type,
    });
    append_event(&tx, plan.project_id, EVENT_PLAN_CREATED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Raw `plans` row: id, project, snapshot, version, status, sha, created,
/// approved.
type PlanRow = (
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
    Option<String>,
);

fn plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlanRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn build_plan(raw: PlanRow) -> DfResult<Plan> {
    let (id, project, snapshot, version, status, sha, created, approved) = raw;
    Ok(Plan {
        id: PlanId::from_str(&id)?,
        project_id: ProjectId::from_str(&project)?,
        snapshot_id: SnapshotId::from_str(&snapshot)?,
        version: version as u32,
        status: PlanStatus::parse(&status)?,
        serialized_sha256: sha,
        created_at: parse_stored_timestamp(&created)?,
        approved_at: approved
            .as_deref()
            .map(parse_stored_timestamp)
            .transpose()?,
    })
}

const PLAN_COLUMNS: &str = "id, project_id, snapshot_id, version, status, serialized_sha256,
                            created_at, approved_at";

/// The newest plan of a project that is not superseded, if any.
pub fn current_plan(db: &Db, project_id: ProjectId) -> DfResult<Option<Plan>> {
    db.conn()
        .query_row(
            &format!(
                "SELECT {PLAN_COLUMNS} FROM plans
                 WHERE project_id = ?1 AND status != 'SUPERSEDED'
                 ORDER BY version DESC LIMIT 1"
            ),
            [project_id.to_string()],
            plan_from_row,
        )
        .optional()
        .map_err(db_err)?
        .map(build_plan)
        .transpose()
}

/// Every operation of a plan, ordered by sequence.
pub fn list_operations(db: &Db, plan_id: PlanId) -> DfResult<Vec<PlanOperation>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, plan_id, sequence, operation_type, source_occurrence,
                    content_id, destination_relative_path, confidence, risk,
                    approval, execution_state, idempotency_key, reason
             FROM plan_operations WHERE plan_id = ?1 ORDER BY sequence",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<PlanOperation>> = stmt
        .query_map([plan_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (
                id,
                plan,
                sequence,
                op_type,
                occurrence,
                content,
                destination,
                confidence,
                risk,
                approval,
                execution,
                key,
                reason,
            ) = raw.map_err(db_err)?;
            Ok(PlanOperation {
                id: OperationId::from_str(&id)?,
                plan_id: PlanId::from_str(&plan)?,
                sequence: sequence as u64,
                operation_type: OperationType::parse(&op_type)?,
                source_occurrence: occurrence
                    .as_deref()
                    .map(OccurrenceId::from_str)
                    .transpose()?,
                content_id: content.as_deref().map(ContentId::from_str).transpose()?,
                destination_relative_path: destination,
                confidence,
                risk: RiskLevel::parse(&risk)?,
                approval: ApprovalState::parse(&approval)?,
                execution_state: ExecutionState::parse(&execution)?,
                idempotency_key: key,
                reason,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Freeze a plan (§26.4): store the canonical hash, approve its operations
/// and emit `PLAN_APPROVED` — one tx.
/// Approve a plan: freeze its execution manifest, record the canonical hash
/// and flip the statuses — all in one transaction, so a plan can never be
/// APPROVED without the manifest that defines what "approved" means
/// (ADR-0018).
pub fn approve_plan(
    db: &mut Db,
    plan: &Plan,
    manifest_entries: &[ManifestEntry],
    serialized_sha256: &str,
    actor: Actor,
) -> DfResult<()> {
    if plan.status != PlanStatus::Ready {
        return Err(DfError::Validation(format!(
            "only READY plans can be approved (plan is {})",
            plan.status.as_str()
        )));
    }
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    insert_manifest(&tx, manifest_entries)?;
    tx.execute(
        "UPDATE plan_operations SET approval = 'APPROVED', updated_at = ?1
         WHERE plan_id = ?2 AND approval = 'PENDING'",
        params![to_stored_timestamp(chrono::Utc::now()), plan.id.to_string(),],
    )
    .map_err(db_err)?;
    let approved_at = chrono::Utc::now();
    tx.execute(
        "UPDATE plans SET status = 'APPROVED', serialized_sha256 = ?1, approved_at = ?2
         WHERE id = ?3",
        params![
            serialized_sha256,
            to_stored_timestamp(approved_at),
            plan.id.to_string(),
        ],
    )
    .map_err(db_err)?;
    let payload = serde_json::json!({
        "plan_id": plan.id.to_string(),
        "version": plan.version,
        "serialized_sha256": serialized_sha256,
    });
    append_event(&tx, plan.project_id, EVENT_PLAN_APPROVED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Gather, from the live inventory, everything the manifest must freeze.
///
/// This is the **only** place the live tables are read as an execution
/// contract, and it happens exactly once: at approval. From that moment the
/// manifest is the contract and these tables are just evidence (ADR-0018).
///
/// `source_root_identity` is left `None` here — df-db does not touch the
/// filesystem; the planner fills it in before hashing.
pub fn build_manifest_entries(db: &Db, plan_id: PlanId) -> DfResult<Vec<ManifestEntry>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT p.id, p.sequence, p.operation_type, p.idempotency_key,
                    p.destination_relative_path, o.source_root_id, r.absolute_path,
                    o.relative_path, o.fingerprint, o.size_bytes, c.sha256, c.blake3
             FROM plan_operations p
             LEFT JOIN path_occurrences o ON o.id = p.source_occurrence
             LEFT JOIN source_roots r ON r.id = o.source_root_id
             LEFT JOIN content_objects c ON c.id = p.content_id
             WHERE p.plan_id = ?1
             ORDER BY p.sequence",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<ManifestEntry>> = stmt
        .query_map([plan_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (
                operation_id,
                sequence,
                operation_type,
                idempotency_key,
                destination,
                source_root_id,
                root_path,
                relative,
                fingerprint,
                size,
                sha256,
                blake3,
            ) = raw.map_err(db_err)?;
            Ok(ManifestEntry {
                operation_id: OperationId::from_str(&operation_id)?,
                plan_id,
                sequence: sequence as u64,
                operation_type: OperationType::parse(&operation_type)?,
                idempotency_key,
                source_root_id: source_root_id
                    .as_deref()
                    .map(SourceRootId::from_str)
                    .transpose()?,
                source_root_identity: None,
                source_root_path_snapshot: root_path,
                source_relative_path_exact: relative,
                source_fingerprint: fingerprint,
                expected_size_bytes: size.map(|n| n as u64),
                expected_sha256: sha256,
                expected_blake3: blake3,
                destination_relative_path: destination,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Freeze the execution manifest of a plan (ADR-0018).
///
/// Written once, inside the approval transaction; the table's triggers reject
/// any later UPDATE or DELETE, so from here on the contract is fixed.
pub fn insert_manifest(tx: &Transaction<'_>, entries: &[ManifestEntry]) -> DfResult<()> {
    let now = to_stored_timestamp(chrono::Utc::now());
    for entry in entries {
        tx.execute(
            "INSERT INTO execution_manifest
                (operation_id, plan_id, sequence, operation_type, idempotency_key,
                 source_root_id, source_root_identity, source_root_path_snapshot,
                 source_relative_path_exact, source_fingerprint,
                 expected_size_bytes, expected_sha256, expected_blake3,
                 destination_relative_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                entry.operation_id.to_string(),
                entry.plan_id.to_string(),
                entry.sequence as i64,
                entry.operation_type.as_str(),
                entry.idempotency_key,
                entry.source_root_id.map(|id| id.to_string()),
                entry.source_root_identity,
                entry.source_root_path_snapshot,
                entry.source_relative_path_exact,
                entry.source_fingerprint,
                entry.expected_size_bytes.map(|n| n as i64),
                entry.expected_sha256,
                entry.expected_blake3,
                entry.destination_relative_path,
                now,
            ],
        )
        .map_err(db_err)?;
    }
    Ok(())
}

/// Read the frozen manifest of a plan, in sequence order.
///
/// This is what the verifier re-hashes and what `report` exports: the
/// authoritative record of what was approved.
pub fn manifest(db: &Db, plan_id: PlanId) -> DfResult<Vec<ManifestEntry>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT operation_id, plan_id, sequence, operation_type, idempotency_key,
                    source_root_id, source_root_identity, source_root_path_snapshot,
                    source_relative_path_exact, source_fingerprint,
                    expected_size_bytes, expected_sha256, expected_blake3,
                    destination_relative_path
             FROM execution_manifest WHERE plan_id = ?1 ORDER BY sequence",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<ManifestEntry>> = stmt
        .query_map([plan_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (
                operation_id,
                plan_id,
                sequence,
                operation_type,
                idempotency_key,
                source_root_id,
                source_root_identity,
                source_root_path_snapshot,
                source_relative_path_exact,
                source_fingerprint,
                expected_size_bytes,
                expected_sha256,
                expected_blake3,
                destination_relative_path,
            ) = raw.map_err(db_err)?;
            Ok(ManifestEntry {
                operation_id: OperationId::from_str(&operation_id)?,
                plan_id: PlanId::from_str(&plan_id)?,
                sequence: sequence as u64,
                operation_type: OperationType::parse(&operation_type)?,
                idempotency_key,
                source_root_id: source_root_id
                    .as_deref()
                    .map(SourceRootId::from_str)
                    .transpose()?,
                source_root_identity,
                source_root_path_snapshot,
                source_relative_path_exact,
                source_fingerprint,
                expected_size_bytes: expected_size_bytes.map(|n| n as u64),
                expected_sha256,
                expected_blake3,
                destination_relative_path,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Everything the executor needs to run one operation.
#[derive(Debug, Clone)]
pub struct ExecutableOperation {
    pub operation_id: OperationId,
    pub sequence: u64,
    pub operation_type: OperationType,
    pub destination_relative_path: String,
    /// Source file, for copies: absolute root + relative path.
    pub source_root_path: Option<PathBuf>,
    pub source_relative_path: Option<String>,
    /// Fingerprint captured at scan time (§27.1 "validate source fingerprint").
    pub source_fingerprint: Option<String>,
    pub size_bytes: u64,
    /// Hashes the copy must reproduce (§27.1 "compare").
    pub expected_sha256: Option<String>,
    pub expected_blake3: Option<String>,
}

/// Approved, executable operations not yet in a terminal state.
///
/// `RUNNING` rows are included: they belong to a run that died mid-copy and
/// are safe to retry (the executor overwrites its own partial file).
///
/// **Reads only the frozen manifest** (ADR-0018). It deliberately does not
/// join `path_occurrences`, `source_roots` or `content_objects`: those are
/// live, mutable evidence, and resolving execution material from them is what
/// let a post-approval edit change the executed bytes without moving the plan
/// hash (threat T5). `plan_operations` is still joined, but only for the
/// mutable *progress* columns (approval, execution_state) — never for what to
/// read or expect.
pub fn executable_operations(
    db: &Db,
    plan_id: PlanId,
    limit: u32,
) -> DfResult<Vec<ExecutableOperation>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT m.operation_id, m.sequence, m.operation_type,
                    m.destination_relative_path, m.source_root_path_snapshot,
                    m.source_relative_path_exact, m.source_fingerprint,
                    m.expected_size_bytes, m.expected_sha256, m.expected_blake3
             FROM execution_manifest m
             JOIN plan_operations p ON p.id = m.operation_id
             WHERE m.plan_id = ?1
               AND p.approval = 'APPROVED'
               AND m.operation_type IN
                   ('COPY_ACTIVE', 'COPY_REVIEW', 'COPY_SEPARATED', 'COPY_TEMPORARY',
                    'COPY_WITH_SUFFIX', 'CREATE_DIRECTORY')
               AND p.execution_state IN ('PENDING', 'RUNNING', 'FAILED_RETRYABLE')
             ORDER BY m.sequence
             LIMIT ?2",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<ExecutableOperation>> = stmt
        .query_map(params![plan_id.to_string(), limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (id, sequence, op_type, destination, root, relative, fingerprint, size, sha, blake) =
                raw.map_err(db_err)?;
            let destination = destination.ok_or_else(|| {
                DfError::Database(format!("executable operation {id} has no destination"))
            })?;
            Ok(ExecutableOperation {
                operation_id: OperationId::from_str(&id)?,
                sequence: sequence as u64,
                operation_type: OperationType::parse(&op_type)?,
                destination_relative_path: destination,
                source_root_path: root.map(PathBuf::from),
                source_relative_path: relative,
                source_fingerprint: fingerprint,
                size_bytes: size.unwrap_or(0) as u64,
                expected_sha256: sha,
                expected_blake3: blake,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Mark an operation `RUNNING` before touching the filesystem, so a crash
/// leaves a retryable trace (§27.4).
pub fn mark_operation_running(db: &Db, operation_id: OperationId) -> DfResult<()> {
    db.conn()
        .execute(
            "UPDATE plan_operations SET execution_state = 'RUNNING', updated_at = ?1
             WHERE id = ?2",
            params![
                to_stored_timestamp(chrono::Utc::now()),
                operation_id.to_string(),
            ],
        )
        .map_err(db_err)?;
    Ok(())
}

/// Result of one execution attempt, journaled append-only (§27.1).
#[derive(Debug, Clone)]
pub struct OperationOutcome {
    pub execution_state: ExecutionState,
    /// Short outcome tag: `COPIED`, `DIRECTORY_CREATED`, `SKIP_REPRESENTED`,
    /// or an error code.
    pub outcome: String,
    pub error_code: Option<OperationErrorCode>,
    pub detail: Option<String>,
    pub final_relative_path: Option<String>,
    pub bytes_copied: u64,
    pub sha256: Option<String>,
    pub blake3: Option<String>,
    pub started_at: df_domain::Timestamp,
}

/// Persist the outcome of one attempt: journal row + state change — one tx.
pub fn record_operation_outcome(
    db: &mut Db,
    operation_id: OperationId,
    outcome: &OperationOutcome,
) -> DfResult<()> {
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO operation_results
            (id, operation_id, outcome, error_code, detail, final_relative_path,
             bytes_copied, sha256, blake3, started_at, finished_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            uuid::Uuid::new_v4().to_string(),
            operation_id.to_string(),
            outcome.outcome,
            outcome.error_code.map(|c| c.as_str()),
            outcome.detail,
            outcome.final_relative_path,
            outcome.bytes_copied as i64,
            outcome.sha256,
            outcome.blake3,
            to_stored_timestamp(outcome.started_at),
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE plan_operations SET execution_state = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            outcome.execution_state.as_str(),
            to_stored_timestamp(chrono::Utc::now()),
            operation_id.to_string(),
        ],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Execution progress of a plan, by state.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PlanProgress {
    pub total: u64,
    pub executable: u64,
    pub pending: u64,
    pub running: u64,
    pub completed: u64,
    pub failed_retryable: u64,
    pub failed_final: u64,
    pub blocked: u64,
}

pub fn plan_progress(db: &Db, plan_id: PlanId) -> DfResult<PlanProgress> {
    let mut progress = PlanProgress::default();
    db.conn()
        .query_row(
            "SELECT
                COUNT(*),
                COUNT(*) FILTER (WHERE operation_type IN
                    ('COPY_ACTIVE', 'COPY_REVIEW', 'COPY_SEPARATED', 'COPY_TEMPORARY',
                     'COPY_WITH_SUFFIX', 'CREATE_DIRECTORY')),
                COUNT(*) FILTER (WHERE execution_state = 'PENDING'),
                COUNT(*) FILTER (WHERE execution_state = 'RUNNING'),
                COUNT(*) FILTER (WHERE execution_state = 'COMPLETED'),
                COUNT(*) FILTER (WHERE execution_state = 'FAILED_RETRYABLE'),
                COUNT(*) FILTER (WHERE execution_state = 'FAILED_FINAL'),
                COUNT(*) FILTER (WHERE execution_state = 'BLOCKED')
             FROM plan_operations WHERE plan_id = ?1",
            [plan_id.to_string()],
            |row| {
                progress.total = row.get::<_, i64>(0)? as u64;
                progress.executable = row.get::<_, i64>(1)? as u64;
                progress.pending = row.get::<_, i64>(2)? as u64;
                progress.running = row.get::<_, i64>(3)? as u64;
                progress.completed = row.get::<_, i64>(4)? as u64;
                progress.failed_retryable = row.get::<_, i64>(5)? as u64;
                progress.failed_final = row.get::<_, i64>(6)? as u64;
                progress.blocked = row.get::<_, i64>(7)? as u64;
                Ok(())
            },
        )
        .map_err(db_err)?;
    Ok(progress)
}

/// A completed materialisation, as the verifier re-checks it (§28).
#[derive(Debug, Clone)]
pub struct VerifiableArtefact {
    pub operation_id: OperationId,
    pub operation_type: OperationType,
    /// Where the artefact actually landed (last successful attempt).
    pub final_relative_path: String,
    pub expected_sha256: Option<String>,
    pub size_bytes: u64,
}

/// Final landing spot and expected identity of every COMPLETED executable
/// operation of a plan.
pub fn verifiable_artefacts(db: &Db, plan_id: PlanId) -> DfResult<Vec<VerifiableArtefact>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT p.id, p.operation_type, res.final_relative_path, c.sha256,
                    COALESCE(c.size_bytes, 0)
             FROM plan_operations p
             LEFT JOIN content_objects c ON c.id = p.content_id
             JOIN operation_results res ON res.operation_id = p.id
             WHERE p.plan_id = ?1
               AND p.execution_state = 'COMPLETED'
               AND res.final_relative_path IS NOT NULL
               AND res.id = (
                   SELECT r2.id FROM operation_results r2
                   WHERE r2.operation_id = p.id AND r2.final_relative_path IS NOT NULL
                   ORDER BY r2.finished_at DESC, r2.id DESC LIMIT 1
               )
             ORDER BY p.sequence",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<VerifiableArtefact>> = stmt
        .query_map([plan_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (id, op_type, path, sha, size) = raw.map_err(db_err)?;
            Ok(VerifiableArtefact {
                operation_id: OperationId::from_str(&id)?,
                operation_type: OperationType::parse(&op_type)?,
                final_relative_path: path,
                expected_sha256: sha,
                size_bytes: size as u64,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// One verification finding (§28), persisted with its run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerificationFinding {
    pub kind: String,
    /// `PROBLEM` fails the verification; `WARNING` degrades it.
    pub severity: String,
    pub subject: String,
    pub detail: String,
}

/// Persist a verification run with its findings and emit
/// `VERIFICATION_COMPLETED` — one tx.
#[allow(clippy::too_many_arguments)]
pub fn record_verification_run(
    db: &mut Db,
    project_id: ProjectId,
    plan_id: PlanId,
    verdict: &str,
    checked: u64,
    findings: &[VerificationFinding],
    started_at: df_domain::Timestamp,
    actor: Actor,
) -> DfResult<VerificationRunId> {
    let run_id = VerificationRunId::new();
    let problems = findings.iter().filter(|f| f.severity == "PROBLEM").count() as u64;
    let warnings = findings.iter().filter(|f| f.severity == "WARNING").count() as u64;
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let now = to_stored_timestamp(chrono::Utc::now());
    tx.execute(
        "INSERT INTO verification_runs
            (id, project_id, plan_id, verdict, checked, problems, warnings,
             started_at, finished_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            run_id.to_string(),
            project_id.to_string(),
            plan_id.to_string(),
            verdict,
            checked as i64,
            problems as i64,
            warnings as i64,
            to_stored_timestamp(started_at),
            now,
        ],
    )
    .map_err(db_err)?;
    for finding in findings {
        tx.execute(
            "INSERT INTO verification_findings
                (id, verification_run_id, kind, severity, subject, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                FindingId::new().to_string(),
                run_id.to_string(),
                finding.kind,
                finding.severity,
                finding.subject,
                finding.detail,
                now,
            ],
        )
        .map_err(db_err)?;
    }
    let payload = serde_json::json!({
        "verification_run_id": run_id.to_string(),
        "plan_id": plan_id.to_string(),
        "verdict": verdict,
        "checked": checked,
        "problems": problems,
        "warnings": warnings,
    });
    append_event(
        &tx,
        project_id,
        EVENT_VERIFICATION_COMPLETED,
        &payload,
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    Ok(run_id)
}

/// Number of exact-duplicate sets recorded for a snapshot.
pub fn duplicate_set_count(db: &Db, snapshot_id: SnapshotId) -> DfResult<u64> {
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM duplicate_sets WHERE snapshot_id = ?1",
            [snapshot_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    Ok(count as u64)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use df_domain::{ProfileRef, Project, SourceRoot};

    use crate::repository;

    use super::*;

    fn project_with_plan(db: &mut Db) -> (Plan, Vec<PlanOperation>) {
        let project = Project::new(
            "p",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, PathBuf::from("D:/in"))];
        repository::create_project(db, &project, &roots, Actor::Test).unwrap();
        let (snapshot, _run) = crate::inventory::start_scan(db, project.id, Actor::Test).unwrap();

        let mut plan = Plan::new(project.id, snapshot.id, 1);
        plan.status = PlanStatus::Ready;
        let operations = vec![PlanOperation {
            id: OperationId::new(),
            plan_id: plan.id,
            sequence: 1,
            operation_type: OperationType::CreateDirectory,
            source_occurrence: None,
            content_id: None,
            destination_relative_path: Some("origen".to_string()),
            confidence: 1.0,
            risk: RiskLevel::Low,
            approval: ApprovalState::Pending,
            execution_state: ExecutionState::Pending,
            idempotency_key: "0".repeat(64),
            reason: "test".to_string(),
        }];
        insert_plan(db, &plan, &operations, Actor::Test).unwrap();
        (plan, operations)
    }

    #[test]
    fn plans_round_trip_and_freeze_on_approval() {
        let mut db = Db::open_in_memory().unwrap();
        let (plan, _ops) = project_with_plan(&mut db);

        let loaded = current_plan(&db, plan.project_id).unwrap().unwrap();
        assert_eq!(loaded.id, plan.id);
        assert_eq!(loaded.status, PlanStatus::Ready);
        assert_eq!(list_operations(&db, plan.id).unwrap().len(), 1);

        // Approving now freezes the manifest in the same transaction, so the
        // entries travel with the approval (ADR-0018).
        let entries = build_manifest_entries(&db, plan.id).unwrap();
        approve_plan(&mut db, &loaded, &entries, &"a".repeat(64), Actor::Test).unwrap();
        let approved = current_plan(&db, plan.project_id).unwrap().unwrap();
        assert_eq!(approved.status, PlanStatus::Approved);
        assert!(approved.approved_at.is_some());

        // Trigger: frozen fields of an approved plan reject updates.
        let frozen = db.conn().execute(
            "UPDATE plan_operations SET destination_relative_path = 'x' WHERE plan_id = ?1",
            [plan.id.to_string()],
        );
        assert!(frozen.is_err(), "approved plans must be immutable");
        // Execution progress is still allowed.
        db.conn()
            .execute(
                "UPDATE plan_operations SET execution_state = 'COMPLETED' WHERE plan_id = ?1",
                [plan.id.to_string()],
            )
            .expect("execution progress stays writable");
    }

    #[test]
    fn plans_and_operations_reject_deletion() {
        let mut db = Db::open_in_memory().unwrap();
        let (plan, _ops) = project_with_plan(&mut db);
        assert!(db
            .conn()
            .execute("DELETE FROM plan_operations", [])
            .is_err());
        assert!(db.conn().execute("DELETE FROM plans", []).is_err());
        let _ = plan;
    }

    #[test]
    fn operation_results_are_append_only() {
        let mut db = Db::open_in_memory().unwrap();
        let (plan, ops) = project_with_plan(&mut db);
        let outcome = OperationOutcome {
            execution_state: ExecutionState::Completed,
            outcome: "DIRECTORY_CREATED".to_string(),
            error_code: None,
            detail: None,
            final_relative_path: Some("origen".to_string()),
            bytes_copied: 0,
            sha256: None,
            blake3: None,
            started_at: chrono::Utc::now(),
        };
        record_operation_outcome(&mut db, ops[0].id, &outcome).unwrap();
        assert!(db
            .conn()
            .execute("UPDATE operation_results SET outcome = 'FORGED'", [])
            .is_err());
        assert!(db
            .conn()
            .execute("DELETE FROM operation_results", [])
            .is_err());
        let progress = plan_progress(&db, plan.id).unwrap();
        assert_eq!(progress.completed, 1);
    }
}
