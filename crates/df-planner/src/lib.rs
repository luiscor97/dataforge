//! Analysis and planning (RFC-0001 §12.4–§12.8, §15, §26).
//!
//! Milestone 0.1 policy is `REPORT_ONLY` (§15.4): the plan mirrors the
//! source structure into the output root and copies everything that was
//! hashed, including duplicates. Duplicate sets are materialised as
//! evidence; no consolidation happens until profiles and contexts exist
//! (Milestone 0.2). Every occurrence is covered by exactly one operation
//! (§26.2) and approval freezes the plan under a canonical SHA-256 (§26.4).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path};

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
    /// Folder pairs that share content but where **both** hold something the
    /// other does not (§19.3). Neither may be dropped for the other (§19.4).
    pub partial_tree_clones: u64,
    /// Folder pairs where one subtree's content is wholly inside the other's.
    pub embedded_trees: u64,
    /// Folders tagged as low-value generic containers (RFC-0001 §18.3).
    pub generic_folders: u64,
    /// Profile-defined protected boundaries that no duplicate policy crosses.
    pub protected_boundaries: u64,
    /// Duplicate sets that got a logical representative (RFC-0001 §15.5).
    pub duplicate_representatives: u64,
    /// Occurrences classified by a versioned declarative rule (§25.1).
    pub rule_matches: u64,
    /// Persisted structural anomalies with self-contained evidence (§12.6).
    pub anomalies: u64,
    pub high_anomalies: u64,
    /// Ambiguous decisions awaiting or carrying human review (§12.7).
    pub review_items: u64,
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
    pub review_copies: u64,
    pub separated_copies: u64,
    pub temporary_copies: u64,
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
    if !matches!(
        project.state,
        ProjectState::Hashed | ProjectState::Analyzing | ProjectState::Analyzed
    ) {
        return Err(DfError::Validation(format!(
            "cannot analyze a project in state {} (expected HASHED, ANALYZING, or a legacy ANALYZED snapshot)",
            project.state
        )));
    }
    // Validate before entering ANALYZING: a typo must not silently select the
    // generic profile or leave the project stranded in a transitional state.
    df_domain::Profile::load(project.profile.as_str())?;
    let snapshot = df_db::inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let legacy_upgrade = project.state == ProjectState::Analyzed;
    if legacy_upgrade && df_db::analysis::is_analysis_complete(db, snapshot.id)? {
        return Err(DfError::Validation(
            "the latest snapshot already has a completed structural analysis".to_string(),
        ));
    }

    // Every analysis repository below is idempotent for a snapshot. If the
    // process died after HASHED -> ANALYZING, replay the stages but do not
    // append a second transition event.
    if project.state == ProjectState::Hashed {
        repository::update_project_state(db, ProjectState::Analyzing, actor)?;
    }
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
    // Pairwise relations need the signatures, so they run after them.
    let relations = df_db::structure::compute_tree_relations(
        db,
        project.id,
        snapshot.id,
        &df_db::structure::TreeRelationOptions::default(),
        actor,
    )?;
    // Representatives need the context penalties, so this runs last.
    let duplicate_representatives =
        df_db::dedup::score_duplicate_representatives(db, project.id, snapshot.id, actor)?;
    let rules = df_db::analysis::evaluate_rules(
        db,
        project.id,
        snapshot.id,
        project.profile.as_str(),
        actor,
    )?;
    let anomalies = df_db::analysis::detect_anomalies(db, project.id, snapshot.id, actor)?;
    let completion_summary = serde_json::json!({
        "duplicate_sets": duplicate_sets,
        "folder_signatures": structure.folders_signed,
        "tree_clone_sets": structure.tree_clone_sets,
        "partial_tree_clones": relations.partial_clones,
        "embedded_trees": relations.embedded,
        "generic_folders": context.generic_folders,
        "protected_boundaries": context.protected_boundaries,
        "duplicate_representatives": duplicate_representatives,
        "rule_matches": rules.matches,
        "anomalies": anomalies.anomalies,
        "high_anomalies": anomalies.high,
        "review_items": anomalies.review_items,
    });
    // This append-only marker is the report visibility boundary: a crash in
    // any preceding stage cannot masquerade as a valid empty diagnostic.
    df_db::analysis::complete_analysis(
        db,
        project.id,
        snapshot.id,
        project.profile.as_str(),
        &completion_summary,
        actor,
    )?;
    let project = if legacy_upgrade {
        // Databases produced by an earlier M0.2 increment may already be in
        // ANALYZED without the 0010 completion marker. Recompute the new
        // append-only evidence in place; moving the lifecycle backwards would
        // violate the RFC state machine.
        repository::load_project(db)?
    } else {
        repository::update_project_state(db, ProjectState::Analyzed, actor)?
    };

    Ok(AnalyzeOutcome {
        snapshot_id: snapshot.id.to_string(),
        duplicate_sets,
        folder_signatures: structure.folders_signed,
        tree_clone_sets: structure.tree_clone_sets,
        partial_tree_clones: relations.partial_clones,
        embedded_trees: relations.embedded,
        generic_folders: context.generic_folders,
        protected_boundaries: context.protected_boundaries,
        duplicate_representatives,
        rule_matches: rules.matches,
        anomalies: anomalies.anomalies,
        high_anomalies: anomalies.high,
        review_items: anomalies.review_items,
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
    if !matches!(
        project.state,
        ProjectState::Analyzed | ProjectState::Planning
    ) {
        return Err(DfError::Validation(format!(
            "cannot plan a project in state {} (expected ANALYZED or PLANNING)",
            project.state
        )));
    }
    let snapshot = df_db::inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    df_db::analysis::require_current_analysis_completion(
        db,
        project.id,
        snapshot.id,
        project.profile.as_str(),
    )?;

    let resuming = project.state == ProjectState::Planning;
    if !resuming {
        repository::update_project_state(db, ProjectState::Planning, actor)?;
    }

    // A crash may happen after the READY plan and PLAN_CREATED event commit,
    // but before PLANNING -> PLAN_READY. Reuse that exact version instead of
    // superseding it with a second one. Rebuilding in memory lets us prove the
    // retry supplied a policy with the same effective operations.
    if resuming {
        if let Some(plan) = plans::current_plan(db, project.id)? {
            if plan.status != PlanStatus::Ready {
                return Err(DfError::Conflict(format!(
                    "project is PLANNING but its current plan is {}",
                    plan.status.as_str()
                )));
            }
            if plan.snapshot_id != snapshot.id {
                return Err(DfError::Conflict(format!(
                    "project is PLANNING for snapshot {}, but READY plan v{} belongs to {}",
                    snapshot.id, plan.version, plan.snapshot_id
                )));
            }

            let operations = plans::list_operations(db, plan.id)?;
            validate_operations(&operations)?;
            let expected = build_operations(db, &plan, policy)?;
            if !same_effective_operations(&operations, &expected) {
                return Err(DfError::Conflict(format!(
                    "persisted READY plan v{} does not match duplicate policy {}; retry with the original policy",
                    plan.version,
                    policy.as_str()
                )));
            }

            let project = repository::update_project_state(db, ProjectState::PlanReady, actor)?;
            return Ok(plan_outcome(&plan, &operations, policy, project.state));
        }
    }

    let version = plans::next_plan_version(db, project.id)?;
    let mut plan = Plan::new(project.id, snapshot.id, version);
    let operations = build_operations(db, &plan, policy)?;
    validate_operations(&operations)?;
    plan.status = PlanStatus::Ready;
    plans::insert_plan(db, &plan, &operations, actor)?;

    let project = repository::update_project_state(db, ProjectState::PlanReady, actor)?;
    Ok(plan_outcome(&plan, &operations, policy, project.state))
}

/// Compare persisted operations with a deterministic rebuild while ignoring
/// only the random operation UUID allocated during each in-memory build.
fn same_effective_operations(stored: &[PlanOperation], rebuilt: &[PlanOperation]) -> bool {
    stored.len() == rebuilt.len()
        && stored.iter().zip(rebuilt).all(|(stored, rebuilt)| {
            let mut rebuilt = rebuilt.clone();
            rebuilt.id = stored.id;
            stored == &rebuilt
        })
}

fn plan_outcome(
    plan: &Plan,
    operations: &[PlanOperation],
    policy: DuplicatePolicy,
    state: ProjectState,
) -> PlanOutcome {
    let count =
        |t: OperationType| operations.iter().filter(|o| o.operation_type == t).count() as u64;
    PlanOutcome {
        plan_id: plan.id.to_string(),
        snapshot_id: plan.snapshot_id.to_string(),
        version: plan.version,
        operations: operations.len() as u64,
        copies: count(OperationType::CopyActive)
            + count(OperationType::CopyReview)
            + count(OperationType::CopySeparated)
            + count(OperationType::CopyTemporary)
            + count(OperationType::CopyWithSuffix)
            + count(OperationType::PreserveAcrossContext),
        review_copies: count(OperationType::CopyReview),
        separated_copies: count(OperationType::CopySeparated),
        temporary_copies: count(OperationType::CopyTemporary),
        directories: count(OperationType::CreateDirectory),
        no_action: count(OperationType::NoAction),
        blocked: count(OperationType::Blocked),
        skipped_represented: count(OperationType::SkipRepresented),
        preserved_across_context: count(OperationType::PreserveAcrossContext),
        duplicate_policy: policy.as_str().to_string(),
        state: state.as_str().to_string(),
    }
}

/// Re-run the §26.5 invariants against the stored current plan.
pub fn validate_plan(db: &Db) -> DfResult<PlanValidationReport> {
    let project = repository::load_project(db)?;
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;
    df_db::analysis::require_current_analysis_completion(
        db,
        project.id,
        plan.snapshot_id,
        project.profile.as_str(),
    )?;
    let operations = plans::list_operations(db, plan.id)?;
    let problems = match validate_operations(&operations) {
        Ok(()) => Vec::new(),
        Err(DfError::Validation(message)) => vec![message],
        Err(other) => return Err(other),
    };
    let occurrences = plans::planning_occurrences(db, plan.snapshot_id)?;
    let mut problems = problems;
    problems.extend(occurrence_coverage_problems(&occurrences, &operations));
    Ok(PlanValidationReport {
        plan_id: plan.id.to_string(),
        version: plan.version,
        status: plan.status.as_str().to_string(),
        operations: operations.len() as u64,
        ok: problems.is_empty(),
        problems,
    })
}

/// Exact §26.2 coverage: every snapshot occurrence appears once, no operation
/// may smuggle in an occurrence from another snapshot, and duplicate coverage
/// is rejected even if a malformed legacy database lacks the unique index.
fn occurrence_coverage_problems(
    occurrences: &[plans::PlanningOccurrence],
    operations: &[PlanOperation],
) -> Vec<String> {
    let expected: BTreeMap<String, &str> = occurrences
        .iter()
        .map(|occurrence| {
            (
                occurrence.occurrence_id.to_string(),
                occurrence.relative_path.as_str(),
            )
        })
        .collect();
    let mut covered: BTreeMap<String, u64> = BTreeMap::new();
    for operation in operations {
        if let Some(occurrence_id) = operation.source_occurrence {
            *covered.entry(occurrence_id.to_string()).or_default() += 1;
        }
    }

    let mut problems = Vec::new();
    for (occurrence_id, relative_path) in &expected {
        match covered.get(occurrence_id).copied().unwrap_or(0) {
            0 => problems.push(format!(
                "occurrence `{relative_path}` is not covered by the plan"
            )),
            1 => {}
            count => problems.push(format!(
                "occurrence `{relative_path}` is covered {count} times; expected exactly once"
            )),
        }
    }
    for occurrence_id in covered.keys() {
        if !expected.contains_key(occurrence_id) {
            problems.push(format!(
                "operation references occurrence `{occurrence_id}` from another snapshot"
            ));
        }
    }
    problems
}

/// Approve the current READY plan (§26.4): validate, canonically serialize,
/// hash, freeze; moves the project `PLAN_READY → PLAN_REVIEW → PLAN_APPROVED`.
pub fn approve_plan(db: &mut Db, actor: Actor) -> DfResult<ApproveOutcome> {
    let project = repository::load_project(db)?;
    if !matches!(
        project.state,
        ProjectState::PlanReady | ProjectState::PlanReview
    ) {
        return Err(DfError::Validation(format!(
            "cannot approve a plan in project state {} (expected PLAN_READY or PLAN_REVIEW)",
            project.state
        )));
    }
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;
    df_db::analysis::require_current_analysis_completion(
        db,
        project.id,
        plan.snapshot_id,
        project.profile.as_str(),
    )?;
    let operations = plans::list_operations(db, plan.id)?;
    validate_operations(&operations)?;
    let occurrences = plans::planning_occurrences(db, plan.snapshot_id)?;
    let coverage_problems = occurrence_coverage_problems(&occurrences, &operations);
    if !coverage_problems.is_empty() {
        return Err(DfError::Validation(format!(
            "plan does not cover its snapshot exactly once: {}",
            coverage_problems.join("; ")
        )));
    }

    if project.state == ProjectState::PlanReady {
        if plan.status != PlanStatus::Ready {
            return Err(DfError::Conflict(format!(
                "project is PLAN_READY but its current plan is {}",
                plan.status.as_str()
            )));
        }
        repository::update_project_state(db, ProjectState::PlanReview, actor)?;
    }

    let sha256 = match plan.status {
        PlanStatus::Ready => {
            // Freeze the whole execution contract (ADR-0018): snapshot the
            // live inventory, stamp each source root's physical identity and
            // hash that. A retry from PLAN_REVIEW before this transaction
            // committed safely rebuilds the same contract.
            let entries = approval_manifest(db, plan.id)?;
            let sha256 = manifest_sha256(&entries);
            plans::approve_plan(db, &plan, &entries, &sha256, actor)?;
            sha256
        }
        PlanStatus::Approved if project.state == ProjectState::PlanReview => {
            // The approval transaction committed but the following project
            // transition did not. Trust only the frozen evidence after
            // checking its shape and hash; never insert a second manifest or
            // append another PLAN_APPROVED event.
            if !operations
                .iter()
                .all(|operation| operation.approval == ApprovalState::Approved)
            {
                return Err(DfError::Conflict(
                    "APPROVED plan contains operations that are not approved".to_string(),
                ));
            }
            let entries = plans::manifest(db, plan.id)?;
            let manifest_matches_operations = entries.len() == operations.len()
                && entries.iter().zip(&operations).all(|(entry, operation)| {
                    entry.plan_id == operation.plan_id
                        && entry.operation_id == operation.id
                        && entry.sequence == operation.sequence
                });
            if !manifest_matches_operations {
                return Err(DfError::Conflict(
                    "APPROVED plan manifest does not match its operations".to_string(),
                ));
            }
            let stored = plan.serialized_sha256.as_deref().ok_or_else(|| {
                DfError::Conflict("APPROVED plan has no serialized SHA-256".to_string())
            })?;
            let calculated = manifest_sha256(&entries);
            if calculated != stored {
                return Err(DfError::Conflict(format!(
                    "APPROVED plan manifest hash mismatch: stored {stored}, calculated {calculated}"
                )));
            }
            calculated
        }
        status => {
            return Err(DfError::Conflict(format!(
                "cannot finish approval from PLAN_REVIEW with a {} plan",
                status.as_str()
            )))
        }
    };
    let project = repository::update_project_state(db, ProjectState::PlanApproved, actor)?;

    Ok(ApproveOutcome {
        plan_id: plan.id.to_string(),
        version: plan.version,
        serialized_sha256: sha256,
        operations_approved: operations.len() as u64,
        state: project.state.as_str().to_string(),
    })
}

fn approval_manifest(db: &Db, plan_id: df_domain::PlanId) -> DfResult<Vec<ManifestEntry>> {
    let mut entries = plans::build_manifest_entries(db, plan_id)?;
    for entry in &mut entries {
        entry.source_root_identity = match entry.source_root_path_snapshot.as_deref() {
            Some(path) => df_fs_safety::identity_of(Path::new(path))?
                .map(|id| format!("{}:{}", id.volume_serial, id.file_index)),
            None => None,
        };
    }
    Ok(entries)
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
const REVIEW_BUCKET: &str = "90_DataForge_Review";
const SEPARATED_BUCKET: &str = "95_DataForge_Separated";
const TEMPORARY_BUCKET: &str = "98_DataForge_Temporary";
const PORTABLE_COMPONENT_PREFIX: &str = "~df-portable-";

/// Turn a source component into a name which is safe to materialise on every
/// supported Windows filesystem. Safe names stay readable; unsafe or reserved
/// names receive a deterministic, domain-separated encoding derived from the
/// exact UTF-16 identity captured by the scanner.
fn portable_component(
    display: &str,
    raw_blob: Option<&[u8]>,
    component_index: usize,
) -> (String, bool) {
    let safe = !display
        .to_ascii_lowercase()
        .starts_with(PORTABLE_COMPONENT_PREFIX)
        && df_fs_safety::SafeRelativePath::parse(Path::new(display))
            .is_ok_and(|parsed| parsed.components().len() == 1);
    if safe {
        return (display.to_string(), false);
    }

    let mut digest = sha2::Sha256::new();
    digest.update(b"dataforge.portable-destination-component.v1\0");
    digest.update((component_index as u64).to_le_bytes());
    match raw_blob {
        Some(raw) => {
            digest.update(b"raw\0");
            digest.update(raw);
        }
        None => {
            digest.update(b"display\0");
            digest.update(display.as_bytes());
        }
    }
    let tag = hex::encode(digest.finalize());
    (format!("{PORTABLE_COMPONENT_PREFIX}{}", &tag[..32]), true)
}

fn raw_component_blobs(raw: Option<&df_domain::RawPath>) -> Vec<Vec<u8>> {
    raw.into_iter()
        .flat_map(|raw| {
            std::path::PathBuf::from(raw.to_os_string())
                .components()
                .filter_map(|component| match component {
                    Component::Normal(value) => {
                        Some(df_domain::RawPath::from_os_str(value).to_blob())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn portable_relative_path(
    relative: &str,
    raw: Option<&df_domain::RawPath>,
) -> DfResult<(String, bool)> {
    if relative.is_empty() {
        return Ok((String::new(), false));
    }
    let raw_components = raw_component_blobs(raw);
    let mut encoded = false;
    let mut components = Vec::new();
    for (index, component) in Path::new(relative).components().enumerate() {
        let Component::Normal(value) = component else {
            return Err(DfError::Validation(format!(
                "inventory path `{relative}` is not a safe relative source path"
            )));
        };
        let display = value.to_string_lossy();
        let (portable, changed) = portable_component(
            &display,
            raw_components.get(index).map(Vec::as_slice),
            index,
        );
        encoded |= changed;
        components.push(portable);
    }
    if components.is_empty() {
        return Err(DfError::Validation(format!(
            "inventory path `{relative}` has no usable components"
        )));
    }
    Ok((
        components
            .iter()
            .collect::<std::path::PathBuf>()
            .display()
            .to_string(),
        encoded,
    ))
}

fn root_destination_dirs(roots: &[df_domain::SourceRoot]) -> HashMap<SourceRootId, (String, bool)> {
    // Bucket names are reserved even when a profile currently emits no item
    // into them, so a source root can never shadow an operational container.
    let mut used: HashSet<String> = [REVIEW_BUCKET, SEPARATED_BUCKET, TEMPORARY_BUCKET]
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let mut mapping = HashMap::new();
    for root in roots {
        let source_name = root
            .absolute_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "origen".to_string());
        let raw_name = root
            .absolute_path
            .file_name()
            .map(df_domain::RawPath::from_os_str);
        let (base, encoded) = portable_component(
            &source_name,
            raw_name.as_ref().map(|raw| raw.to_blob()).as_deref(),
            0,
        );
        let mut candidate = base.clone();
        let mut suffix = 1;
        while !used.insert(candidate.to_lowercase()) {
            suffix += 1;
            candidate = format!("{base}-{suffix}");
        }
        mapping.insert(root.id, (candidate, encoded));
    }
    mapping
}

fn operational_bucket(operation_type: OperationType) -> Option<&'static str> {
    match operation_type {
        OperationType::CopyReview => Some(REVIEW_BUCKET),
        OperationType::CopySeparated => Some(SEPARATED_BUCKET),
        OperationType::CopyTemporary => Some(TEMPORARY_BUCKET),
        _ => None,
    }
}

fn route_destination(operation_type: OperationType, active_destination: &str) -> String {
    operational_bucket(operation_type).map_or_else(
        || active_destination.to_string(),
        |bucket| {
            Path::new(bucket)
                .join(active_destination)
                .display()
                .to_string()
        },
    )
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
/// copies provably share a source root and folder, because without the entity
/// graph (§18.2) that is the only "same context" we can demonstrate. Anything
/// else that is not clearly generic-to-canonical stays `UnknownContext`, which
/// no policy consolidates.
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

    // Provably the same context: all copies live in the same folder of the
    // same source root. Equal relative paths from different roots are not the
    // same location.
    let first_root = &members[0].source_root_id;
    let first_parent = &members[0].parent_relative_path;
    if members
        .iter()
        .all(|m| &m.source_root_id == first_root && &m.parent_relative_path == first_parent)
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
    let guidance = df_db::analysis::occurrence_guidance(db, plan.snapshot_id)?;

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
    let destination_for = |root_id: SourceRootId,
                           relative: &str,
                           raw: Option<&df_domain::RawPath>|
     -> DfResult<Option<(String, bool)>> {
        let Some((dir, root_encoded)) = root_dirs.get(&root_id) else {
            return Ok(None);
        };
        let (portable_relative, relative_encoded) = portable_relative_path(relative, raw)?;
        Ok(Some((
            if portable_relative.is_empty() {
                dir.clone()
            } else {
                format!("{dir}{sep}{portable_relative}")
            },
            *root_encoded || relative_encoded,
        )))
    };

    let mut operations: Vec<PlanOperation> = Vec::new();
    let mut sequence: u64 = 0;
    let mut taken_destinations: HashSet<String> = HashSet::new();
    let mut directory_destinations: HashSet<String> = HashSet::new();
    let next = |seq: &mut u64| {
        *seq += 1;
        *seq
    };

    // Folders first, parents before children (list_folders orders by depth).
    for folder in &folders {
        let (operation_type, destination, risk, reason) = match folder.status {
            df_domain::ScanEntryStatus::Ok => {
                let (destination, portable_encoded) = destination_for(
                    folder.source_root_id,
                    &folder.relative_path,
                    folder.raw_relative_path.as_ref(),
                )?
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
                    if portable_encoded {
                        format!(
                            "mirror source directory structure with a deterministic portable name for `{}`",
                            folder.relative_path
                        )
                    } else {
                        "mirror source directory structure (REPORT_ONLY policy)".to_string()
                    },
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
            directory_destinations.insert(dest.to_lowercase());
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
        let recommendation = guidance.get(&occurrence.occurrence_id);
        let (operation_type, destination, risk, confidence, reason) = match occurrence.scan_status {
            df_domain::ScanEntryStatus::ReparseNotFollowed => (
                OperationType::NoAction,
                None,
                RiskLevel::Low,
                1.0,
                format!(
                    "reparse point `{}` recorded but not followed by policy (RFC-0001 §13.6)",
                    occurrence.relative_path
                ),
            ),
            df_domain::ScanEntryStatus::Error => (
                OperationType::Blocked,
                None,
                RiskLevel::High,
                0.0,
                format!(
                    "file `{}` could not be inventoried: {}",
                    occurrence.relative_path,
                    occurrence.hash_error.as_deref().unwrap_or("scan error")
                ),
            ),
            df_domain::ScanEntryStatus::Ok => match (&occurrence.content_id, &occurrence.sha256) {
                (Some(_), Some(sha256)) => {
                    let (planned, portable_encoded) = destination_for(
                        occurrence.source_root_id,
                        &occurrence.relative_path,
                        occurrence.raw_relative_path.as_ref(),
                    )?
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
                                    1.0,
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
                                    1.0,
                                    format!(
                                        "protected duplicate whose destination `{planned}` \
                                         collides; deterministic suffix applied (§27.3)"
                                    ),
                                )
                            }
                        }
                        // Rules and unresolved human review are conservative
                        // copy actions. They override duplicate consolidation
                        // (never protected-boundary preservation) so an
                        // ambiguous occurrence is not silently represented by
                        // another path before the user decides.
                        _ if recommendation.is_some() => {
                            let recommendation = recommendation.ok_or_else(|| {
                                DfError::Database(
                                    "planner recommendation guard became inconsistent".to_string(),
                                )
                            })?;
                            let routed = route_destination(recommendation.operation_type, &planned);
                            let (destination, collision) =
                                if taken_destinations.insert(routed.to_lowercase()) {
                                    (routed.clone(), false)
                                } else {
                                    let suffixed = suffixed_destination(&routed, sha256);
                                    taken_destinations.insert(suffixed.to_lowercase());
                                    (suffixed, true)
                                };
                            let risk = if collision && recommendation.risk == RiskLevel::Low {
                                RiskLevel::Medium
                            } else {
                                recommendation.risk
                            };
                            (
                                recommendation.operation_type,
                                Some(destination),
                                risk,
                                recommendation.confidence,
                                if collision {
                                    format!(
                                        "{}; destination `{routed}` collided and received the \
                                         deterministic §27.3 suffix",
                                        recommendation.reason
                                    )
                                } else if let Some(bucket) =
                                    operational_bucket(recommendation.operation_type)
                                {
                                    format!(
                                        "{}; routed to operational bucket `{bucket}`",
                                        recommendation.reason
                                    )
                                } else if portable_encoded {
                                    format!(
                                        "{}; an unsafe source component received a deterministic portable destination name",
                                        recommendation.reason
                                    )
                                } else {
                                    recommendation.reason.clone()
                                },
                            )
                        }
                        Some((kind, _, DuplicateDisposition::SkipRepresented)) => (
                            OperationType::SkipRepresented,
                            None,
                            RiskLevel::Medium,
                            1.0,
                            format!(
                                "exact duplicate ({}) already represented by the set's canonical \
                                 copy; not copied under policy {} — the source keeps it",
                                kind.as_str(),
                                policy.as_str()
                            ),
                        ),
                        // Copy: representative, preserved policy, or not a duplicate.
                        _ if taken_destinations.insert(planned.to_lowercase()) => (
                            OperationType::CopyActive,
                            Some(planned),
                            RiskLevel::Low,
                            1.0,
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
                                1.0,
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
                    0.0,
                    format!(
                        "file `{}` has no verified content identity ({}): {}",
                        occurrence.relative_path,
                        occurrence.hash_status.as_deref().unwrap_or("no hash job"),
                        occurrence.hash_error.as_deref().unwrap_or("not hashed")
                    ),
                ),
            },
        };
        if operation_type.is_executable() {
            if let Some(parent) = destination
                .as_deref()
                .and_then(|destination| Path::new(destination).parent())
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                let parent = parent.display().to_string();
                if directory_destinations.insert(parent.to_lowercase()) {
                    taken_destinations.insert(parent.to_lowercase());
                    operations.push(PlanOperation {
                        id: df_domain::OperationId::new(),
                        plan_id: plan.id,
                        sequence: next(&mut sequence),
                        operation_type: OperationType::CreateDirectory,
                        source_occurrence: None,
                        content_id: None,
                        confidence: 1.0,
                        risk: RiskLevel::Low,
                        approval: ApprovalState::Pending,
                        execution_state: initial_execution_state(OperationType::CreateDirectory),
                        idempotency_key: idempotency_key(
                            plan,
                            None,
                            OperationType::CreateDirectory,
                            Some(&parent),
                        ),
                        destination_relative_path: Some(parent),
                        reason: "create the parent for a routed operational bucket".to_string(),
                    });
                }
            }
        }
        operations.push(PlanOperation {
            id: df_domain::OperationId::new(),
            plan_id: plan.id,
            sequence: next(&mut sequence),
            operation_type,
            source_occurrence: Some(occurrence.occurrence_id),
            content_id: occurrence.content_id,
            confidence,
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
                if let Err(error) = df_fs_safety::SafeRelativePath::parse(path) {
                    return Err(DfError::Validation(format!(
                        "operation #{} has an unsafe output path `{destination}`: {error} \
                         (RFC-0001 §26.5)",
                        op.sequence,
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

    fn event_count(db: &Db, project_id: df_domain::ProjectId, event_type: &str) -> usize {
        repository::list_events(db, project_id)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == event_type)
            .count()
    }

    fn transition_count(db: &Db, project_id: df_domain::ProjectId, to: &str) -> usize {
        repository::list_events(db, project_id)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == repository::EVENT_STATE_CHANGED)
            .filter(|event| {
                serde_json::from_str::<serde_json::Value>(&event.payload_json)
                    .ok()
                    .and_then(|payload| {
                        payload
                            .get("to")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    })
                    .as_deref()
                    == Some(to)
            })
            .count()
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
    fn analyze_resumes_from_analyzing_without_repeating_the_initial_transition() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let project = repository::load_project(&db).unwrap();
        let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .unwrap();

        // Simulate a crash after the state change and one committed analysis
        // repository. Both mutations go through the public repositories.
        repository::update_project_state(&mut db, ProjectState::Analyzing, Actor::Test).unwrap();
        plans::materialize_duplicate_sets(&mut db, project.id, snapshot.id, Actor::Test).unwrap();

        let outcome = analyze_project(&mut db, Actor::Test).unwrap();
        assert_eq!(outcome.state, "ANALYZED");
        assert_eq!(outcome.duplicate_sets, 1);
        assert_eq!(
            transition_count(&db, project.id, "ANALYZING"),
            1,
            "resuming must not append a second HASHED -> ANALYZING transition"
        );
    }

    #[test]
    fn analyze_upgrades_a_legacy_analyzed_snapshot_without_moving_state_backwards() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let project = repository::load_project(&db).unwrap();
        let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .unwrap();

        // Simulate the lifecycle left by an earlier M0.2 increment, before
        // migration 0010 introduced the final append-only analysis marker.
        repository::update_project_state(&mut db, ProjectState::Analyzing, Actor::Test).unwrap();
        repository::update_project_state(&mut db, ProjectState::Analyzed, Actor::Test).unwrap();
        assert!(!df_db::analysis::is_analysis_complete(&db, snapshot.id).unwrap());

        let error = create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("has not completed")),
            "lifecycle state alone must not authorize planning: {error:?}"
        );
        assert_eq!(
            repository::load_project(&db).unwrap().state,
            ProjectState::Analyzed
        );

        let outcome = analyze_project(&mut db, Actor::Test).unwrap();
        assert_eq!(outcome.state, "ANALYZED");
        assert!(df_db::analysis::is_analysis_complete(&db, snapshot.id).unwrap());
        assert_eq!(
            transition_count(&db, project.id, "ANALYZED"),
            1,
            "legacy upgrade must not forge a second state transition"
        );
    }

    #[test]
    fn analyze_rejects_an_unknown_profile_before_changing_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        db.conn_for_tests()
            .execute("UPDATE projects SET profile = 'legla'", [])
            .unwrap();

        let error = analyze_project(&mut db, Actor::Test).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("legla")),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            repository::load_project(&db).unwrap().state,
            ProjectState::Hashed,
            "profile validation must happen before HASHED -> ANALYZING"
        );
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

        let project = repository::load_project(&db).unwrap();
        let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
        let operations = plans::list_operations(&db, plan.id).unwrap();
        let occurrences = plans::planning_occurrences(&db, plan.snapshot_id).unwrap();
        assert!(occurrence_coverage_problems(&occurrences, &operations).is_empty());

        let target = occurrences[0].occurrence_id;
        let omitted: Vec<_> = operations
            .iter()
            .filter(|operation| operation.source_occurrence != Some(target))
            .cloned()
            .collect();
        assert!(occurrence_coverage_problems(&occurrences, &omitted)
            .iter()
            .any(|problem| problem.contains("not covered")));

        let mut duplicated = operations.clone();
        duplicated.push(
            operations
                .iter()
                .find(|operation| operation.source_occurrence == Some(target))
                .unwrap()
                .clone(),
        );
        assert!(occurrence_coverage_problems(&occurrences, &duplicated)
            .iter()
            .any(|problem| problem.contains("covered 2 times")));

        let mut stale = operations.clone();
        stale
            .iter_mut()
            .find(|operation| operation.source_occurrence.is_some())
            .unwrap()
            .source_occurrence = Some(df_domain::OccurrenceId::new());
        assert!(occurrence_coverage_problems(&occurrences, &stale)
            .iter()
            .any(|problem| problem.contains("another snapshot")));
    }

    #[test]
    fn planning_resumes_before_a_plan_was_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyze_project(&mut db, Actor::Test).unwrap();
        let project = repository::load_project(&db).unwrap();

        repository::update_project_state(&mut db, ProjectState::Planning, Actor::Test).unwrap();
        let outcome = create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();

        assert_eq!(outcome.version, 1);
        assert_eq!(outcome.state, "PLAN_READY");
        assert_eq!(transition_count(&db, project.id, "PLANNING"), 1);
        assert_eq!(event_count(&db, project.id, plans::EVENT_PLAN_CREATED), 1);
    }

    #[test]
    fn planning_reuses_a_ready_plan_persisted_before_the_state_transition() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyze_project(&mut db, Actor::Test).unwrap();
        let project = repository::load_project(&db).unwrap();
        let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .unwrap();

        repository::update_project_state(&mut db, ProjectState::Planning, Actor::Test).unwrap();
        let mut persisted = Plan::new(project.id, snapshot.id, 1);
        let operations = build_operations(&db, &persisted, DuplicatePolicy::ReportOnly).unwrap();
        validate_operations(&operations).unwrap();
        persisted.status = PlanStatus::Ready;
        plans::insert_plan(&mut db, &persisted, &operations, Actor::Test).unwrap();
        let created_events = event_count(&db, project.id, plans::EVENT_PLAN_CREATED);

        let outcome = create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
        assert_eq!(outcome.plan_id, persisted.id.to_string());
        assert_eq!(outcome.version, 1);
        assert_eq!(outcome.operations, operations.len() as u64);
        assert_eq!(outcome.state, "PLAN_READY");
        assert_eq!(
            event_count(&db, project.id, plans::EVENT_PLAN_CREATED),
            created_events,
            "recovery must not create another version or PLAN_CREATED event"
        );
        let current = plans::current_plan(&db, project.id).unwrap().unwrap();
        assert_eq!(current.id, persisted.id);
        assert_eq!(plans::next_plan_version(&db, project.id).unwrap(), 2);
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
    fn approval_resumes_from_plan_review_before_persistence() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyzed_and_planned(&mut db);
        let project = repository::load_project(&db).unwrap();

        repository::update_project_state(&mut db, ProjectState::PlanReview, Actor::Test).unwrap();
        let approved = approve_plan(&mut db, Actor::Test).unwrap();

        assert_eq!(approved.state, "PLAN_APPROVED");
        assert_eq!(transition_count(&db, project.id, "PLAN_REVIEW"), 1);
        assert_eq!(event_count(&db, project.id, plans::EVENT_PLAN_APPROVED), 1);
    }

    #[test]
    fn approval_finishes_after_persistence_without_a_second_manifest_or_event() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        analyzed_and_planned(&mut db);
        let project = repository::load_project(&db).unwrap();
        repository::update_project_state(&mut db, ProjectState::PlanReview, Actor::Test).unwrap();

        let ready = plans::current_plan(&db, project.id).unwrap().unwrap();
        let entries = approval_manifest(&db, ready.id).unwrap();
        let sha256 = manifest_sha256(&entries);
        plans::approve_plan(&mut db, &ready, &entries, &sha256, Actor::Test).unwrap();
        let approved_events = event_count(&db, project.id, plans::EVENT_PLAN_APPROVED);
        let manifest_len = plans::manifest(&db, ready.id).unwrap().len();

        let outcome = approve_plan(&mut db, Actor::Test).unwrap();
        assert_eq!(outcome.plan_id, ready.id.to_string());
        assert_eq!(outcome.serialized_sha256, sha256);
        assert_eq!(outcome.state, "PLAN_APPROVED");
        assert_eq!(
            event_count(&db, project.id, plans::EVENT_PLAN_APPROVED),
            approved_events,
            "recovery must not append a second PLAN_APPROVED event"
        );
        assert_eq!(
            plans::manifest(&db, ready.id).unwrap().len(),
            manifest_len,
            "recovery must reuse the frozen manifest"
        );
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
    fn unsafe_source_names_receive_stable_portable_destinations() {
        let unsafe_folder = PathBuf::from("CON");
        let raw_folder = df_domain::RawPath::from_os_str(unsafe_folder.as_os_str());
        let (encoded_folder, changed) = portable_relative_path("CON", Some(&raw_folder)).unwrap();
        assert!(changed);
        assert!(encoded_folder.starts_with(PORTABLE_COMPONENT_PREFIX));
        assert!(df_fs_safety::SafeRelativePath::parse(Path::new(&encoded_folder)).is_ok());

        let child = unsafe_folder.join("sub").join("informe.txt");
        let raw_child = df_domain::RawPath::from_os_str(child.as_os_str());
        let (encoded_child, changed) =
            portable_relative_path(&child.display().to_string(), Some(&raw_child)).unwrap();
        assert!(changed);
        assert_eq!(
            Path::new(&encoded_child).components().next(),
            Path::new(&encoded_folder).components().next(),
            "an unsafe ancestor must map identically for its folder and descendants"
        );
        assert!(df_fs_safety::SafeRelativePath::parse(Path::new(&encoded_child)).is_ok());

        // Reserve the encoding namespace: a literal source name can never
        // masquerade as an encoded unsafe component.
        let literal = format!("{PORTABLE_COMPONENT_PREFIX}0123456789abcdef");
        let raw_literal = df_domain::RawPath::from_os_str(std::ffi::OsStr::new(&literal));
        let (encoded_literal, changed) =
            portable_relative_path(&literal, Some(&raw_literal)).unwrap();
        assert!(changed);
        assert_ne!(encoded_literal, literal);
        assert_eq!(
            encoded_literal,
            portable_relative_path(&literal, Some(&raw_literal))
                .unwrap()
                .0
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
        for unsafe_name in ["CON.txt", "informe. ", "nombre:stream", "borrador?.txt"] {
            op.destination_relative_path = Some(unsafe_name.to_string());
            assert!(
                validate_operations(std::slice::from_ref(&op)).is_err(),
                "`{unsafe_name}` must fail during planning, not execution"
            );
        }
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

    #[test]
    fn equal_relative_folders_in_different_roots_are_not_the_same_context() {
        let tmp = tempfile::tempdir().unwrap();
        let root_a = tmp.path().join("origen-a");
        let root_b = tmp.path().join("origen-b");
        std::fs::create_dir_all(root_a.join("documentos")).unwrap();
        std::fs::create_dir_all(root_b.join("documentos")).unwrap();
        std::fs::write(
            root_a.join("documentos").join("informe.txt"),
            b"same payload",
        )
        .unwrap();
        std::fs::write(
            root_b.join("documentos").join("informe.txt"),
            b"same payload",
        )
        .unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Misma ruta relativa en dos raices",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![
            SourceRoot::new(project.id, root_a),
            SourceRoot::new(project.id, root_b),
        ];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        analyze_project(&mut db, Actor::Test).unwrap();

        let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .unwrap();
        let members = df_db::dedup::duplicate_members(&db, snapshot.id).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].parent_relative_path, "documentos");
        assert_eq!(members[1].parent_relative_path, "documentos");
        assert_ne!(
            members[0].source_root_id, members[1].source_root_id,
            "duplicate membership must preserve the source root"
        );

        let outcome = create_plan(
            &mut db,
            Actor::Test,
            DuplicatePolicy::ConsolidateWithinContext,
        )
        .unwrap();
        assert_eq!(outcome.copies, 2);
        assert_eq!(
            outcome.skipped_represented, 0,
            "equal relative paths in different roots are not WITHIN_SAME_CONTEXT"
        );
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

    /// §19.3/§19.4: two branches that are almost the same but where EACH holds
    /// something the other does not. This is the case the RFC warns about:
    /// dropping either branch loses data, so it is reported as a
    /// PARTIAL_TREE_CLONE with the evidence of what is unique on each side.
    #[test]
    fn a_partial_tree_clone_reports_the_unique_content_of_both_branches() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(origin.join("casos")).unwrap();
        std::fs::create_dir_all(origin.join("casos-2019")).unwrap();

        // Shared between both branches.
        for (name, body) in [("a.txt", b"aaa".as_slice()), ("b.txt", b"bbb".as_slice())] {
            std::fs::write(origin.join("casos").join(name), body).unwrap();
            std::fs::write(origin.join("casos-2019").join(name), body).unwrap();
        }
        // Unique to each side.
        std::fs::write(origin.join("casos").join("solo-casos.txt"), b"only here").unwrap();
        std::fs::write(
            origin.join("casos-2019").join("solo-2019.txt"),
            b"only there",
        )
        .unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Clon parcial",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        let outcome = analyze_project(&mut db, Actor::Test).unwrap();

        // Not an exact clone: the subtrees differ.
        assert_eq!(outcome.tree_clone_sets, 0);
        // But they are recognised as a partial clone.
        assert_eq!(outcome.partial_tree_clones, 1, "expected one partial clone");
        assert_eq!(outcome.embedded_trees, 0);

        let relation = df_db::structure::tree_relations(&db, outcome.snapshot_id.parse().unwrap())
            .unwrap()
            .into_iter()
            .find(|r| r.relationship == df_domain::TreeRelationship::PartialClone)
            .expect("the partial clone relation");
        assert_eq!(relation.shared_files, 2);
        // The evidence that matters: each side has exactly one unique file.
        assert_eq!(relation.unique_a_files, 1);
        assert_eq!(relation.unique_b_files, 1);
        assert!(relation.similarity > 0.4 && relation.similarity < 1.0);
    }

    /// A folder is not "embedded" in its own parent: that would be noise on
    /// every single snapshot.
    #[test]
    fn a_folder_is_never_related_to_its_own_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = hashed_project(tmp.path());
        let outcome = analyze_project(&mut db, Actor::Test).unwrap();
        // origen/ contains origen/sub/, which would trivially be "embedded".
        assert_eq!(outcome.embedded_trees, 0);
        assert_eq!(outcome.partial_tree_clones, 0);
    }

    /// A pass-through container (an ancestor holding nothing beyond one
    /// descendant folder) must not duplicate that descendant's relations.
    /// Before this filter, `Backup/` holding only `Backup/casos/` produced a
    /// second, redundant PARTIAL_TREE_CLONE against the original `casos/`.
    #[test]
    fn a_pass_through_container_does_not_duplicate_its_childs_relations() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(origin.join("casos")).unwrap();
        std::fs::create_dir_all(origin.join("Backup").join("casos")).unwrap();

        for (name, body) in [("a.txt", b"aaa".as_slice()), ("b.txt", b"bbb".as_slice())] {
            std::fs::write(origin.join("casos").join(name), body).unwrap();
            std::fs::write(origin.join("Backup").join("casos").join(name), body).unwrap();
        }
        std::fs::write(origin.join("casos").join("solo-original.txt"), b"only here").unwrap();
        std::fs::write(
            origin.join("Backup").join("casos").join("solo-backup.txt"),
            b"only there",
        )
        .unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Pasa-through",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "generic",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        let outcome = analyze_project(&mut db, Actor::Test).unwrap();

        // One relation, not two: Backup/ is a pass-through of Backup/casos.
        assert_eq!(
            outcome.partial_tree_clones, 1,
            "the pass-through container must not add a duplicate relation"
        );
        assert_eq!(outcome.embedded_trees, 0);

        // And the surviving relation is between the two deepest folders.
        let snapshot: df_domain::SnapshotId = outcome.snapshot_id.parse().unwrap();
        let views = df_db::structure::tree_relation_views(&db, snapshot).unwrap();
        assert_eq!(views.len(), 1);
        let sep = std::path::MAIN_SEPARATOR;
        assert!(
            views[0].path_a.ends_with(&format!("Backup{sep}casos"))
                || views[0].path_b.ends_with(&format!("Backup{sep}casos")),
            "the deepest folder reports the relation: {views:?}"
        );
        assert!(
            !views[0].path_a.ends_with("Backup") && !views[0].path_b.ends_with("Backup"),
            "the pass-through ancestor must not appear: {views:?}"
        );
    }
}
