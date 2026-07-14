//! DataForge facade: the stable, safe API for local clients.
//!
//! RFC-0001 rules honoured here:
//! - rule 16/17: clients (CLI, desktop) never touch `df-db` directly;
//! - rule 1: the facade never writes inside a source root;
//! - §35: a project lives in its own directory with a JSON marker and a
//!   SQLite database under `state/`.

use std::path::{Component, Path, PathBuf};

use df_db::inventory::{DuplicateSet, InventorySummary};
use df_db::{integrity::IntegrityReport, repository, Db};
use df_domain::{Actor, ProfileRef, Project, SourceRoot, TreeCloneSet};
use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};

pub use df_executor::ExecuteOutcome;
pub use df_hash::HashOutcome;
pub use df_planner::{AnalyzeOutcome, ApproveOutcome, PlanOutcome, PlanValidationReport};
pub use df_scan::ScanOutcome;
pub use df_verifier::VerifyOutcome;

/// Version of the engine that clients report and projects record.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Marker file that identifies a project directory (RFC-0001 §35/§36).
pub const PROJECT_MARKER_FILE: &str = "project.dataforge.json";

/// Location of the SQLite state inside a project directory.
pub const PROJECT_DB_RELATIVE: &str = "state/dataforge.sqlite";

const MARKER_SCHEMA: &str = "dataforge.project";
const MARKER_SCHEMA_VERSION: &str = "1.0.0";

/// Request to create a project (RFC-0001 §9.1, §35).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    /// Human name of the project.
    pub name: String,
    /// Directory that will hold the project (marker + SQLite state).
    pub project_dir: PathBuf,
    /// Where verified copies will be produced in later milestones.
    pub output_root: PathBuf,
    /// Where audit material will be produced. Defaults to `<project_dir>/audit`.
    #[serde(default)]
    pub audit_root: Option<PathBuf>,
    /// Origin directories. May be empty at creation time.
    #[serde(default)]
    pub source_roots: Vec<PathBuf>,
    /// Profile name; defaults to `generic`.
    #[serde(default)]
    pub profile: Option<String>,
}

/// Serializable view of a source root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRootView {
    pub id: String,
    pub absolute_path: String,
    pub filesystem: String,
    pub read_only_policy: bool,
}

/// Serializable view of the last audit event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventView {
    pub sequence: u64,
    pub event_type: String,
    pub timestamp: String,
    pub actor: String,
}

/// Full status of a project, as consumed by CLI `--json` and the desktop UI.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectStatus {
    pub project_id: String,
    pub name: String,
    pub state: String,
    pub profile: String,
    pub app_version: String,
    pub created_at: String,
    pub updated_at: String,
    pub project_dir: String,
    pub output_root: String,
    pub audit_root: String,
    pub source_roots: Vec<SourceRootView>,
    pub event_count: u64,
    pub last_event: Option<EventView>,
    /// Latest complete snapshot, if the project has been scanned.
    pub latest_snapshot_id: Option<String>,
    /// Inventory counters of that snapshot (files, folders, hash progress).
    pub inventory: Option<InventorySummary>,
    /// Present when an integrity pass was executed (project_status).
    pub integrity: Option<IntegrityReport>,
}

/// Content of `project.dataforge.json` (versioned per RFC-0001 §36).
#[derive(Debug, Serialize, Deserialize)]
struct ProjectMarker {
    schema: String,
    schema_version: String,
    project_id: String,
    database_path: String,
    created_at: String,
    generator_version: String,
}

/// Turn a possibly-relative user path into an absolute one (no filesystem
/// access, the path may not exist yet).
fn absolutize(path: &Path) -> DfResult<PathBuf> {
    std::path::absolute(path).map_err(|e| DfError::io(path, e))
}

/// Normalise an absolute path for containment comparison (lexical only;
/// Windows paths compare case-insensitively).
fn comparable_components(path: &Path) -> DfResult<Vec<String>> {
    if !path.is_absolute() {
        return Err(DfError::Validation(format!(
            "path `{}` must be absolute",
            path.display()
        )));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(DfError::Validation(format!(
                    "path `{}` must not contain `..`",
                    path.display()
                )));
            }
            other => parts.push(other.as_os_str().to_string_lossy().to_lowercase()),
        }
    }
    Ok(parts)
}

/// Whether `inner` equals or lies inside `outer` (lexically).
fn is_within(inner: &Path, outer: &Path) -> DfResult<bool> {
    let inner = comparable_components(inner)?;
    let outer = comparable_components(outer)?;
    Ok(inner.len() >= outer.len() && inner[..outer.len()] == outer[..])
}

fn ensure_disjoint(a: &Path, a_label: &str, b: &Path, b_label: &str) -> DfResult<()> {
    if is_within(a, b)? || is_within(b, a)? {
        return Err(DfError::Validation(format!(
            "{a_label} (`{}`) and {b_label} (`{}`) must not contain one another",
            a.display(),
            b.display()
        )));
    }
    Ok(())
}

fn validate_request(request: &CreateProjectRequest) -> DfResult<(PathBuf, ProfileRef)> {
    if request.name.trim().is_empty() {
        return Err(DfError::Validation("project name must not be empty".into()));
    }

    let project_dir = &request.project_dir;
    let output_root = &request.output_root;
    let audit_root = request
        .audit_root
        .clone()
        .unwrap_or_else(|| project_dir.join("audit"));

    // The project directory must be fresh (or an empty directory).
    if project_dir.exists() {
        if !project_dir.is_dir() {
            return Err(DfError::Validation(format!(
                "project path `{}` exists and is not a directory",
                project_dir.display()
            )));
        }
        let mut entries =
            std::fs::read_dir(project_dir).map_err(|e| DfError::io(project_dir.clone(), e))?;
        if entries.next().is_some() {
            return Err(DfError::Validation(format!(
                "project directory `{}` is not empty",
                project_dir.display()
            )));
        }
    }

    // The output must never live inside the project (RFC-0001 §35) and the
    // project must never live inside the output.
    ensure_disjoint(project_dir, "project directory", output_root, "output root")?;

    for source in &request.source_roots {
        if !source.is_dir() {
            return Err(DfError::Validation(format!(
                "source root `{}` does not exist or is not a directory",
                source.display()
            )));
        }
        // Nothing of ours may be created inside an origin (rule 1), and an
        // origin inside the output/project would self-feed later phases.
        ensure_disjoint(source, "source root", project_dir, "project directory")?;
        ensure_disjoint(source, "source root", output_root, "output root")?;
        ensure_disjoint(source, "source root", &audit_root, "audit root")?;
    }

    let profile = match request.profile.as_deref() {
        None | Some("") => ProfileRef::default(),
        Some(name) => ProfileRef::new(name),
    };

    Ok((audit_root, profile))
}

fn status_from_db(
    db: &Db,
    project_dir: &Path,
    integrity: Option<IntegrityReport>,
) -> DfResult<ProjectStatus> {
    let project: Project = repository::load_project(db)?;
    let roots = repository::load_source_roots(db, project.id)?;
    let events = repository::list_events(db, project.id)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(db, project.id)?;
    let inventory = snapshot
        .as_ref()
        .map(|s| df_db::inventory::inventory_summary(db, s.id))
        .transpose()?;

    Ok(ProjectStatus {
        project_id: project.id.to_string(),
        name: project.name.clone(),
        state: project.state.as_str().to_string(),
        profile: project.profile.as_str().to_string(),
        app_version: project.app_version.clone(),
        created_at: df_ledger::canonical_timestamp(project.created_at),
        updated_at: df_ledger::canonical_timestamp(project.updated_at),
        project_dir: project_dir.display().to_string(),
        output_root: project.output_root.display().to_string(),
        audit_root: project.audit_root.display().to_string(),
        source_roots: roots
            .iter()
            .map(|r| SourceRootView {
                id: r.id.to_string(),
                absolute_path: r.absolute_path.display().to_string(),
                filesystem: r.filesystem.as_str().to_string(),
                read_only_policy: r.read_only_policy,
            })
            .collect(),
        event_count: events.len() as u64,
        last_event: events.last().map(|e| EventView {
            sequence: e.sequence,
            event_type: e.event_type.clone(),
            timestamp: df_ledger::canonical_timestamp(e.timestamp),
            actor: e.actor.as_str().to_string(),
        }),
        latest_snapshot_id: snapshot.map(|s| s.id.to_string()),
        inventory,
        integrity,
    })
}

fn read_marker(project_dir: &Path) -> DfResult<ProjectMarker> {
    let marker_path = project_dir.join(PROJECT_MARKER_FILE);
    if !marker_path.is_file() {
        return Err(DfError::NotFound(format!(
            "`{}` is not a DataForge project (missing {PROJECT_MARKER_FILE})",
            project_dir.display()
        )));
    }
    let text =
        std::fs::read_to_string(&marker_path).map_err(|e| DfError::io(marker_path.clone(), e))?;
    let marker: ProjectMarker = serde_json::from_str(&text)
        .map_err(|e| DfError::Serialization(format!("invalid project marker: {e}")))?;
    if marker.schema != MARKER_SCHEMA {
        return Err(DfError::Validation(format!(
            "unexpected marker schema `{}`",
            marker.schema
        )));
    }
    Ok(marker)
}

fn open_db(project_dir: &Path, marker: &ProjectMarker) -> DfResult<Db> {
    let db_path = project_dir.join(&marker.database_path);
    if !db_path.is_file() {
        return Err(DfError::NotFound(format!(
            "project database `{}` is missing",
            db_path.display()
        )));
    }
    let db = Db::open(&db_path)?;
    let project = repository::load_project(&db)?;
    if project.id.to_string() != marker.project_id {
        return Err(DfError::Conflict(format!(
            "marker project id {} does not match database project id {}",
            marker.project_id, project.id
        )));
    }
    Ok(db)
}

/// Create a project directory, its SQLite state and the genesis audit event.
pub fn create_project(request: &CreateProjectRequest, actor: Actor) -> DfResult<ProjectStatus> {
    let mut request = request.clone();
    request.project_dir = absolutize(&request.project_dir)?;
    request.output_root = absolutize(&request.output_root)?;
    request.audit_root = match request.audit_root {
        Some(path) => Some(absolutize(&path)?),
        None => None,
    };
    request.source_roots = request
        .source_roots
        .iter()
        .map(|p| absolutize(p))
        .collect::<DfResult<Vec<_>>>()?;
    let request = &request;

    let (audit_root, profile) = validate_request(request)?;

    let project_dir = &request.project_dir;
    let db_path = project_dir.join(PROJECT_DB_RELATIVE);
    let state_dir = db_path
        .parent()
        .expect("PROJECT_DB_RELATIVE has a parent directory");
    std::fs::create_dir_all(state_dir).map_err(|e| DfError::io(state_dir, e))?;

    let mut project = Project::new(
        request.name.trim(),
        profile,
        request.output_root.clone(),
        audit_root,
        APP_VERSION,
    );
    let roots: Vec<SourceRoot> = request
        .source_roots
        .iter()
        .map(|path| SourceRoot::new(project.id, path.clone()))
        .collect();
    project.source_roots = roots.iter().map(|r| r.id).collect();

    let mut db = Db::open(&db_path)?;
    repository::create_project(&mut db, &project, &roots, actor)?;

    let marker = ProjectMarker {
        schema: MARKER_SCHEMA.to_string(),
        schema_version: MARKER_SCHEMA_VERSION.to_string(),
        project_id: project.id.to_string(),
        database_path: PROJECT_DB_RELATIVE.to_string(),
        created_at: df_ledger::canonical_timestamp(project.created_at),
        generator_version: APP_VERSION.to_string(),
    };
    let marker_path = project_dir.join(PROJECT_MARKER_FILE);
    let marker_json =
        serde_json::to_string_pretty(&marker).map_err(|e| DfError::Serialization(e.to_string()))?;
    std::fs::write(&marker_path, marker_json).map_err(|e| DfError::io(&marker_path, e))?;

    status_from_db(&db, project_dir, None)
}

/// Open an existing project directory (read-only, no integrity pass).
pub fn open_project(project_dir: &Path) -> DfResult<ProjectStatus> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    status_from_db(&db, &project_dir, None)
}

/// Full status of a project, including a database + ledger integrity pass.
pub fn project_status(project_dir: &Path) -> DfResult<ProjectStatus> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let report = df_db::integrity::check(&db)?;
    status_from_db(&db, &project_dir, Some(report))
}

/// Validate (if needed) and scan the source roots into a fresh snapshot
/// (RFC-0001 §12.1–§12.2). Ends in `SCANNED`.
pub fn scan_project(project_dir: &Path, actor: Actor) -> DfResult<ScanOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_scan::scan_project(&mut db, actor, &df_scan::ScanOptions::default(), None)
}

/// Hash every scanned occurrence of the latest snapshot (RFC-0001 §12.3,
/// §14). Ends in `HASHED`; safe to re-run after an interruption.
pub fn hash_project(project_dir: &Path, actor: Actor) -> DfResult<HashOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_hash::hash_project(&mut db, actor, &df_hash::HashOptions::default(), None)
}

/// Analyse the hashed snapshot: materialise exact duplicate sets (§15).
/// Ends in `ANALYZED`.
pub fn analyze_project(project_dir: &Path, actor: Actor) -> DfResult<AnalyzeOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_planner::analyze_project(&mut db, actor)
}

/// Generate and validate the plan for the analysed snapshot (§26).
/// Ends in `PLAN_READY`.
pub fn create_plan(project_dir: &Path, actor: Actor) -> DfResult<PlanOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_planner::create_plan(&mut db, actor)
}

/// Re-run the §26.5 invariants against the stored current plan.
pub fn validate_plan(project_dir: &Path) -> DfResult<PlanValidationReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    df_planner::validate_plan(&db)
}

/// Approve the current plan: canonical serialization, SHA-256, freeze
/// (§26.4). Ends in `PLAN_APPROVED`.
pub fn approve_plan(project_dir: &Path, actor: Actor) -> DfResult<ApproveOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_planner::approve_plan(&mut db, actor)
}

/// Execute the approved plan (§27). Resumable; ends in `EXECUTED` or
/// `EXECUTION_PAUSED` when work remains.
pub fn execute_plan(project_dir: &Path, actor: Actor) -> DfResult<ExecuteOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_executor::execute_plan(
        &mut db,
        actor,
        &df_executor::ExecuteOptions::default(),
        None,
    )
}

/// Verify the executed plan from primary evidence (§28). Ends in
/// `COMPLETED`, `COMPLETED_WITH_WARNINGS` or `FAILED`.
pub fn verify_project_output(project_dir: &Path, actor: Actor) -> DfResult<VerifyOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_verifier::verify_project(&mut db, actor, &df_verifier::VerifyOptions::default())
}

/// Exact duplicate report of the latest snapshot (RFC-0001 §15).
///
/// Evidence only: DataForge never infers that a duplicate is dispensable
/// (§15.2), so this report proposes no action.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateReport {
    pub snapshot_id: String,
    /// Files that are an extra copy of some content (occurrences − sets).
    pub redundant_files: u64,
    /// Bytes those extra copies occupy.
    pub redundant_bytes: u64,
    pub sets: Vec<DuplicateSet>,
}

/// Compute the exact-duplicate report of the latest complete snapshot.
pub fn duplicate_report(project_dir: &Path) -> DfResult<DuplicateReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let sets = df_db::inventory::exact_duplicates(&db, snapshot.id)?;
    let redundant_files = sets.iter().map(|s| s.occurrences.len() as u64 - 1).sum();
    let redundant_bytes = sets
        .iter()
        .map(|s| (s.occurrences.len() as u64 - 1) * s.size_bytes)
        .sum();
    Ok(DuplicateReport {
        snapshot_id: snapshot.id.to_string(),
        redundant_files,
        redundant_bytes,
        sets,
    })
}

/// Exact tree-clone report of the latest snapshot (RFC-0001 §19).
///
/// Evidence only: DataForge reports directory trees that are byte-for-byte
/// identical but never infers that a branch is dispensable before its unique
/// content is identified (§19.4), so this report proposes no action.
#[derive(Debug, Clone, Serialize)]
pub struct TreeCloneReport {
    pub snapshot_id: String,
    /// Folders that belong to some exact tree-clone set.
    pub cloned_folders: u64,
    /// Bytes the redundant copies of those subtrees occupy.
    pub redundant_bytes: u64,
    pub sets: Vec<TreeCloneSet>,
}

/// Compute the exact tree-clone report of the latest complete snapshot.
pub fn tree_clone_report(project_dir: &Path) -> DfResult<TreeCloneReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let sets = df_db::structure::tree_clone_sets(&db, snapshot.id)?;
    let cloned_folders = sets.iter().map(|s| s.folders.len() as u64).sum();
    let redundant_bytes = sets.iter().map(|s| s.redundant_bytes()).sum();
    Ok(TreeCloneReport {
        snapshot_id: snapshot.id.to_string(),
        cloned_folders,
        redundant_bytes,
        sets,
    })
}

/// Generic low-value folders of the latest snapshot (RFC-0001 §18.3).
///
/// Evidence only: a generic classification lowers a folder's ranking as a
/// canonical location but never marks its contents for removal.
#[derive(Debug, Clone, Serialize)]
pub struct ContextReport {
    pub snapshot_id: String,
    pub generic_folders: Vec<df_db::context::GenericFolder>,
}

/// Report the generic folders of the latest complete snapshot.
pub fn context_report(project_dir: &Path) -> DfResult<ContextReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let generic_folders = df_db::context::generic_folders(&db, snapshot.id)?;
    Ok(ContextReport {
        snapshot_id: snapshot.id.to_string(),
        generic_folders,
    })
}

/// Result of `dataforge audit verify`: a cryptographic pass over the ledger.
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub project_id: String,
    pub event_count: u64,
    pub ledger_ok: bool,
    /// Present when the chain fails verification.
    pub problem: Option<String>,
}

/// Verify the audit ledger chain of a project (RFC-0001 §29).
pub fn verify_audit(project_dir: &Path) -> DfResult<AuditReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let events = repository::list_events(&db, project.id)?;
    let problem = df_ledger::verify_chain(&events)
        .err()
        .map(|e| e.to_string());
    Ok(AuditReport {
        project_id: project.id.to_string(),
        event_count: events.len() as u64,
        ledger_ok: problem.is_none(),
        problem,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(base: &Path) -> CreateProjectRequest {
        CreateProjectRequest {
            name: "Expedientes 2020".to_string(),
            project_dir: base.join("proyecto"),
            output_root: base.join("salida"),
            audit_root: None,
            source_roots: vec![],
            profile: None,
        }
    }

    #[test]
    fn create_open_and_status_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        let created = create_project(&req, Actor::Test).expect("create");
        assert_eq!(created.state, "CREATED");
        assert_eq!(created.profile, "generic");
        assert_eq!(created.event_count, 1);
        assert!(req.project_dir.join(PROJECT_MARKER_FILE).is_file());
        assert!(req.project_dir.join(PROJECT_DB_RELATIVE).is_file());

        let opened = open_project(&req.project_dir).expect("open");
        assert_eq!(opened.project_id, created.project_id);
        assert!(opened.integrity.is_none());

        let status = project_status(&req.project_dir).expect("status");
        let integrity = status.integrity.expect("status runs integrity");
        assert!(integrity.is_ok(), "{:?}", integrity.problems);
        assert_eq!(
            status.last_event.as_ref().map(|e| e.event_type.as_str()),
            Some("PROJECT_CREATED")
        );
    }

    #[test]
    fn create_registers_source_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        let mut req = request(tmp.path());
        req.source_roots = vec![origin.clone()];
        let created = create_project(&req, Actor::Test).expect("create");
        assert_eq!(created.source_roots.len(), 1);
        assert!(created.source_roots[0].read_only_policy);
        // Nothing was written inside the origin (rule 1).
        assert_eq!(std::fs::read_dir(&origin).unwrap().count(), 0);
    }

    #[test]
    fn empty_name_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut req = request(tmp.path());
        req.name = "   ".to_string();
        assert!(matches!(
            create_project(&req, Actor::Test),
            Err(DfError::Validation(_))
        ));
    }

    #[test]
    fn non_empty_project_dir_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        std::fs::create_dir_all(&req.project_dir).unwrap();
        std::fs::write(req.project_dir.join("x.txt"), "x").unwrap();
        assert!(matches!(
            create_project(&req, Actor::Test),
            Err(DfError::Validation(_))
        ));
    }

    #[test]
    fn output_inside_project_is_rejected_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let mut req = request(tmp.path());
        req.output_root = req.project_dir.join("salida");
        assert!(matches!(
            create_project(&req, Actor::Test),
            Err(DfError::Validation(_))
        ));

        let mut req = request(tmp.path());
        req.project_dir = req.output_root.join("proyecto");
        assert!(matches!(
            create_project(&req, Actor::Test),
            Err(DfError::Validation(_))
        ));
    }

    #[test]
    fn missing_source_root_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut req = request(tmp.path());
        req.source_roots = vec![tmp.path().join("no-existe")];
        assert!(matches!(
            create_project(&req, Actor::Test),
            Err(DfError::Validation(_))
        ));
    }

    #[test]
    fn project_dir_inside_source_root_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().to_path_buf();
        let mut req = request(tmp.path());
        req.source_roots = vec![origin];
        // project_dir lives inside the source root -> forbidden.
        assert!(matches!(
            create_project(&req, Actor::Test),
            Err(DfError::Validation(_))
        ));
    }

    #[test]
    fn opening_a_non_project_directory_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            open_project(tmp.path()),
            Err(DfError::NotFound(_))
        ));
    }

    #[test]
    fn scan_hash_and_duplicate_report_through_the_facade() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("uno.txt"), b"contenido").unwrap();
        std::fs::write(origin.join("dos.txt"), b"contenido").unwrap();
        std::fs::write(origin.join("tres.txt"), b"distinto").unwrap();

        let mut req = request(tmp.path());
        req.source_roots = vec![origin.clone()];
        create_project(&req, Actor::Test).unwrap();

        let scan = scan_project(&req.project_dir, Actor::Test).expect("scan");
        assert_eq!(scan.files, 3);
        assert_eq!(scan.state, "SCANNED");

        let hash = hash_project(&req.project_dir, Actor::Test).expect("hash");
        assert_eq!(hash.hashed, 3);
        assert_eq!(hash.state, "HASHED");

        let report = duplicate_report(&req.project_dir).expect("duplicates");
        assert_eq!(report.sets.len(), 1);
        assert_eq!(report.redundant_files, 1);
        assert_eq!(report.redundant_bytes, 9);

        let status = project_status(&req.project_dir).expect("status");
        assert_eq!(status.state, "HASHED");
        let inventory = status.inventory.expect("inventory populated after scan");
        assert_eq!(inventory.files, 3);
        assert_eq!(inventory.hash_done, 3);
        assert!(status.integrity.expect("integrity ran").is_ok());

        let audit = verify_audit(&req.project_dir).expect("audit");
        assert!(audit.ledger_ok);
        // create + validating + ready + scanning + scan started/completed +
        // scanned + hashing + hash started/completed + hashed
        assert!(audit.event_count >= 10, "got {}", audit.event_count);

        // Nothing was written inside the origin during the whole pipeline.
        assert_eq!(std::fs::read_dir(&origin).unwrap().count(), 3);
    }

    #[test]
    fn full_pipeline_reaches_completed_through_the_facade() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(origin.join("docs")).unwrap();
        std::fs::write(origin.join("uno.txt"), b"contenido").unwrap();
        std::fs::write(origin.join("docs").join("dos.txt"), b"contenido").unwrap();

        let mut req = request(tmp.path());
        req.source_roots = vec![origin.clone()];
        create_project(&req, Actor::Test).unwrap();

        scan_project(&req.project_dir, Actor::Test).expect("scan");
        hash_project(&req.project_dir, Actor::Test).expect("hash");

        let analysis = analyze_project(&req.project_dir, Actor::Test).expect("analyze");
        assert_eq!(analysis.duplicate_sets, 1);
        assert_eq!(analysis.state, "ANALYZED");

        let plan = create_plan(&req.project_dir, Actor::Test).expect("plan");
        assert_eq!(plan.copies, 2);
        assert_eq!(plan.state, "PLAN_READY");

        let validation = validate_plan(&req.project_dir).expect("validate");
        assert!(validation.ok, "{:?}", validation.problems);

        let approval = approve_plan(&req.project_dir, Actor::Test).expect("approve");
        assert_eq!(approval.state, "PLAN_APPROVED");

        let execution = execute_plan(&req.project_dir, Actor::Test).expect("execute");
        assert_eq!(execution.state, "EXECUTED");
        assert_eq!(execution.failed_final + execution.failed_retryable, 0);

        let verification = verify_project_output(&req.project_dir, Actor::Test).expect("verify");
        assert_eq!(
            verification.verdict, "COMPLETED",
            "{:?}",
            verification.findings
        );
        assert_eq!(verification.state, "COMPLETED");

        // The verified copy exists and the origin is intact.
        assert_eq!(
            std::fs::read(req.output_root.join("origen").join("uno.txt")).unwrap(),
            b"contenido"
        );
        assert_eq!(std::fs::read(origin.join("uno.txt")).unwrap(), b"contenido");

        let audit = verify_audit(&req.project_dir).expect("audit");
        assert!(audit.ledger_ok);
    }

    #[test]
    fn duplicate_report_requires_a_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();
        assert!(matches!(
            duplicate_report(&req.project_dir),
            Err(DfError::Validation(_))
        ));
    }

    #[test]
    fn marker_and_database_must_agree_on_project_id() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();
        let marker_path = req.project_dir.join(PROJECT_MARKER_FILE);
        let text = std::fs::read_to_string(&marker_path).unwrap();
        let mut marker: serde_json::Value = serde_json::from_str(&text).unwrap();
        marker["project_id"] =
            serde_json::Value::String("00000000-0000-4000-8000-000000000000".to_string());
        std::fs::write(&marker_path, marker.to_string()).unwrap();
        assert!(matches!(
            open_project(&req.project_dir),
            Err(DfError::Conflict(_))
        ));
    }
}
