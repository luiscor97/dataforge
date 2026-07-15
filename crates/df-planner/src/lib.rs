//! Analysis and planning (RFC-0001 §12.4–§12.8, §15, §26).
//!
//! Milestone 0.1 policy is `REPORT_ONLY` (§15.4): the plan mirrors the
//! source structure into the output root and copies everything that was
//! hashed, including duplicates. Duplicate sets are materialised as
//! evidence; no consolidation happens until profiles and contexts exist
//! (Milestone 0.2). Every occurrence is covered by exactly one operation
//! (§26.2) and approval freezes the plan under a canonical SHA-256 (§26.4).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use df_db::{plans, repository, Db};
use df_domain::{
    Actor, ApprovalState, DuplicateDisposition, DuplicateKind, DuplicatePolicy, ExecutionState,
    ManifestEntry, OperationType, Plan, PlanOperation, PlanStatus, ProjectState, RiskLevel,
    SourceRootId,
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
    /// Duplicate occurrences not copied because the set's representative
    /// already carries the content (§15.4). Always 0 under REPORT_ONLY.
    pub skipped_represented: u64,
    /// Duplicate occurrences copied *because* they sit in a protected
    /// boundary that the policy must not dissolve (rule 9).
    pub preserved_across_context: u64,
    /// The duplicate policy this plan was generated under.
    pub duplicate_policy: String,
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
///
/// `policy` governs what happens to exact duplicates (§15.4). The default,
/// [`DuplicatePolicy::ReportOnly`], copies every occurrence; consolidation is
/// always an explicit choice, and never dissolves a protected boundary.
pub fn create_plan(db: &mut Db, actor: Actor, policy: DuplicatePolicy) -> DfResult<PlanOutcome> {
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
    let operations = build_operations(db, &plan, policy)?;
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
        copies: count(OperationType::CopyActive)
            + count(OperationType::CopyWithSuffix)
            + count(OperationType::PreserveAcrossContext),
        directories: count(OperationType::CreateDirectory),
        no_action: count(OperationType::NoAction),
        blocked: count(OperationType::Blocked),
        skipped_represented: count(OperationType::SkipRepresented),
        preserved_across_context: count(OperationType::PreserveAcrossContext),
        duplicate_policy: policy.as_str().to_string(),
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

    // Freeze the whole execution contract (ADR-0018): snapshot the live
    // inventory into manifest entries, stamp each source root's physical
    // identity, and hash *that*. After this point the live tables are evidence,
    // not the contract.
    let mut entries = plans::build_manifest_entries(db, plan.id)?;
    for entry in &mut entries {
        entry.source_root_identity = entry
            .source_root_path_snapshot
            .as_ref()
            .and_then(|path| df_fs_safety::identity_of(Path::new(path)).ok().flatten())
            .map(|id| format!("{}:{}", id.volume_serial, id.file_index));
    }
    let sha256 = manifest_sha256(&entries);
    plans::approve_plan(db, &plan, &entries, &sha256, actor)?;
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
/// Canonical JSON of the execution manifest (RFC-0001 §26.4, ADR-0018).
///
/// This is what approval signs. It replaced an earlier version that covered
/// only identifiers (occurrence id, content id, destination): those bound the
/// paperwork while the executor resolved the actual bytes to read and the
/// hashes to expect from live tables, so a post-approval edit to
/// `content_objects.sha256` changed the work without moving this hash. Now
/// every field that decides what is read, expected, written or done is inside.
pub fn serialize_manifest(entries: &[ManifestEntry]) -> String {
    let items: Vec<serde_json::Value> = entries.iter().map(|e| e.canonical_value()).collect();
    df_ledger::canonical_json(&serde_json::Value::Array(items))
}

/// SHA-256 of [`serialize_manifest`] — the approval hash.
pub fn manifest_sha256(entries: &[ManifestEntry]) -> String {
    hex::encode(sha2::Sha256::digest(serialize_manifest(entries).as_bytes()))
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

/// Classify a duplicate set from the contexts of its members (§15.3).
///
/// Conservative by construction: `WithinSameContext` is only claimed when the
/// copies provably share a folder, because without the entity graph (§18.2)
/// that is the only "same context" we can demonstrate. Anything else that is
/// not clearly generic-to-canonical stays `UnknownContext`, which no policy
/// consolidates.
fn classify_duplicate_set(members: &[&df_db::dedup::DuplicateMember]) -> DuplicateKind {
    if members.len() < 2 {
        return DuplicateKind::UnknownContext;
    }
    // Any protected copy in a multi-copy set means consolidating would cross
    // a boundary (rule 9).
    if members
        .iter()
        .any(|m| m.context == df_domain::ContextKind::Protected)
    {
        return DuplicateKind::AcrossProtectedContexts;
    }

    // Provably the same context: all copies live in the same folder.
    let first_parent = &members[0].parent_relative_path;
    if members
        .iter()
        .all(|m| &m.parent_relative_path == first_parent)
    {
        return DuplicateKind::WithinSameContext;
    }

    let generic: Vec<_> = members
        .iter()
        .filter(|m| m.context == df_domain::ContextKind::Generic)
        .collect();
    let has_canonical = members.len() > generic.len();
    if !generic.is_empty() && has_canonical {
        // A backup replica is a generic copy whose marker says "backup".
        let all_backup = generic.iter().all(|m| {
            m.context_marker
                .as_deref()
                .is_some_and(|marker| marker.contains("backup"))
        });
        return if all_backup {
            DuplicateKind::BackupReplica
        } else {
            DuplicateKind::GenericToCanonical
        };
    }

    // Different folders, no generic/canonical split we can justify.
    DuplicateKind::UnknownContext
}

fn build_operations(db: &Db, plan: &Plan, policy: DuplicatePolicy) -> DfResult<Vec<PlanOperation>> {
    let roots = repository::load_source_roots(db, plan.project_id)?;
    let root_dirs = root_destination_dirs(&roots);
    let folders = df_db::inventory::list_folders(db, plan.snapshot_id)?;
    let occurrences = plans::planning_occurrences(db, plan.snapshot_id)?;

    // Duplicate membership + the kind of each set, so the policy can be
    // applied per occurrence. Occurrences absent from this map are not
    // duplicated and are always copied.
    let members = df_db::dedup::duplicate_members(db, plan.snapshot_id)?;
    let mut sets: HashMap<&str, Vec<&df_db::dedup::DuplicateMember>> = HashMap::new();
    for member in &members {
        sets.entry(&member.duplicate_set_id)
            .or_default()
            .push(member);
    }
    let kinds: HashMap<&str, DuplicateKind> = sets
        .iter()
        .map(|(set_id, members)| (*set_id, classify_duplicate_set(members)))
        .collect();
    let by_occurrence: HashMap<&str, &df_db::dedup::DuplicateMember> = members
        .iter()
        .map(|m| (m.occurrence_id.as_str(), m))
        .collect();

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

                    // Duplicate policy (§15.3/§15.4). Non-duplicated files are
                    // absent from the map and fall through to a plain copy.
                    let duplicate = by_occurrence.get(occurrence_key.as_str()).map(|member| {
                        let kind = kinds
                            .get(member.duplicate_set_id.as_str())
                            .copied()
                            .unwrap_or(DuplicateKind::UnknownContext);
                        let placement = df_domain::Placement {
                            is_representative: member.is_representative,
                            in_protected_context: member.context
                                == df_domain::ContextKind::Protected,
                            in_generic_context: member.context == df_domain::ContextKind::Generic,
                        };
                        (
                            kind,
                            *member,
                            df_domain::decide_duplicate(policy, kind, placement),
                        )
                    });

                    match duplicate {
                        Some((kind, _, DuplicateDisposition::SkipRepresented)) => (
                            OperationType::SkipRepresented,
                            None,
                            RiskLevel::Medium,
                            format!(
                                "exact duplicate ({}) already represented by the set's canonical \
                                 copy; not copied under policy {} — the source keeps it",
                                kind.as_str(),
                                policy.as_str()
                            ),
                        ),
                        Some((kind, member, DuplicateDisposition::PreserveAcrossContext)) => {
                            let marker = member
                                .context_marker
                                .as_deref()
                                .unwrap_or("protected boundary");
                            if taken_destinations.insert(planned.to_lowercase()) {
                                (
                                    OperationType::PreserveAcrossContext,
                                    Some(planned),
                                    RiskLevel::Low,
                                    format!(
                                        "exact duplicate ({}) preserved: it lives in a protected \
                                         context (`{marker}`) that policy {} must not dissolve \
                                         (RFC-0001 rule 9, §15.2)",
                                        kind.as_str(),
                                        policy.as_str()
                                    ),
                                )
                            } else {
                                let suffixed = suffixed_destination(&planned, sha256);
                                taken_destinations.insert(suffixed.to_lowercase());
                                (
                                    OperationType::CopyWithSuffix,
                                    Some(suffixed),
                                    RiskLevel::Medium,
                                    format!(
                                        "protected duplicate whose destination `{planned}` \
                                         collides; deterministic suffix applied (§27.3)"
                                    ),
                                )
                            }
                        }
                        // Copy: representative, preserved policy, or not a duplicate.
                        _ if taken_destinations.insert(planned.to_lowercase()) => (
                            OperationType::CopyActive,
                            Some(planned),
                            RiskLevel::Low,
                            match &duplicate {
                                Some((kind, _, _)) => format!(
                                    "verified copy of the canonical occurrence of an exact \
                                     duplicate set ({}) under policy {}",
                                    kind.as_str(),
                                    policy.as_str()
                                ),
                                None => format!(
                                    "verified copy preserving source structure (policy {})",
                                    policy.as_str()
                                ),
                            },
                        ),
                        _ => {
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
        create_plan(db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap()
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

        // The stored manifest re-serializes to the same hash: approval is
        // reproducible from what was frozen (ADR-0018).
        let manifest = plans::manifest(&db, plan.id).unwrap();
        assert!(!manifest.is_empty(), "approval must freeze a manifest");
        assert_eq!(manifest_sha256(&manifest), approved.serialized_sha256);
        let ops = plans::list_operations(&db, plan.id).unwrap();
        assert_eq!(manifest.len(), ops.len(), "every operation is frozen");
        assert!(ops.iter().all(|o| o.approval == ApprovalState::Approved));

        // A second approval attempt is rejected: the plan is frozen.
        let err = approve_plan(&mut db, Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[test]
    fn planning_requires_the_analyzed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let err = create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap_err();
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

    /// Two identical files in the SAME folder: provably one context, so
    /// CONSOLIDATE_WITHIN_CONTEXT may drop the non-representative copy.
    fn project_with_two_copies_in_one_folder(tmp: &Path) -> Db {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("informe.txt"), b"same payload").unwrap();
        std::fs::write(origin.join("informe copia.txt"), b"same payload").unwrap();

        let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Duplicados en una carpeta",
            ProfileRef::default(),
            tmp.join("salida"),
            tmp.join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        analyze_project(&mut db, Actor::Test).unwrap();
        db
    }

    #[test]
    fn report_only_copies_every_duplicate_and_skips_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = project_with_two_copies_in_one_folder(tmp.path());
        let outcome = create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
        assert_eq!(outcome.copies, 2, "both copies must be planned");
        assert_eq!(outcome.skipped_represented, 0);
        assert_eq!(outcome.duplicate_policy, "REPORT_ONLY");
    }

    #[test]
    fn consolidate_within_context_skips_the_non_representative_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = project_with_two_copies_in_one_folder(tmp.path());
        let outcome = create_plan(
            &mut db,
            Actor::Test,
            DuplicatePolicy::ConsolidateWithinContext,
        )
        .unwrap();

        // One copy survives (the representative); the other is represented.
        assert_eq!(outcome.copies, 1);
        assert_eq!(outcome.skipped_represented, 1);

        // Coverage still holds: the skipped occurrence is IN the plan (§26.2),
        // it simply carries a non-executable operation.
        let report = validate_plan(&db).unwrap();
        assert!(report.ok, "{:?}", report.problems);

        // And the decision is explained, never silent (§5.3).
        let project = repository::load_project(&db).unwrap();
        let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
        let skipped = plans::list_operations(&db, plan.id)
            .unwrap()
            .into_iter()
            .find(|o| o.operation_type == OperationType::SkipRepresented)
            .expect("a SKIP_REPRESENTED operation");
        assert!(skipped.destination_relative_path.is_none());
        assert!(skipped.reason.contains("WITHIN_SAME_CONTEXT"));
        assert!(skipped.reason.contains("CONSOLIDATE_WITHIN_CONTEXT"));
    }

    /// §15.2: copies in different folders cannot be proven to share a context
    /// without the entity graph, so no policy consolidates them.
    #[test]
    fn duplicates_across_unproven_contexts_are_never_consolidated() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyze_project(&mut db, Actor::Test).unwrap();
        let outcome = create_plan(&mut db, Actor::Test, DuplicatePolicy::ConsolidateAll).unwrap();
        // a.txt and sub/b.txt are identical but live in different folders.
        assert_eq!(outcome.skipped_represented, 0);
        assert_eq!(outcome.copies, 3);
    }

    /// RFC-0001 §19.4: "No eliminar una rama completa hasta identificar
    /// contenido único". This is the acceptance criterion "preservar contenido
    /// único" and it must hold under the most aggressive policy available.
    ///
    /// Scenario: a grafted copy of a working folder that ALSO carries one file
    /// the original never had. Consolidating the branch must never take that
    /// file with it.
    #[test]
    fn unique_content_in_a_cloned_branch_is_never_consolidated_away() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(origin.join("casos")).unwrap();
        // "casos - copia" is a generic container (copy-suffix marker).
        std::fs::create_dir_all(origin.join("casos - copia")).unwrap();

        std::fs::write(origin.join("casos").join("escrito.txt"), b"shared payload").unwrap();
        std::fs::write(
            origin.join("casos - copia").join("escrito.txt"),
            b"shared payload",
        )
        .unwrap();
        // The file that only exists in the copied branch.
        std::fs::write(
            origin.join("casos - copia").join("solo-aqui.txt"),
            b"this exists nowhere else",
        )
        .unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Rama injertada",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        analyze_project(&mut db, Actor::Test).unwrap();

        let outcome = create_plan(
            &mut db,
            Actor::Test,
            DuplicatePolicy::ConsolidateGenericCopies,
        )
        .unwrap();

        // The duplicated file in the generic copy is consolidated...
        assert_eq!(outcome.skipped_represented, 1);

        // ...but the unique file is planned for copy, no matter the policy.
        let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
        let operations = plans::list_operations(&db, plan.id).unwrap();
        let unique = operations
            .iter()
            .find(|o| {
                o.destination_relative_path
                    .as_deref()
                    .is_some_and(|d| d.ends_with("solo-aqui.txt"))
            })
            .expect("the unique file must have a copy operation with a destination");
        assert_eq!(unique.operation_type, OperationType::CopyActive);

        // It was never even a candidate for consolidation: it is not a duplicate.
        assert!(
            !operations
                .iter()
                .any(|o| o.operation_type == OperationType::SkipRepresented
                    && o.source_occurrence == unique.source_occurrence),
            "unique content must never be skipped"
        );
        // Coverage still holds.
        assert!(validate_plan(&db).unwrap().ok);
    }
}
