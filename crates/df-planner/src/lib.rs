//! Analysis and planning (RFC-0001 §12.4–§12.8, §15, §26).
//!
//! Milestone 0.1 policy is `REPORT_ONLY` (§15.4): the plan mirrors the
//! source structure into the output root and copies everything that was
//! hashed, including duplicates. Duplicate sets are materialised as
//! evidence; no consolidation happens until profiles and contexts exist
//! (Milestone 0.2). Every occurrence is covered by exactly one operation
//! (§26.2) and approval freezes the plan under a canonical SHA-256 (§26.4).

use std::collections::{BTreeMap, HashMap, HashSet};

use df_db::{plans, repository, Db};
use df_domain::{
    Actor, ApprovalState, ExecutionState, OperationType, Plan, PlanOperation, PlanStatus,
    ProjectState, RiskLevel, SourceRootId,
};
use df_error::{DfError, DfResult};
use serde::Serialize;
use sha2::Digest;

/// Result of the analysis phase.
#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeOutcome {
    pub snapshot_id: String,
    pub duplicate_sets: u64,
    /// Folders that received a Merkle signature (RFC-0001 §19.2).
    pub folder_signatures: u64,
    /// Groups of folders whose subtrees are byte-for-byte identical (§19.3).
    pub tree_clone_sets: u64,
    /// Folders tagged as low-value generic containers (RFC-0001 §18.3).
    pub generic_folders: u64,
    /// Duplicate sets that got a logical representative (RFC-0001 §15.5).
    pub duplicate_representatives: u64,
    pub state: String,
}

/// Result of plan generation.
#[derive(Debug, Clone, Serialize)]
pub struct PlanOutcome {
    pub plan_id: String,
    pub snapshot_id: String,
    pub version: u32,
    pub operations: u64,
    pub copies: u64,
    pub directories: u64,
    pub no_action: u64,
    pub blocked: u64,
    pub state: String,
}

/// Result of approving a plan.
#[derive(Debug, Clone, Serialize)]
pub struct ApproveOutcome {
    pub plan_id: String,
    pub version: u32,
    pub serialized_sha256: String,
    pub operations_approved: u64,
    pub state: String,
}

/// Report of `plan validate`.
#[derive(Debug, Clone, Serialize)]
pub struct PlanValidationReport {
    pub plan_id: String,
    pub version: u32,
    pub status: String,
    pub operations: u64,
    pub ok: bool,
    pub problems: Vec<String>,
}

/// Analyse the hashed snapshot: materialise exact duplicate sets (§15),
/// compute folder signatures and tree clones (§19), and move the project
/// `HASHED → ANALYZING → ANALYZED`.
pub fn analyze_project(db: &mut Db, actor: Actor) -> DfResult<AnalyzeOutcome> {
    let project = repository::load_project(db)?;
    if project.state != ProjectState::Hashed {
        return Err(DfError::Validation(format!(
            "cannot analyze a project in state {} (expected HASHED)",
            project.state
        )));
    }
    let snapshot = df_db::inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;

    repository::update_project_state(db, ProjectState::Analyzing, actor)?;
    let duplicate_sets = plans::materialize_duplicate_sets(db, project.id, snapshot.id, actor)?;
    let structure =
        df_db::structure::compute_folder_signatures(db, project.id, snapshot.id, actor)?;
    let context = df_db::context::classify_folders(
        db,
        project.id,
        snapshot.id,
        project.profile.as_str(),
        actor,
    )?;
    // Representatives need the context penalties, so this runs last.
    let duplicate_representatives =
        df_db::dedup::score_duplicate_representatives(db, project.id, snapshot.id, actor)?;
    let project = repository::update_project_state(db, ProjectState::Analyzed, actor)?;

    Ok(AnalyzeOutcome {
        snapshot_id: snapshot.id.to_string(),
        duplicate_sets,
        folder_signatures: structure.folders_signed,
        tree_clone_sets: structure.tree_clone_sets,
        generic_folders: context.generic_folders,
        duplicate_representatives,
        state: project.state.as_str().to_string(),
    })
}

/// Generate, validate and persist the plan for the analyzed snapshot;
/// moves the project `ANALYZED → PLANNING → PLAN_READY`.
pub fn create_plan(db: &mut Db, actor: Actor) -> DfResult<PlanOutcome> {
    let project = repository::load_project(db)?;
    if project.state != ProjectState::Analyzed {
        return Err(DfError::Validation(format!(
            "cannot plan a project in state {} (expected ANALYZED)",
            project.state
        )));
    }
    let snapshot = df_db::inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;

    repository::update_project_state(db, ProjectState::Planning, actor)?;

    let version = plans::next_plan_version(db, project.id)?;
    let mut plan = Plan::new(project.id, snapshot.id, version);
    let operations = build_operations(db, &plan)?;
    validate_operations(&operations)?;
    plan.status = PlanStatus::Ready;
    plans::insert_plan(db, &plan, &operations, actor)?;

    let project = repository::update_project_state(db, ProjectState::PlanReady, actor)?;

    let count =
        |t: OperationType| operations.iter().filter(|o| o.operation_type == t).count() as u64;
    Ok(PlanOutcome {
        plan_id: plan.id.to_string(),
        snapshot_id: snapshot.id.to_string(),
        version,
        operations: operations.len() as u64,
        copies: count(OperationType::CopyActive) + count(OperationType::CopyWithSuffix),
        directories: count(OperationType::CreateDirectory),
        no_action: count(OperationType::NoAction),
        blocked: count(OperationType::Blocked),
        state: project.state.as_str().to_string(),
    })
}

/// Re-run the §26.5 invariants against the stored current plan.
pub fn validate_plan(db: &Db) -> DfResult<PlanValidationReport> {
    let project = repository::load_project(db)?;
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;
    let operations = plans::list_operations(db, plan.id)?;
    let problems = match validate_operations(&operations) {
        Ok(()) => Vec::new(),
        Err(DfError::Validation(message)) => vec![message],
        Err(other) => return Err(other),
    };
    // Coverage against the snapshot (§26.2).
    let occurrences = plans::planning_occurrences(db, plan.snapshot_id)?;
    let covered: HashSet<String> = operations
        .iter()
        .filter_map(|op| op.source_occurrence.map(|id| id.to_string()))
        .collect();
    let mut problems = problems;
    for occurrence in &occurrences {
        if !covered.contains(&occurrence.occurrence_id.to_string()) {
            problems.push(format!(
                "occurrence `{}` is not covered by the plan",
                occurrence.relative_path
            ));
        }
    }
    Ok(PlanValidationReport {
        plan_id: plan.id.to_string(),
        version: plan.version,
        status: plan.status.as_str().to_string(),
        operations: operations.len() as u64,
        ok: problems.is_empty(),
        problems,
    })
}

/// Approve the current READY plan (§26.4): validate, canonically serialize,
/// hash, freeze; moves the project `PLAN_READY → PLAN_REVIEW → PLAN_APPROVED`.
pub fn approve_plan(db: &mut Db, actor: Actor) -> DfResult<ApproveOutcome> {
    let project = repository::load_project(db)?;
    if project.state != ProjectState::PlanReady {
        return Err(DfError::Validation(format!(
            "cannot approve a plan in project state {} (expected PLAN_READY)",
            project.state
        )));
    }
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;
    let operations = plans::list_operations(db, plan.id)?;
    validate_operations(&operations)?;

    repository::update_project_state(db, ProjectState::PlanReview, actor)?;
    let sha256 = plan_operations_sha256(&operations);
    plans::approve_plan(db, &plan, &sha256, actor)?;
    let project = repository::update_project_state(db, ProjectState::PlanApproved, actor)?;

    Ok(ApproveOutcome {
        plan_id: plan.id.to_string(),
        version: plan.version,
        serialized_sha256: sha256,
        operations_approved: operations.len() as u64,
        state: project.state.as_str().to_string(),
    })
}

/// Canonical serialization of the approval-covered operation fields.
///
/// Shared with the verifier, which recomputes this hash to prove the plan
/// was not modified after approval (§26.4, §28.2).
pub fn serialize_plan_operations(operations: &[PlanOperation]) -> String {
    let items: Vec<serde_json::Value> = operations
        .iter()
        .map(|op| {
            serde_json::json!({
                "sequence": op.sequence,
                "operation_type": op.operation_type.as_str(),
                "source_occurrence": op.source_occurrence.map(|id| id.to_string()),
                "content_id": op.content_id.map(|id| id.to_string()),
                "destination": op.destination_relative_path,
                "idempotency_key": op.idempotency_key,
            })
        })
        .collect();
    df_ledger::canonical_json(&serde_json::Value::Array(items))
}

/// SHA-256 of [`serialize_plan_operations`].
pub fn plan_operations_sha256(operations: &[PlanOperation]) -> String {
    hex::encode(sha2::Sha256::digest(
        serialize_plan_operations(operations).as_bytes(),
    ))
}

/// Deterministic idempotency key (§26.3).
fn idempotency_key(
    plan: &Plan,
    occurrence: Option<&str>,
    operation_type: OperationType,
    destination: Option<&str>,
) -> String {
    let value = serde_json::json!({
        "project": plan.project_id.to_string(),
        "snapshot": plan.snapshot_id.to_string(),
        "plan_version": plan.version,
        "occurrence": occurrence,
        "operation": operation_type.as_str(),
        "destination": destination,
    });
    hex::encode(sha2::Sha256::digest(
        df_ledger::canonical_json(&value).as_bytes(),
    ))
}

/// Destination top-level directory per source root: the root's folder name,
/// disambiguated deterministically when several roots share one.
fn root_destination_dirs(roots: &[df_domain::SourceRoot]) -> HashMap<SourceRootId, String> {
    let mut used: HashSet<String> = HashSet::new();
    let mut mapping = HashMap::new();
    for root in roots {
        let base = root
            .absolute_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "origen".to_string());
        let mut candidate = base.clone();
        let mut suffix = 1;
        while !used.insert(candidate.to_lowercase()) {
            suffix += 1;
            candidate = format!("{base}-{suffix}");
        }
        mapping.insert(root.id, candidate);
    }
    mapping
}

/// Deterministic collision suffix (§27.3, applied at plan time): the first
/// 8 hex chars of the content SHA-256 before the extension.
fn suffixed_destination(destination: &str, sha256: &str) -> String {
    let tag = &sha256[..8.min(sha256.len())];
    match destination.rfind('.') {
        // Only suffix a real extension: a dot inside the last path segment.
        Some(dot)
            if dot > 0
                && !destination[dot..].contains(std::path::MAIN_SEPARATOR)
                && destination[..dot]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c != std::path::MAIN_SEPARATOR) =>
        {
            format!("{}~df-{tag}{}", &destination[..dot], &destination[dot..])
        }
        _ => format!("{destination}~df-{tag}"),
    }
}

fn build_operations(db: &Db, plan: &Plan) -> DfResult<Vec<PlanOperation>> {
    let roots = repository::load_source_roots(db, plan.project_id)?;
    let root_dirs = root_destination_dirs(&roots);
    let folders = df_db::inventory::list_folders(db, plan.snapshot_id)?;
    let occurrences = plans::planning_occurrences(db, plan.snapshot_id)?;

    let sep = std::path::MAIN_SEPARATOR;
    let destination_for = |root_id: SourceRootId, relative: &str| -> Option<String> {
        let dir = root_dirs.get(&root_id)?;
        Some(if relative.is_empty() {
            dir.clone()
        } else {
            format!("{dir}{sep}{relative}")
        })
    };

    let mut operations: Vec<PlanOperation> = Vec::new();
    let mut sequence: u64 = 0;
    let mut taken_destinations: HashSet<String> = HashSet::new();
    let next = |seq: &mut u64| {
        *seq += 1;
        *seq
    };

    // Folders first, parents before children (list_folders orders by depth).
    for folder in &folders {
        let (operation_type, destination, risk, reason) = match folder.status {
            df_domain::ScanEntryStatus::Ok => {
                let destination = destination_for(folder.source_root_id, &folder.relative_path)
                    .ok_or_else(|| {
                        DfError::Database(format!(
                            "folder `{}` references an unknown source root",
                            folder.relative_path
                        ))
                    })?;
                (
                    OperationType::CreateDirectory,
                    Some(destination),
                    RiskLevel::Low,
                    "mirror source directory structure (REPORT_ONLY policy)".to_string(),
                )
            }
            df_domain::ScanEntryStatus::ReparseNotFollowed => (
                OperationType::NoAction,
                None,
                RiskLevel::Low,
                format!(
                    "reparse point `{}` recorded but not followed by policy (RFC-0001 §13.6)",
                    folder.relative_path
                ),
            ),
            df_domain::ScanEntryStatus::Error => (
                OperationType::Blocked,
                None,
                RiskLevel::Medium,
                format!(
                    "directory `{}` could not be read: {}",
                    folder.relative_path,
                    folder.error.as_deref().unwrap_or("unknown error")
                ),
            ),
        };
        if let Some(dest) = &destination {
            taken_destinations.insert(dest.to_lowercase());
        }
        operations.push(PlanOperation {
            id: df_domain::OperationId::new(),
            plan_id: plan.id,
            sequence: next(&mut sequence),
            operation_type,
            source_occurrence: None,
            content_id: None,
            confidence: 1.0,
            risk,
            approval: ApprovalState::Pending,
            execution_state: initial_execution_state(operation_type),
            idempotency_key: idempotency_key(plan, None, operation_type, destination.as_deref()),
            destination_relative_path: destination,
            reason,
        });
    }

    for occurrence in &occurrences {
        let occurrence_key = occurrence.occurrence_id.to_string();
        let (operation_type, destination, risk, reason) = match occurrence.scan_status {
            df_domain::ScanEntryStatus::ReparseNotFollowed => (
                OperationType::NoAction,
                None,
                RiskLevel::Low,
                format!(
                    "reparse point `{}` recorded but not followed by policy (RFC-0001 §13.6)",
                    occurrence.relative_path
                ),
            ),
            df_domain::ScanEntryStatus::Error => (
                OperationType::Blocked,
                None,
                RiskLevel::High,
                format!(
                    "file `{}` could not be inventoried: {}",
                    occurrence.relative_path,
                    occurrence.hash_error.as_deref().unwrap_or("scan error")
                ),
            ),
            df_domain::ScanEntryStatus::Ok => match (&occurrence.content_id, &occurrence.sha256) {
                (Some(_), Some(sha256)) => {
                    let planned =
                        destination_for(occurrence.source_root_id, &occurrence.relative_path)
                            .ok_or_else(|| {
                                DfError::Database(format!(
                                    "occurrence `{}` references an unknown source root",
                                    occurrence.relative_path
                                ))
                            })?;
                    // Same-destination collision (e.g. names differing only
                    // by case): deterministic suffix, decided at plan time.
                    if taken_destinations.insert(planned.to_lowercase()) {
                        (
                            OperationType::CopyActive,
                            Some(planned),
                            RiskLevel::Low,
                            "verified copy preserving source structure (REPORT_ONLY policy)"
                                .to_string(),
                        )
                    } else {
                        let suffixed = suffixed_destination(&planned, sha256);
                        taken_destinations.insert(suffixed.to_lowercase());
                        (
                            OperationType::CopyWithSuffix,
                            Some(suffixed),
                            RiskLevel::Medium,
                            format!(
                                "destination `{planned}` collides with another occurrence; \
                                 deterministic suffix applied (RFC-0001 §27.3)"
                            ),
                        )
                    }
                }
                _ => (
                    OperationType::Blocked,
                    None,
                    RiskLevel::High,
                    format!(
                        "file `{}` has no verified content identity ({}): {}",
                        occurrence.relative_path,
                        occurrence.hash_status.as_deref().unwrap_or("no hash job"),
                        occurrence.hash_error.as_deref().unwrap_or("not hashed")
                    ),
                ),
            },
        };
        operations.push(PlanOperation {
            id: df_domain::OperationId::new(),
            plan_id: plan.id,
            sequence: next(&mut sequence),
            operation_type,
            source_occurrence: Some(occurrence.occurrence_id),
            content_id: occurrence.content_id,
            confidence: 1.0,
            risk,
            approval: ApprovalState::Pending,
            execution_state: initial_execution_state(operation_type),
            idempotency_key: idempotency_key(
                plan,
                Some(&occurrence_key),
                operation_type,
                destination.as_deref(),
            ),
            destination_relative_path: destination,
            reason,
        });
    }

    Ok(operations)
}

fn initial_execution_state(operation_type: OperationType) -> ExecutionState {
    match operation_type {
        OperationType::Blocked => ExecutionState::Blocked,
        t if t.is_executable() => ExecutionState::Pending,
        // NO_ACTION and other non-executable coverage entries are complete
        // by definition: there is nothing to run.
        _ => ExecutionState::Completed,
    }
}

/// §26.5 invariants that are checkable without touching the filesystem.
fn validate_operations(operations: &[PlanOperation]) -> DfResult<()> {
    let mut destinations: BTreeMap<String, u64> = BTreeMap::new();
    for op in operations {
        match &op.destination_relative_path {
            Some(destination) => {
                if destination.is_empty() {
                    return Err(DfError::Validation(format!(
                        "operation #{} has an empty destination",
                        op.sequence
                    )));
                }
                let path = std::path::Path::new(destination);
                if path.is_absolute()
                    || path.components().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir | std::path::Component::Prefix(_)
                        )
                    })
                {
                    return Err(DfError::Validation(format!(
                        "operation #{} escapes the output root: `{destination}` (RFC-0001 §26.5)",
                        op.sequence
                    )));
                }
                if op.operation_type != OperationType::CreateDirectory {
                    if let Some(previous) =
                        destinations.insert(destination.to_lowercase(), op.sequence)
                    {
                        return Err(DfError::Validation(format!(
                            "operations #{previous} and #{} collide on destination `{destination}`",
                            op.sequence
                        )));
                    }
                }
            }
            None => {
                if op.operation_type.is_executable() {
                    return Err(DfError::Validation(format!(
                        "executable operation #{} has no destination",
                        op.sequence
                    )));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use df_domain::{ProfileRef, Project, SnapshotId, SourceRoot};
    use df_hash::{hash_project, HashOptions};
    use df_scan::{scan_project, ScanOptions};

    use super::*;

    fn hashed_project(tmp: &Path) -> Db {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(origin.join("sub")).unwrap();
        std::fs::write(origin.join("a.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("sub").join("b.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("c.txt"), b"different").unwrap();

        let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Prueba plan",
            ProfileRef::default(),
            tmp.join("salida"),
            tmp.join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        db
    }

    fn analyzed_and_planned(db: &mut Db) -> PlanOutcome {
        analyze_project(db, Actor::Test).unwrap();
        create_plan(db, Actor::Test).unwrap()
    }

    #[test]
    fn analyze_materialises_duplicate_sets_and_reaches_analyzed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let outcome = analyze_project(&mut db, Actor::Test).unwrap();
        assert_eq!(outcome.duplicate_sets, 1);
        assert_eq!(outcome.state, "ANALYZED");
    }

    #[test]
    fn plan_covers_every_occurrence_and_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let outcome = analyzed_and_planned(&mut db);

        // 3 files → copies; 2 folders (origen + sub) → directories.
        assert_eq!(outcome.copies, 3);
        assert_eq!(outcome.directories, 2);
        assert_eq!(outcome.blocked, 0);
        assert_eq!(outcome.operations, 5);
        assert_eq!(outcome.state, "PLAN_READY");

        let report = validate_plan(&db).unwrap();
        assert!(report.ok, "{:?}", report.problems);
    }

    #[test]
    fn plan_destinations_mirror_the_source_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyzed_and_planned(&mut db);

        let project = repository::load_project(&db).unwrap();
        let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
        let ops = plans::list_operations(&db, plan.id).unwrap();
        let destinations: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.destination_relative_path.as_deref())
            .collect();
        let sep = std::path::MAIN_SEPARATOR;
        assert!(destinations.contains(&"origen"));
        assert!(destinations.contains(&format!("origen{sep}sub").as_str()));
        assert!(destinations.contains(&format!("origen{sep}a.txt").as_str()));
        assert!(destinations.contains(&format!("origen{sep}sub{sep}b.txt").as_str()));
        // Explainability: every operation carries a reason.
        assert!(ops.iter().all(|o| !o.reason.is_empty()));
    }

    #[test]
    fn approval_freezes_the_plan_with_a_canonical_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyzed_and_planned(&mut db);

        let approved = approve_plan(&mut db, Actor::Test).unwrap();
        assert_eq!(approved.state, "PLAN_APPROVED");
        assert_eq!(approved.serialized_sha256.len(), 64);

        let project = repository::load_project(&db).unwrap();
        let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
        assert_eq!(plan.status, PlanStatus::Approved);
        assert_eq!(
            plan.serialized_sha256.as_deref(),
            Some(approved.serialized_sha256.as_str())
        );

        // The stored operations re-serialize to the same hash.
        let ops = plans::list_operations(&db, plan.id).unwrap();
        assert_eq!(plan_operations_sha256(&ops), approved.serialized_sha256);
        assert!(ops.iter().all(|o| o.approval == ApprovalState::Approved));

        // A second approval attempt is rejected: the plan is frozen.
        let err = approve_plan(&mut db, Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[test]
    fn planning_requires_the_analyzed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let err = create_plan(&mut db, Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
        let err = approve_plan(&mut db, Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[test]
    fn a_file_that_failed_hashing_is_blocked_not_copied() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("ok.txt"), b"fine").unwrap();
        std::fs::write(origin.join("volatile.txt"), b"will change").unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Con bloqueados",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin.clone())];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        // Change the file between scan and hash → SOURCE_CHANGED.
        std::fs::write(origin.join("volatile.txt"), b"changed after the scan").unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        let mut db = db;
        let outcome = analyzed_and_planned(&mut db);
        assert_eq!(outcome.copies, 1);
        assert_eq!(outcome.blocked, 1);

        // Coverage still holds: the blocked occurrence is in the plan.
        let report = validate_plan(&db).unwrap();
        assert!(report.ok, "{:?}", report.problems);
    }

    #[test]
    fn suffixed_destination_is_deterministic_and_keeps_the_extension() {
        assert_eq!(
            suffixed_destination("dir/doc.pdf", "abcdef1234567890"),
            "dir/doc~df-abcdef12.pdf"
        );
        assert_eq!(
            suffixed_destination("dir/no-extension", "abcdef1234567890"),
            "dir/no-extension~df-abcdef12"
        );
    }

    #[test]
    fn validation_rejects_escaping_destinations() {
        let plan = Plan::new(df_domain::ProjectId::new(), SnapshotId::new(), 1);
        let mut op = PlanOperation {
            id: df_domain::OperationId::new(),
            plan_id: plan.id,
            sequence: 1,
            operation_type: OperationType::CopyActive,
            source_occurrence: None,
            content_id: None,
            destination_relative_path: Some("..\\fuera.txt".to_string()),
            confidence: 1.0,
            risk: RiskLevel::Low,
            approval: ApprovalState::Pending,
            execution_state: ExecutionState::Pending,
            idempotency_key: "0".repeat(64),
            reason: "test".to_string(),
        };
        assert!(validate_operations(std::slice::from_ref(&op)).is_err());
        op.destination_relative_path = Some(PathBuf::from("C:\\absoluta").display().to_string());
        assert!(validate_operations(std::slice::from_ref(&op)).is_err());
    }
}
