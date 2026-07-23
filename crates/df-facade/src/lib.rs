//! DataForge facade: the stable, safe API for local clients.
//!
//! RFC-0001 rules honoured here:
//! - rule 16/17: clients (CLI, desktop) never touch `df-db` directly;
//! - rule 1: the facade never writes inside a source root;
//! - §35: a project lives in its own directory with a JSON marker and a
//!   SQLite database under `state/`.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use df_db::extraction::{
    self, AnalyticalSnapshotRecord, ExtractionContentSource, ExtractionRunSpec, MailThreadInput,
    MailThreadMemberInput, SearchIndexRecord, ThreadMessageRow,
};
use df_db::inventory::{DuplicateSet, InventorySummary};
use df_db::{integrity::IntegrityReport, repository, Db};
use df_domain::{
    Actor, ExtractionRun, ExtractionRunCounters, ExtractionRunId, ExtractionRunStatus,
    FileFingerprint, MailThreadId, ProfileRef, Project, ProjectId, ProjectState, RepresentationId,
    SnapshotId, SourceRoot, TreeCloneSet,
};
use sha2::{Digest, Sha256};

/// Re-exported so clients can name a policy without depending on `df-domain`
/// (RFC-0001 rules 16/17: clients only ever talk to the facade).
pub use df_domain::DuplicatePolicy;
use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};

pub use df_db::analysis::{AnomalyReport, ReviewItemView, ReviewQueue, StructuralDiagnostics};
pub use df_domain::RuleAction;
pub use df_executor::ExecuteOptions;
pub use df_executor::ExecuteOutcome;
pub use df_hash::{HashOptions, HashOutcome};
mod ai_transport;
mod secrets;

pub use df_db::assistance::AssistanceAuditView;
pub use df_media::{MediaLimits, MediaOutcome, MediaProjectOptions, MediaSidecars};
pub use df_planner::{AnalyzeOutcome, ApproveOutcome, PlanOutcome, PlanValidationReport};
pub use df_plugin::{
    Capability as PluginCapability, PluginProjectOptions, PluginsOutcome, RegisteredPluginMetadata,
};
pub use df_scan::ScanOutcome;
pub use df_similarity::{SimilarityOptions, SimilarityOutcome};
pub use df_verifier::{VerifyOptions, VerifyOutcome};
pub use secrets::{ai_key_present, remove_ai_key, set_ai_key, AiKeyProvider};

pub use df_extract::ExtractionLimits;
pub use df_query::{QueryColumn, QueryOptions, QueryResult, SnapshotBuildOptions};
pub use df_search::{SearchBuildOptions, SearchHit, SearchRequest};

/// Version of the engine that clients report and projects record.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Marker file that identifies a project directory (RFC-0001 §35/§36).
pub const PROJECT_MARKER_FILE: &str = "project.dataforge.json";

/// Location of the SQLite state inside a project directory.
pub const PROJECT_DB_RELATIVE: &str = "state/dataforge.sqlite";

const MARKER_SCHEMA: &str = "dataforge.project";
const MARKER_SCHEMA_VERSION: &str = "1.0.0";
const DEFAULT_EXTRACTION_PAGE_SIZE: u32 = 64;
const MAX_EXTRACTION_PAGE_SIZE: u32 = 4_096;

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
    /// Structural M0.2 diagnostic for the latest snapshot. Present after a
    /// scan even when analysis has not completed, so clients can distinguish
    /// “not analysed” from a genuine zero count.
    pub structural_diagnostics: Option<StructuralDiagnostics>,
    /// Latest sealed M0.3 content-similarity evidence for this snapshot.
    pub similarity: Option<SimilarityStatus>,
    /// Latest sealed M0.5 perceptual media evidence for this snapshot.
    pub media: Option<MediaStatusReport>,
    /// Present when an integrity pass was executed (project_status).
    pub integrity: Option<IntegrityReport>,
}

/// Compact, human-readable relation. It is evidence only: `automatic_action`
/// is not a field because no such action exists in the M0.3 contract.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarityRelationView {
    pub id: String,
    pub content_a: String,
    pub content_b: String,
    pub path_a: String,
    pub path_b: String,
    pub kind: String,
    pub direction: String,
    pub similarity: f64,
    pub shared_chunks: u64,
    pub shared_bytes: u64,
    pub union_bytes: u64,
    pub estimated_similarity: f64,
    pub confidence: f64,
    pub evidence: serde_json::Value,
}

/// Sealed similarity summary embedded in project status and reports.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarityStatus {
    pub run_id: String,
    pub snapshot_id: String,
    pub algorithm_version: String,
    pub config_digest: String,
    pub config: serde_json::Value,
    pub counters: df_domain::SimilarityRunCounters,
    pub candidate_cap_reached: bool,
    pub relationships: Vec<SimilarityRelationView>,
    pub relationships_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarityReport {
    pub status: SimilarityStatus,
    pub evidence_only: bool,
}

/// Sealed media evidence of the latest run (M0.5).
#[derive(Debug, Clone, Serialize)]
pub struct MediaStatusReport {
    pub run_id: String,
    pub snapshot_id: String,
    pub contract_version: String,
    pub config_digest: String,
    pub config: serde_json::Value,
    pub counters: df_domain::MediaRunCounters,
    pub pair_cap_reached: bool,
    pub relations: Vec<df_db::media::MediaRelationView>,
    pub relations_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaReport {
    pub status: MediaStatusReport,
    pub evidence_only: bool,
}

/// Bounded, persisted content-extraction settings. `page_size` affects only
/// scheduling and is deliberately excluded from the evidence digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentExtractionOptions {
    #[serde(default)]
    pub limits: ExtractionLimits,
    #[serde(default = "default_extraction_page_size")]
    pub page_size: u32,
    /// Optional absolute path to the isolated PDF worker. This is an
    /// operational deployment choice, never part of the evidence digest.
    #[serde(default)]
    pub pdf_worker: Option<PathBuf>,
}

const fn default_extraction_page_size() -> u32 {
    DEFAULT_EXTRACTION_PAGE_SIZE
}

impl Default for ContentExtractionOptions {
    fn default() -> Self {
        Self {
            limits: ExtractionLimits::default(),
            page_size: DEFAULT_EXTRACTION_PAGE_SIZE,
            pdf_worker: None,
        }
    }
}

impl ContentExtractionOptions {
    fn validate(&self) -> DfResult<()> {
        self.limits.validate()?;
        if self.page_size == 0 || self.page_size > MAX_EXTRACTION_PAGE_SIZE {
            return Err(DfError::Validation(format!(
                "content extraction page_size must be between 1 and {MAX_EXTRACTION_PAGE_SIZE}"
            )));
        }
        if let Some(path) = self.pdf_worker.as_deref() {
            if !path.is_absolute() {
                return Err(DfError::Validation(
                    "PDF worker path must be absolute; PATH lookup is disabled".to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn validate_worker_path(path: &Path, worker_name: &str, explicit: bool) -> DfResult<()> {
    if !path.is_absolute() {
        return Err(DfError::Validation(format!(
            "{worker_name} worker path must be absolute; PATH lookup is disabled"
        )));
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if explicit && error.kind() == std::io::ErrorKind::NotFound {
            DfError::NotFound(format!("{worker_name} worker `{}`", path.display()))
        } else {
            DfError::io(path, error)
        }
    })?;
    if !metadata.is_file() || df_fs_safety::metadata_is_reparse(&metadata) {
        return Err(DfError::Validation(format!(
            "{worker_name} worker `{}` must be a plain regular file, never a reparse point",
            path.display()
        )));
    }
    Ok(())
}

fn resolve_pdf_worker(explicit: Option<&Path>) -> DfResult<Option<df_extract::PdfWorkerConfig>> {
    let path = match explicit {
        Some(path) => {
            validate_worker_path(path, "PDF", true)?;
            path.to_path_buf()
        }
        None => {
            let executable = std::env::current_exe().map_err(|error| {
                DfError::Validation(format!("cannot locate the DataForge executable: {error}"))
            })?;
            let parent = executable.parent().ok_or_else(|| {
                DfError::Validation(
                    "DataForge executable has no directory for a PDF worker sidecar".to_string(),
                )
            })?;
            #[cfg(windows)]
            let candidate = parent.join("df-extract-worker.exe");
            #[cfg(not(windows))]
            let candidate = parent.join("df-extract-worker");
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => {
                    validate_worker_path(&candidate, "PDF", false)?;
                    candidate
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(DfError::io(&candidate, error)),
            }
        }
    };
    Ok(Some(df_extract::PdfWorkerConfig::new(path)))
}

fn resolve_query_worker(explicit: Option<&Path>) -> DfResult<df_query::QueryWorkerConfig> {
    let path = match explicit {
        Some(path) => {
            validate_worker_path(path, "analytical", true)?;
            path.to_path_buf()
        }
        None => {
            let executable = std::env::current_exe().map_err(|error| {
                DfError::Validation(format!("cannot locate the DataForge executable: {error}"))
            })?;
            let parent = executable.parent().ok_or_else(|| {
                DfError::Validation(
                    "DataForge executable has no directory for an analytical worker sidecar"
                        .to_string(),
                )
            })?;
            #[cfg(windows)]
            let candidate = parent.join("df-query-worker.exe");
            #[cfg(not(windows))]
            let candidate = parent.join("df-query-worker");
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => {
                    validate_worker_path(&candidate, "analytical", false)?;
                    candidate
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(DfError::Validation(
                        "analytical SQL requires the resource-isolated `df-query-worker` sidecar; PATH lookup and in-process fallback are disabled"
                            .to_string(),
                    ))
                }
                Err(error) => return Err(DfError::io(&candidate, error)),
            }
        }
    };
    Ok(df_query::QueryWorkerConfig::new(path))
}

/// Stable CLI/desktop view of an extraction run. Per-invocation counters make
/// resumptions and cross-snapshot evidence reuse visible without weakening the
/// sealed database counters.
#[derive(Debug, Clone, Serialize)]
pub struct ContentExtractionOutcome {
    pub run_id: String,
    pub snapshot_id: String,
    pub status: String,
    pub extractor_version: String,
    pub config_digest: String,
    pub counters: ExtractionRunCounters,
    pub processed_this_invocation: u64,
    pub reused_this_invocation: u64,
    pub threads_built_this_invocation: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchIndexView {
    pub id: String,
    pub run_id: String,
    pub snapshot_id: String,
    pub schema_version: String,
    pub relative_path: String,
    pub content_digest: String,
    pub documents: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyticalSnapshotView {
    pub id: String,
    pub run_id: String,
    pub snapshot_id: String,
    pub schema_version: String,
    pub relative_path: String,
    pub sha256: String,
    pub rows: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentArtifactBuildOutcome {
    pub run_id: String,
    pub search_index: SearchIndexView,
    pub analytical_snapshot: AnalyticalSnapshotView,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentSearchOutcome {
    pub run_id: String,
    pub index: SearchIndexView,
    pub query: String,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentQueryOutcome {
    pub run_id: String,
    pub snapshot: AnalyticalSnapshotView,
    pub result: QueryResult,
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
    // Lexical separation is not enough on Windows: a junction, 8.3 alias or
    // alternate spelling can name the same physical tree. Resolve only the
    // existing prefixes (without creating either root) and fail closed.
    df_fs_safety::ensure_physical_roots_disjoint(a, b)?;
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
        // `read_dir(source)` would follow a junction before the scanner can
        // inspect any child, contradicting the no-follow source policy.
        df_fs_safety::ensure_root_is_not_reparse(source)?;
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
    for (index, source) in request.source_roots.iter().enumerate() {
        for other in request.source_roots.iter().skip(index + 1) {
            ensure_disjoint(source, "source root", other, "source root")?;
        }
    }

    let profile = match request.profile.as_deref() {
        None | Some("") => ProfileRef::default(),
        Some(name) => ProfileRef::new(name),
    };
    df_domain::Profile::load(profile.as_str())?;

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
    let structural_diagnostics = snapshot
        .as_ref()
        .map(|snapshot| df_db::analysis::diagnostics(db, snapshot.id))
        .transpose()?;
    let similarity = match (
        snapshot.as_ref(),
        df_db::similarity::latest_completed_run(db, project.id)?,
    ) {
        (Some(snapshot), Some(run)) if run.snapshot_id == snapshot.id => {
            Some(similarity_status(db, &run, 20)?)
        }
        _ => None,
    };
    let media = match (
        snapshot.as_ref(),
        df_db::media::latest_completed_run(db, project.id)?,
    ) {
        (Some(snapshot), Some(run)) if run.snapshot_id == snapshot.id => {
            let relations = df_db::media::list_media_relations(db, run.id, 20)?;
            let relations_truncated = run.counters.relations_total > relations.len() as u64;
            Some(MediaStatusReport {
                run_id: run.id.to_string(),
                snapshot_id: run.snapshot_id.to_string(),
                contract_version: run.contract_version.clone(),
                config_digest: run.config_digest.clone(),
                config: run.config.clone(),
                counters: run.counters,
                pair_cap_reached: run.pair_cap_reached,
                relations,
                relations_truncated,
            })
        }
        _ => None,
    };

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
        structural_diagnostics,
        similarity,
        media,
        integrity,
    })
}

fn similarity_status(
    db: &Db,
    run: &df_domain::SimilarityRun,
    limit: u32,
) -> DfResult<SimilarityStatus> {
    let relationships = df_db::similarity::list_relationships(db, run.id, limit)?
        .into_iter()
        .map(|relation| {
            Ok(SimilarityRelationView {
                id: relation.id.to_string(),
                content_a: relation.content_a.to_string(),
                content_b: relation.content_b.to_string(),
                path_a: df_db::similarity::representative_display_path(
                    db,
                    relation.snapshot_id,
                    relation.content_a,
                )?,
                path_b: df_db::similarity::representative_display_path(
                    db,
                    relation.snapshot_id,
                    relation.content_b,
                )?,
                kind: relation.kind.as_str().to_string(),
                direction: relation.direction.as_str().to_string(),
                similarity: relation.similarity,
                shared_chunks: relation.shared_chunks,
                shared_bytes: relation.shared_bytes,
                union_bytes: relation.union_bytes,
                estimated_similarity: relation.estimated_similarity,
                confidence: relation.confidence,
                evidence: relation.evidence,
            })
        })
        .collect::<DfResult<Vec<_>>>()?;
    let relationships_truncated = run.counters.relations_total > relationships.len() as u64;
    Ok(SimilarityStatus {
        run_id: run.id.to_string(),
        snapshot_id: run.snapshot_id.to_string(),
        algorithm_version: run.algorithm_version.clone(),
        config_digest: run.config_digest.clone(),
        config: run.config.clone(),
        counters: run.counters,
        candidate_cap_reached: run.candidate_cap_reached,
        relationships,
        relationships_truncated,
    })
}

/// Major version of a `major.minor.patch` string.
fn major_version(text: &str) -> DfResult<u32> {
    text.split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .ok_or_else(|| DfError::Validation(format!("unparsable schema_version `{text}`")))
}

/// Validate the marker (P1-3).
///
/// The marker is not the source of truth (SQLite is, rule 5), but it is the
/// front door, and it is an ordinary file a user or another program can edit.
/// So it is treated as untrusted input: every field is checked, and none of
/// them is allowed to point the engine somewhere else.
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
            "unexpected marker schema `{}` (expected `{MARKER_SCHEMA}`)",
            marker.schema
        )));
    }

    // Version policy: same major = compatible (minor/patch are additive); a
    // newer major was written by a DataForge that knows things this build does
    // not, so refuse rather than guess; an older major would need a migration
    // and none exists yet.
    let ours = major_version(MARKER_SCHEMA_VERSION)?;
    let theirs = major_version(&marker.schema_version)?;
    match theirs.cmp(&ours) {
        std::cmp::Ordering::Greater => {
            return Err(DfError::Validation(format!(
                "project marker schema_version {} was written by a newer DataForge; \
                 this build supports {MARKER_SCHEMA_VERSION}. Upgrade DataForge to open it.",
                marker.schema_version
            )))
        }
        std::cmp::Ordering::Less => {
            return Err(DfError::Validation(format!(
                "project marker schema_version {} is older than {MARKER_SCHEMA_VERSION} \
                 and no migration exists for it",
                marker.schema_version
            )))
        }
        std::cmp::Ordering::Equal => {}
    }

    // `database_path` is kept for compatibility but is NOT authoritative: it
    // must equal the constant exactly. Anything else — `..`, an absolute path,
    // a different name — is a redirection attempt. (On Windows `join` with an
    // absolute path silently discards the base, so this check is what stops
    // the engine opening `C:\somewhere\else.sqlite`.)
    if marker.database_path != PROJECT_DB_RELATIVE {
        return Err(DfError::Validation(format!(
            "project marker database_path `{}` must be exactly `{PROJECT_DB_RELATIVE}`; \
             a marker cannot point the database anywhere else",
            marker.database_path
        )));
    }

    if marker.project_id.parse::<uuid::Uuid>().is_err() {
        return Err(DfError::Validation(format!(
            "project marker project_id `{}` is not a UUID",
            marker.project_id
        )));
    }
    if marker.generator_version.trim().is_empty() {
        return Err(DfError::Validation(
            "project marker has no generator_version".to_string(),
        ));
    }
    Ok(marker)
}

fn open_db(project_dir: &Path, marker: &ProjectMarker) -> DfResult<Db> {
    // The constant, never the marker field: even validated, the field is only
    // a compatibility echo (P1-3).
    let db_path = project_dir.join(PROJECT_DB_RELATIVE);
    // `Connection::open` would happily CREATE a missing file, which would hand
    // the user an empty project instead of telling them theirs is gone.
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
    // The profile is persisted user-controlled state. Refuse unknown ids at
    // the project boundary so every caller gets the same fail-closed result.
    df_domain::Profile::load(project.profile.as_str())?;
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

    // Build the whole project in a staging directory next to its final home,
    // and only put it in place once it is sound (P1-2, ADR-0022). Doing it in
    // situ meant a failure between `create_dir_all`, the migrations and the
    // marker left a directory that could not be opened *and* was no longer
    // empty — so even retrying was refused.
    let staging = staging_dir(project_dir);
    // `build_project_in` closes the database before returning: Windows refuses
    // to rename a directory that still holds an open handle, so the staging
    // database must be shut before the finalize.
    if let Err(error) = build_project_in(&staging, request, audit_root, profile, actor) {
        // Only ever the staging directory we just created ourselves.
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    if let Err(error) = finalize_project_dir(&staging, project_dir) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let db = Db::open(&project_dir.join(PROJECT_DB_RELATIVE))?;
    status_from_db(&db, project_dir, None)
}

/// `<project_dir>.init-<uuid>`: a sibling, so the rename that finalizes it
/// stays on the same volume and is therefore atomic.
fn staging_dir(project_dir: &Path) -> PathBuf {
    let name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "dataforge".to_string());
    let parent = project_dir.parent().unwrap_or(Path::new("."));
    parent.join(format!(".{name}.init-{}", uuid::Uuid::new_v4()))
}

/// Create the structure, the database and the marker inside `staging`.
///
/// The marker is written **last**, and only after the integrity check passes:
/// it is what makes a directory a project, so it must never appear over a
/// database that is not sound.
fn build_project_in(
    staging: &Path,
    request: &CreateProjectRequest,
    audit_root: PathBuf,
    profile: ProfileRef,
    actor: Actor,
) -> DfResult<()> {
    let db_path = staging.join(PROJECT_DB_RELATIVE);
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

    // Opening applies the migrations.
    let mut db = Db::open(&db_path)?;
    repository::create_project(&mut db, &project, &roots, actor)?;

    // Prove it is sound before advertising it as a project.
    let report = df_db::integrity::check(&db)?;
    if !report.is_ok() {
        return Err(DfError::Database(format!(
            "the new project failed its integrity check: {}",
            report.problems.join("; ")
        )));
    }

    let marker = ProjectMarker {
        schema: MARKER_SCHEMA.to_string(),
        schema_version: MARKER_SCHEMA_VERSION.to_string(),
        project_id: project.id.to_string(),
        database_path: PROJECT_DB_RELATIVE.to_string(),
        created_at: df_ledger::canonical_timestamp(project.created_at),
        generator_version: APP_VERSION.to_string(),
    };
    let marker_json =
        serde_json::to_string_pretty(&marker).map_err(|e| DfError::Serialization(e.to_string()))?;
    let marker_path = staging.join(PROJECT_MARKER_FILE);
    {
        use std::io::Write;
        let mut file =
            std::fs::File::create(&marker_path).map_err(|e| DfError::io(&marker_path, e))?;
        file.write_all(marker_json.as_bytes())
            .map_err(|e| DfError::io(&marker_path, e))?;
        // Flush before the finalize: a marker that exists but is empty would be
        // worse than no marker at all.
        file.sync_all().map_err(|e| DfError::io(&marker_path, e))?;
    }

    // Close the database here: the caller is about to rename this directory,
    // and Windows will not rename a directory with an open handle inside it.
    drop(db);
    Ok(())
}

/// Move the staged project into place.
///
/// The only directory this may ever remove is one the OS itself confirms is
/// empty: `remove_dir` (never `remove_dir_all`) fails on a non-empty
/// directory, so no user data can be lost here even if the checks above were
/// wrong (P1-2).
fn finalize_project_dir(staging: &Path, project_dir: &Path) -> DfResult<()> {
    if project_dir.exists() {
        // `validate_request` already established it is an empty directory; a
        // reparse point would make "empty" meaningless, so refuse it.
        if df_fs_safety::is_reparse_point(project_dir).unwrap_or(true) {
            return Err(DfError::Validation(format!(
                "project directory `{}` is a reparse point",
                project_dir.display()
            )));
        }
        std::fs::remove_dir(project_dir).map_err(|e| DfError::io(project_dir, e))?;
    }
    std::fs::rename(staging, project_dir).map_err(|e| DfError::io(project_dir, e))
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
    hash_project_with_options(project_dir, actor, &df_hash::HashOptions::default())
}

/// `incremental: true` carries content bindings forward from the previous
/// snapshot when the v2 fingerprint is byte-identical (ADR-0035); every
/// reused binding records its provenance and full mode stays the default.
pub fn hash_project_with_options(
    project_dir: &Path,
    actor: Actor,
    options: &df_hash::HashOptions,
) -> DfResult<HashOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_hash::hash_project(&mut db, actor, options, None)
}

/// Analyse the hashed snapshot: materialise exact duplicate sets (§15).
/// Ends in `ANALYZED`.
pub fn analyze_project(project_dir: &Path, actor: Actor) -> DfResult<AnalyzeOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_planner::analyze_project(&mut db, actor)
}

/// Discover version-like relationships between non-identical contents using
/// the bounded default M0.3 configuration.
pub fn analyze_similarity(project_dir: &Path, actor: Actor) -> DfResult<SimilarityOutcome> {
    analyze_similarity_with_options(project_dir, actor, &SimilarityOptions::default())
}

/// Configurable entry point used by tests and future advanced clients. The
/// serialized options are part of the immutable run identity.
pub fn analyze_similarity_with_options(
    project_dir: &Path,
    actor: Actor,
    options: &SimilarityOptions,
) -> DfResult<SimilarityOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_similarity::analyze_project(&mut db, actor, options, None)
}

/// Perceptual media evidence over the latest snapshot (M0.5). Sidecars are
/// explicit absolute paths; audio and video without FFmpeg produce explicit
/// failure evidence instead of silently vanishing.
pub fn analyze_media(project_dir: &Path, actor: Actor) -> DfResult<MediaOutcome> {
    let mut options = MediaProjectOptions::default();
    if let Some(worker) = default_media_worker() {
        options.sidecars = options.sidecars.with_image_worker(worker);
    }
    analyze_media_with_options(project_dir, actor, &options)
}

/// Configurable entry point; the serialized limits and pair cap are part of
/// the immutable run identity, sidecar paths are not.
pub fn analyze_media_with_options(
    project_dir: &Path,
    actor: Actor,
    options: &MediaProjectOptions,
) -> DfResult<MediaOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_media::analyze_media_project(&mut db, actor, options, None)
}

/// The bundled image worker next to the running executable, if present.
/// Deterministic deployment wiring — never a `PATH` or environment lookup.
pub fn default_media_worker() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(if cfg!(windows) {
        "df-media-worker.exe"
    } else {
        "df-media-worker"
    });
    let metadata = std::fs::symlink_metadata(&candidate).ok()?;
    metadata.is_file().then_some(candidate)
}

/// Provider selection for one assistance invocation (M0.7).
#[derive(Debug, Clone)]
pub enum AiProviderChoice {
    /// A cloud provider authenticated with the key stored in the OS
    /// credential vault (BYOK). Requires explicit disclosure consent.
    Cloud {
        provider: AiKeyProvider,
        model: String,
    },
    /// Air-gapped: an explicit absolute executable run under
    /// `df-process-safety` that reads the envelope on stdin and writes the
    /// model JSON on stdout.
    LocalProcess { executable: PathBuf, model: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct AiDisclosedFieldView {
    pub evidence_id: String,
    pub field_name: String,
    pub visible_bytes: usize,
    pub redactions: usize,
    pub visible_text: String,
}

/// Everything the user must see before consenting to one cloud disclosure.
#[derive(Debug, Clone, Serialize)]
pub struct AiDisclosureView {
    pub request_id: String,
    pub purpose: String,
    pub provider: String,
    pub model: String,
    pub endpoint: String,
    pub visible_content_bytes: usize,
    pub transport_bytes: usize,
    pub fields: Vec<AiDisclosedFieldView>,
    /// Digest to pass back verbatim as consent for exactly this disclosure.
    pub disclosure_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiAssistOutcome {
    /// False when only the disclosure preview was produced.
    pub executed: bool,
    pub disclosure: AiDisclosureView,
    pub status: Option<String>,
    pub explanation: Option<String>,
    pub suggestions: Vec<df_ai::ValidatedSuggestion>,
    /// Always true: assistance explains and suggests, it never executes.
    pub evidence_only: bool,
}

fn enum_str<T: Serialize>(value: &T) -> DfResult<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or_else(|| DfError::Validation("non-string enum serialization".to_string()))
}

fn review_item_request(item: &ReviewItemView) -> df_ai::AssistanceRequest {
    let risk = match item.risk.as_str() {
        "LOW" => df_ai::LocalRisk::Low,
        "MEDIUM" => df_ai::LocalRisk::Medium,
        "HIGH" => df_ai::LocalRisk::High,
        "CRITICAL" => df_ai::LocalRisk::Critical,
        _ => df_ai::LocalRisk::Medium,
    };
    // Evidence ids are stable field names: the request carries exactly one
    // review item, uniqueness holds, and deterministic ids keep static or
    // air-gapped providers reproducible.
    let mut evidence = vec![
        df_ai::EvidenceInput {
            id: "kind".to_string(),
            field_name: "kind".to_string(),
            text: item.kind.clone(),
            local_risk: risk,
            reliability_basis_points: 9_000,
        },
        df_ai::EvidenceInput {
            id: "reason".to_string(),
            field_name: "reason".to_string(),
            text: item.reason.clone(),
            local_risk: risk,
            reliability_basis_points: 9_000,
        },
        df_ai::EvidenceInput {
            id: "recommended".to_string(),
            field_name: "recommended_action".to_string(),
            text: item.recommended_action.clone(),
            local_risk: risk,
            reliability_basis_points: 9_000,
        },
    ];
    if item.folder_a.is_some() || item.folder_b.is_some() {
        evidence.push(df_ai::EvidenceInput {
            id: "folders".to_string(),
            field_name: "folders".to_string(),
            text: format!(
                "{} | {}",
                item.folder_a.as_deref().unwrap_or("-"),
                item.folder_b.as_deref().unwrap_or("-")
            ),
            local_risk: risk,
            reliability_basis_points: 9_000,
        });
    }
    df_ai::AssistanceRequest {
        request_id: format!("review:{}", item.id),
        purpose: df_ai::AssistancePurpose::Explain,
        evidence,
        // Defaults redact paths, e-mails and phone numbers; the disclosure
        // preview shows exactly what survives.
        redaction: df_ai::RedactionConfig::default(),
    }
}

fn disclosure_view(manifest: &df_ai::DisclosureManifest) -> DfResult<AiDisclosureView> {
    Ok(AiDisclosureView {
        request_id: manifest.request_id.clone(),
        purpose: enum_str(&manifest.purpose)?,
        provider: manifest.provider.provider.clone(),
        model: manifest.provider.model.clone(),
        endpoint: manifest.provider.endpoint.clone(),
        visible_content_bytes: manifest.visible_content_bytes,
        transport_bytes: manifest.transport_bytes,
        fields: manifest
            .fields
            .iter()
            .map(|field| AiDisclosedFieldView {
                evidence_id: field.evidence_id.clone(),
                field_name: field.field_name.clone(),
                visible_bytes: field.visible_bytes,
                redactions: field.redactions.len(),
                visible_text: field.visible_text.clone(),
            })
            .collect(),
        disclosure_sha256: manifest.digest(),
    })
}

const LOCAL_AI_LIMITS: df_process_safety::ProcessLimits = df_process_safety::ProcessLimits {
    timeout: std::time::Duration::from_secs(120),
    memory_bytes: 2 * 1024 * 1024 * 1024,
    max_stdin_bytes: 1024 * 1024,
    max_stdout_bytes: 1024 * 1024,
};

/// Explain one pending review item with assisted intelligence.
///
/// Without `accept_disclosure` this is a pure preview: it returns the exact
/// disclosure manifest and sends nothing anywhere. Passing back the
/// manifest's digest is the one-invocation consent; the audit row persists
/// either way once execution happens.
pub fn ai_explain_review(
    project_dir: &Path,
    item_id: &str,
    choice: &AiProviderChoice,
    accept_disclosure: Option<&str>,
    actor: Actor,
) -> DfResult<AiAssistOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    let queue = df_db::analysis::review_queue(&db, snapshot.id)?;
    let item = queue
        .items
        .iter()
        .find(|item| item.id == item_id)
        .ok_or_else(|| {
            DfError::Validation(format!("review item `{item_id}` is not in the queue"))
        })?;

    let request = review_item_request(item);
    let (descriptor, local_provider) = match choice {
        AiProviderChoice::Cloud { provider, model } => (
            df_ai::ProviderDescriptor {
                kind: df_ai::ProviderKind::Cloud,
                provider: provider.as_str().to_string(),
                model: model.clone(),
                endpoint: match provider {
                    AiKeyProvider::Anthropic => ai_transport::ANTHROPIC_ENDPOINT.to_string(),
                    AiKeyProvider::OpenAi => ai_transport::OPENAI_ENDPOINT.to_string(),
                },
            },
            None,
        ),
        AiProviderChoice::LocalProcess { executable, model } => {
            let provider = df_ai::LocalProcessProvider::new(
                "local-process",
                model.clone(),
                executable,
                Vec::new(),
                LOCAL_AI_LIMITS,
            );
            let descriptor = df_ai::Provider::descriptor(&provider).clone();
            (descriptor, Some(provider))
        }
    };

    let engine = df_ai::AssistanceEngine::new(df_ai::AiMode::Enabled);
    let prepared = engine
        .prepare(&request, &descriptor)
        .map_err(|error| DfError::Validation(error.to_string()))?;
    let disclosure = disclosure_view(prepared.disclosure())?;

    let Some(accepted) = accept_disclosure else {
        return Ok(AiAssistOutcome {
            executed: false,
            disclosure,
            status: None,
            explanation: None,
            suggestions: Vec::new(),
            evidence_only: true,
        });
    };
    if accepted != disclosure.disclosure_sha256 {
        return Err(DfError::Validation(
            "the accepted digest does not match this disclosure; review it again".to_string(),
        ));
    }

    let outcome = match (choice, local_provider) {
        (AiProviderChoice::Cloud { provider, model }, _) => {
            let api_key = secrets::ai_key(*provider)?;
            let consent = df_ai::CloudConsentToken::grant_for(prepared.disclosure());
            match provider {
                AiKeyProvider::Anthropic => {
                    let cloud = df_ai::CloudProvider::new(
                        provider.as_str(),
                        model.clone(),
                        ai_transport::ANTHROPIC_ENDPOINT,
                        ai_transport::AnthropicTransport {
                            api_key,
                            model: model.clone(),
                        },
                    );
                    engine.execute(&prepared, &cloud, Some(&consent))
                }
                AiKeyProvider::OpenAi => {
                    let cloud = df_ai::CloudProvider::new(
                        provider.as_str(),
                        model.clone(),
                        ai_transport::OPENAI_ENDPOINT,
                        ai_transport::OpenAiTransport {
                            api_key,
                            model: model.clone(),
                        },
                    );
                    engine.execute(&prepared, &cloud, Some(&consent))
                }
            }
        }
        (AiProviderChoice::LocalProcess { .. }, Some(provider)) => {
            engine.execute(&prepared, &provider, None)
        }
        (AiProviderChoice::LocalProcess { .. }, None) => unreachable!("local provider built above"),
    };

    let audit = &outcome.audit;
    df_db::assistance::insert_audit(
        &mut db,
        project.id,
        &df_db::assistance::AssistanceAuditInput {
            request_id_sha256: audit.request_id_sha256.clone(),
            purpose: enum_str(&audit.purpose)?,
            provider_kind: enum_str(&audit.provider_kind)?,
            provider: audit.provider.clone(),
            model: audit.model.clone(),
            endpoint: descriptor.endpoint.clone(),
            status: enum_str(&audit.status)?,
            failure: audit.failure.as_ref().map(enum_str).transpose()?,
            disclosure_sha256: audit.disclosure_sha256.clone(),
            prompt_sha256: audit.prompt_sha256.clone(),
            audit_json: serde_json::to_string(audit)
                .map_err(|error| DfError::Validation(format!("audit serialization: {error}")))?,
        },
        actor,
    )?;

    Ok(AiAssistOutcome {
        executed: true,
        disclosure,
        status: Some(enum_str(&audit.status)?),
        explanation: outcome.result.as_ref().map(|r| r.explanation.clone()),
        suggestions: outcome
            .result
            .map(|result| result.suggestions)
            .unwrap_or_default(),
        evidence_only: true,
    })
}

/// Latest assistance audit rows, newest first.
pub fn ai_audit_report(
    project_dir: &Path,
    limit: u32,
) -> DfResult<Vec<df_db::assistance::AssistanceAuditView>> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    df_db::assistance::list_audits(&db, project.id, limit)
}

/// Transport file of one signed plugin package: everything except the
/// component bytes, which live in their own file next to it.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPackageFile {
    pub manifest: df_plugin::PluginManifest,
    pub component_sha256: String,
    pub publisher_public_key_hex: String,
    pub signature_hex: String,
}

/// Verify (signature, hash, manifest, ABI, compile) and persist one plugin.
pub fn register_plugin(
    project_dir: &Path,
    package_path: &Path,
    component_path: &Path,
    actor: Actor,
) -> DfResult<RegisteredPluginMetadata> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    let package_text =
        std::fs::read_to_string(package_path).map_err(|error| DfError::io(package_path, error))?;
    let package: PluginPackageFile = serde_json::from_str(&package_text)
        .map_err(|error| DfError::Validation(format!("plugin package file: {error}")))?;
    let component_bytes =
        std::fs::read(component_path).map_err(|error| DfError::io(component_path, error))?;
    df_plugin::register_project_plugin(
        &mut db,
        actor,
        df_plugin::SignedPluginPackage {
            manifest: package.manifest,
            component_sha256: package.component_sha256,
            component_bytes,
            publisher_public_key_hex: package.publisher_public_key_hex,
            signature_hex: package.signature_hex,
        },
        &PluginProjectOptions::default(),
    )
}

/// Stored registrations of the project (identity view; runs re-verify).
#[derive(Debug, Clone, Serialize)]
pub struct PluginRegistrationView {
    pub plugin: String,
    pub component_sha256: String,
    pub publisher_public_key_hex: String,
}

pub fn list_plugins(project_dir: &Path) -> DfResult<Vec<PluginRegistrationView>> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    Ok(df_db::plugins::list_registrations(&db, project.id)?
        .into_iter()
        .map(|registration| PluginRegistrationView {
            plugin: format!("{}@{}", registration.plugin_id, registration.plugin_version),
            component_sha256: registration.component_sha256,
            publisher_public_key_hex: registration.publisher_public_key_hex,
        })
        .collect())
}

/// Default execution policy: bounded subject *metadata* (paths and sizes the
/// operator already sees in every report) is granted; normalized *text*
/// stays an explicit opt-in per invocation. The host itself grants nothing.
pub fn default_plugin_options() -> PluginProjectOptions {
    let mut options = PluginProjectOptions::default();
    options
        .policy
        .granted_capabilities
        .insert(df_plugin::Capability::SubjectMetadata);
    options
}

/// Execute every registered plugin over the latest analysed snapshot.
pub fn run_plugins(project_dir: &Path, actor: Actor) -> DfResult<PluginsOutcome> {
    run_plugins_with_options(project_dir, actor, &default_plugin_options())
}

pub fn run_plugins_with_options(
    project_dir: &Path,
    actor: Actor,
    options: &PluginProjectOptions,
) -> DfResult<PluginsOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_plugin::run_project_plugins(&mut db, actor, options, None)
}

/// Sealed plugin findings of the latest snapshot's completed runs.
#[derive(Debug, Clone, Serialize)]
pub struct PluginReport {
    pub snapshot_id: String,
    pub runs: Vec<df_db::plugins::PluginRunView>,
    pub findings: Vec<df_db::plugins::PluginFindingView>,
    pub evidence_only: bool,
}

pub fn plugin_report(project_dir: &Path) -> DfResult<PluginReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let runs = df_db::plugins::latest_completed_runs(&db, project.id, snapshot.id)?;
    if runs.is_empty() {
        return Err(DfError::Validation(
            "the latest snapshot has no completed plugin runs; run plugins first".to_string(),
        ));
    }
    let mut findings = Vec::new();
    for run in &runs {
        let run_id = run.run_id.parse()?;
        findings.extend(df_db::plugins::list_findings(&db, run_id, 500)?);
    }
    Ok(PluginReport {
        snapshot_id: snapshot.id.to_string(),
        runs,
        findings,
        evidence_only: true,
    })
}

/// Latest sealed media evidence. Relations are explicitly informational and
/// cannot be translated to plan operations by this API.
pub fn media_report(project_dir: &Path) -> DfResult<MediaReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let run = df_db::media::latest_completed_run(&db, project.id)?
        .filter(|run| run.snapshot_id == snapshot.id)
        .ok_or_else(|| {
            DfError::Validation(
                "the latest snapshot has no completed media run; run media analysis first"
                    .to_string(),
            )
        })?;
    let relations = df_db::media::list_media_relations(&db, run.id, 1_000)?;
    let relations_truncated = run.counters.relations_total > relations.len() as u64;
    Ok(MediaReport {
        status: MediaStatusReport {
            run_id: run.id.to_string(),
            snapshot_id: run.snapshot_id.to_string(),
            contract_version: run.contract_version.clone(),
            config_digest: run.config_digest.clone(),
            config: run.config.clone(),
            counters: run.counters,
            pair_cap_reached: run.pair_cap_reached,
            relations,
            relations_truncated,
        },
        evidence_only: true,
    })
}

/// Latest sealed similarity evidence. Relations are explicitly informational
/// and cannot be translated to plan operations by this API.
pub fn similarity_report(project_dir: &Path) -> DfResult<SimilarityReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let run = df_db::similarity::latest_completed_run(&db, project.id)?
        .filter(|run| run.snapshot_id == snapshot.id)
        .ok_or_else(|| {
            DfError::Validation(
                "the latest snapshot has no completed similarity run; run similarity first"
                    .to_string(),
            )
        })?;
    Ok(SimilarityReport {
        status: similarity_status(&db, &run, 1_000)?,
        evidence_only: true,
    })
}

fn extraction_outcome(
    run: ExtractionRun,
    processed: u64,
    reused: u64,
    threads: u64,
) -> ContentExtractionOutcome {
    ContentExtractionOutcome {
        run_id: run.id.to_string(),
        snapshot_id: run.snapshot_id.to_string(),
        status: run.status.as_str().to_string(),
        extractor_version: run.extractor_version,
        config_digest: run.config_digest,
        counters: run.counters,
        processed_this_invocation: processed,
        reused_this_invocation: reused,
        threads_built_this_invocation: threads,
        error: run.error,
    }
}

fn extraction_spec(
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    limits: &ExtractionLimits,
) -> DfResult<ExtractionRunSpec> {
    limits.validate()?;
    Ok(ExtractionRunSpec {
        project_id,
        snapshot_id,
        extractor_version: df_extract::EXTRACTOR_VERSION.to_string(),
        // The extractor version is a separate part of the database identity.
        // The digest is intentionally just the canonical limit structure.
        config_digest: limits.digest()?,
        config_json: serde_json::to_string(limits)
            .map_err(|error| DfError::Serialization(format!("extraction limits: {error}")))?,
        max_input_bytes: limits.max_input_bytes,
        max_text_chars: limits.max_text_chars,
        text_segment_chars: limits.text_segment_chars,
        max_archive_entries: u32::try_from(limits.max_archive_entries)
            .map_err(|_| DfError::Validation("max_archive_entries does not fit u32".to_string()))?,
        max_archive_entry_bytes: limits.max_archive_entry_bytes,
        max_archive_total_bytes: limits.max_archive_total_bytes,
        max_archive_ratio: limits.max_archive_compression_ratio as f64,
        max_archive_depth: u32::try_from(limits.max_archive_nesting_depth).map_err(|_| {
            DfError::Validation("max_archive_nesting_depth does not fit u32".to_string())
        })?,
    })
}

fn raw_source_relative(source: &ExtractionContentSource) -> DfResult<PathBuf> {
    let raw = source.raw_relative_path.as_ref().ok_or_else(|| {
        DfError::Validation(format!(
            "content `{}` cannot be reopened safely because its raw path evidence is missing",
            source.content_id
        ))
    })?;
    if raw.is_lossy() {
        return Err(DfError::Validation(format!(
            "content `{}` has a raw path that the strong read-lease boundary cannot represent safely",
            source.content_id
        )));
    }
    let relative = PathBuf::from(raw.to_os_string());
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(DfError::Validation(format!(
            "content `{}` has a non-relative raw source path",
            source.content_id
        )));
    }
    Ok(relative)
}

struct VerifiedSource {
    bytes: Vec<u8>,
    sha256: String,
}

/// Read only the bounded prefix needed by the extractor while streaming the
/// whole file through SHA-256. Fingerprint, physical root identity, size and
/// canonical digest are checked around the read to close the ordinary TOCTOU
/// windows without ever opening a source for writing.
fn read_verified_source(
    source: &ExtractionContentSource,
    max_input_bytes: u64,
) -> DfResult<VerifiedSource> {
    let relative = raw_source_relative(source)?;
    let safe_relative = df_fs_safety::SafeRelativePath::parse(&relative)?;
    let safe_root = df_fs_safety::SafeOutputRoot::validate(&source.root_path)?;
    let lease = safe_root.lease_existing_file(&safe_relative)?;
    let path = lease.path().to_path_buf();
    let stored = FileFingerprint::parse(&source.fingerprint)?;
    let pre = df_fs_safety::capture_fingerprint(&path)?;
    if FileFingerprint::compare(&stored, &pre).is_changed() || pre.size_bytes() != source.size_bytes
    {
        return Err(DfError::Conflict(format!(
            "source `{}` changed after inventory/hash evidence was recorded",
            source.relative_path
        )));
    }

    let prefix_limit = usize::try_from(max_input_bytes)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            DfError::Validation("max_input_bytes cannot reserve a bounded prefix".to_string())
        })?;
    let mut file = lease
        .file()
        .try_clone()
        .map_err(|error| DfError::io(&path, error))?;
    file.rewind().map_err(|error| DfError::io(&path, error))?;
    let mut digest = Sha256::new();
    let source_capacity = usize::try_from(source.size_bytes).unwrap_or(prefix_limit);
    let mut bytes = Vec::with_capacity(prefix_limit.min(source_capacity));
    let mut buffer = [0_u8; 64 * 1024];
    let mut read_total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DfError::io(&path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        read_total = read_total
            .checked_add(read as u64)
            .ok_or_else(|| DfError::Validation("source byte count overflow".to_string()))?;
        if bytes.len() < prefix_limit {
            let take = read.min(prefix_limit - bytes.len());
            bytes.extend_from_slice(&buffer[..take]);
        }
    }
    drop(file);

    let post = df_fs_safety::capture_fingerprint(&path)?;
    safe_root.revalidate()?;
    if FileFingerprint::compare(&pre, &post).is_changed()
        || FileFingerprint::compare(&stored, &post).is_changed()
        || read_total != source.size_bytes
    {
        return Err(DfError::Conflict(format!(
            "source `{}` changed while content was being read",
            source.relative_path
        )));
    }
    let sha256 = hex::encode(digest.finalize());
    if sha256 != source.sha256 {
        return Err(DfError::Conflict(format!(
            "source `{}` no longer matches its canonical SHA-256",
            source.relative_path
        )));
    }
    Ok(VerifiedSource { bytes, sha256 })
}

fn normalized_message_id(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(['<', '>']).trim();
    (!value.is_empty()).then(|| value.to_lowercase())
}

fn deterministic_thread_id(
    run_id: ExtractionRunId,
    root_representation_id: RepresentationId,
) -> MailThreadId {
    let mut digest = Sha256::new();
    digest.update(b"dataforge-mail-thread-v1\0");
    digest.update(run_id.to_string().as_bytes());
    digest.update([0]);
    digest.update(root_representation_id.to_string().as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 variant + custom version 8 bits communicate that this is a
    // stable SHA-256-derived identifier rather than a random UUIDv4.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    MailThreadId::from_str(&uuid::Uuid::from_bytes(bytes).to_string())
        .expect("a constructed UUID always parses as a typed thread id")
}

fn reconstruct_mail_threads(
    run_id: ExtractionRunId,
    messages: &[ThreadMessageRow],
) -> Vec<MailThreadInput> {
    let mut positions = HashMap::new();
    let mut duplicate_ids = HashSet::new();
    for (position, message) in messages.iter().enumerate() {
        if let Some(id) = message
            .message_id
            .as_deref()
            .and_then(normalized_message_id)
        {
            if positions.insert(id.clone(), position).is_some() {
                duplicate_ids.insert(id);
            }
        }
    }
    for id in duplicate_ids {
        positions.remove(&id);
    }

    let mut parents = vec![None; messages.len()];
    let mut last_by_subject: HashMap<&str, usize> = HashMap::new();
    for (position, message) in messages.iter().enumerate() {
        let explicit = message
            .references
            .iter()
            .rev()
            .chain(message.in_reply_to.iter().rev())
            .filter_map(|id| normalized_message_id(id))
            .filter_map(|id| positions.get(&id).copied())
            .find(|candidate| *candidate < position);
        let subject = message
            .normalized_subject
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        parents[position] =
            explicit.or_else(|| subject.and_then(|key| last_by_subject.get(key).copied()));
        if let Some(subject) = subject {
            last_by_subject.insert(subject, position);
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for position in 0..messages.len() {
        let mut root = position;
        while let Some(parent) = parents[root] {
            root = parent;
        }
        groups.entry(root).or_default().push(position);
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by_key(|(root, _)| messages[*root].representation_id.to_string());
    groups
        .into_iter()
        .map(|(root, members)| MailThreadInput {
            id: deterministic_thread_id(run_id, messages[root].representation_id),
            root_message_id: messages[root].message_id.clone(),
            normalized_subject: messages[root].normalized_subject.clone(),
            members: members
                .into_iter()
                .map(|position| MailThreadMemberInput {
                    representation_id: messages[position].representation_id,
                    parent_representation_id: parents[position]
                        .map(|parent| messages[parent].representation_id),
                })
                .collect(),
        })
        .collect()
}

fn build_threads_if_needed(db: &mut Db, run_id: ExtractionRunId, actor: Actor) -> DfResult<u64> {
    let messages = extraction::mail_messages_for_threading(db, run_id)?;
    let (thread_count, member_count) = extraction::mail_thread_counts(db, run_id)?;
    if thread_count > 0 {
        if usize::try_from(member_count).ok() != Some(messages.len()) {
            return Err(DfError::LedgerIntegrity(format!(
                "run `{run_id}` has partial mail-thread evidence"
            )));
        }
        return Ok(0);
    }
    let threads = reconstruct_mail_threads(run_id, &messages);
    extraction::persist_mail_threads(db, run_id, &threads, actor)
}

/// Extract all unique contents of the latest analysed snapshot. Committed
/// contents are omitted on replay, so rerunning after interruption resumes at
/// the first gap. Operational errors leave the run RUNNING: after restoring
/// the immutable source, the same command can safely continue.
pub fn extract_project_content(
    project_dir: &Path,
    actor: Actor,
    options: &ContentExtractionOptions,
) -> DfResult<ContentExtractionOutcome> {
    options.validate()?;
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let spec = extraction_spec(project.id, snapshot.id, &options.limits)?;
    let run = extraction::start_or_resume_run(&mut db, &spec, actor)?;
    if run.status == ExtractionRunStatus::Completed {
        return Ok(extraction_outcome(run, 0, 0, 0));
    }
    let pdf_worker = resolve_pdf_worker(options.pdf_worker.as_deref())?;

    let mut processed = 0_u64;
    let mut reused = 0_u64;
    let mut after: Option<String> = None;
    loop {
        let sources = extraction::pending_content_sources_after(
            &db,
            run.id,
            after.as_deref(),
            options.page_size,
        )?;
        if sources.is_empty() {
            break;
        }
        for source in &sources {
            if source.reusable_representation_id.is_some()
                && extraction::bind_reusable_representation(&mut db, run.id, source.content_id)?
                    .is_some()
            {
                reused = reused.checked_add(1).ok_or_else(|| {
                    DfError::Validation("extraction reuse counter overflow".to_string())
                })?;
            } else {
                let verified = read_verified_source(source, options.limits.max_input_bytes)?;
                let representation_id = RepresentationId::new();
                let request = df_extract::ExtractionRequest {
                    content_id: source.content_id,
                    representation_id,
                    source_sha256: &verified.sha256,
                    source_size_bytes: source.size_bytes,
                    display_name: &source.file_name,
                    mime_hint: None,
                    bytes: &verified.bytes,
                    extractor_version: df_extract::EXTRACTOR_VERSION,
                    config_digest: &spec.config_digest,
                    created_at: chrono::Utc::now(),
                };
                let bundle = match pdf_worker.as_ref() {
                    Some(worker) => {
                        df_extract::extract_with_pdf_worker(request, &options.limits, worker)?
                    }
                    None => df_extract::extract(request, &options.limits)?,
                };
                let input = bundle.into_db_input()?;
                extraction::persist_content_result(&mut db, run.id, &input)?;
                processed = processed.checked_add(1).ok_or_else(|| {
                    DfError::Validation("extraction processed counter overflow".to_string())
                })?;
            }
        }
        after = sources.last().map(|source| source.content_id.to_string());
        if sources.len() < options.page_size as usize {
            break;
        }
    }
    let threads = build_threads_if_needed(&mut db, run.id, actor)?;
    let completed = extraction::complete_run(&mut db, run.id, actor)?;
    Ok(extraction_outcome(completed, processed, reused, threads))
}

/// Explicitly seal an unrecoverable extraction run. Normal extraction errors
/// do not call this: keeping the run open is what makes source restoration and
/// crash recovery possible.
pub fn fail_content_extraction(
    project_dir: &Path,
    run_id: &str,
    reason: &str,
    actor: Actor,
) -> DfResult<ContentExtractionOutcome> {
    if reason.trim().is_empty() || reason.chars().count() > 4_096 {
        return Err(DfError::Validation(
            "failure reason must contain between 1 and 4096 characters".to_string(),
        ));
    }
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let run_id = ExtractionRunId::from_str(run_id)?;
    let run = extraction::load_run(&db, run_id)?;
    if run.project_id != project.id {
        return Err(DfError::NotFound(format!("extraction run `{run_id}`")));
    }
    let failed = extraction::fail_run(&mut db, run_id, reason.trim(), actor)?;
    Ok(extraction_outcome(failed, 0, 0, 0))
}

fn resolve_completed_extraction_run(
    db: &Db,
    project_id: ProjectId,
    requested: Option<&str>,
) -> DfResult<ExtractionRun> {
    let run_id = match requested {
        Some(id) => ExtractionRunId::from_str(id)?,
        None => {
            let snapshot =
                df_db::inventory::latest_complete_snapshot(db, project_id)?.ok_or_else(|| {
                    DfError::Validation("the project has no complete snapshot".to_string())
                })?;
            extraction::latest_completed_run(db, project_id, Some(snapshot.id))?
                .ok_or_else(|| {
                    DfError::Validation(
                        "the latest snapshot has no completed content extraction; run `content extract` first"
                            .to_string(),
                    )
                })?
                .id
        }
    };
    let run = extraction::load_run(db, run_id)?;
    if run.project_id != project_id {
        return Err(DfError::NotFound(format!("extraction run `{run_id}`")));
    }
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::Conflict(format!(
            "extraction run `{run_id}` is not completed"
        )));
    }
    Ok(run)
}

fn search_index_view(record: &SearchIndexRecord) -> SearchIndexView {
    SearchIndexView {
        id: record.id.to_string(),
        run_id: record.run_id.to_string(),
        snapshot_id: record.snapshot_id.to_string(),
        schema_version: record.schema_version.clone(),
        relative_path: record.relative_path.clone(),
        content_digest: record.content_digest.clone(),
        documents: record.documents,
        created_at: df_ledger::canonical_timestamp(record.created_at),
    }
}

fn analytical_snapshot_view(record: &AnalyticalSnapshotRecord) -> AnalyticalSnapshotView {
    AnalyticalSnapshotView {
        id: record.id.to_string(),
        run_id: record.run_id.to_string(),
        snapshot_id: record.snapshot_id.to_string(),
        schema_version: record.schema_version.clone(),
        relative_path: record.relative_path.clone(),
        sha256: record.sha256.clone(),
        rows: record.rows,
        created_at: df_ledger::canonical_timestamp(record.created_at),
    }
}

fn ensure_content_artifacts_outside_sources(
    db: &Db,
    project_id: ProjectId,
    project_dir: &Path,
) -> DfResult<()> {
    for source in repository::load_source_roots(db, project_id)? {
        ensure_disjoint(
            &source.absolute_path,
            "source root",
            project_dir,
            "content artifact root",
        )?;
    }
    Ok(())
}

/// Rebuild both disposable M0.4 artifacts from sealed SQLite evidence.
pub fn build_content_artifacts(
    project_dir: &Path,
    run_id: Option<&str>,
    search_options: SearchBuildOptions,
    snapshot_options: SnapshotBuildOptions,
    actor: Actor,
) -> DfResult<ContentArtifactBuildOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    ensure_content_artifacts_outside_sources(&db, project.id, &project_dir)?;
    let run = resolve_completed_extraction_run(&db, project.id, run_id)?;
    let search = df_search::build_index(&mut db, run.id, &project_dir, search_options, actor)?;
    let analytical = df_query::build_analytical_snapshot(
        &mut db,
        run.id,
        &project_dir,
        snapshot_options,
        actor,
    )?;
    Ok(ContentArtifactBuildOutcome {
        run_id: run.id.to_string(),
        search_index: search_index_view(&search),
        analytical_snapshot: analytical_snapshot_view(&analytical),
    })
}

/// Search the newest verified index for a completed extraction run.
pub fn search_project_content(
    project_dir: &Path,
    run_id: Option<&str>,
    request: &SearchRequest,
) -> DfResult<ContentSearchOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    ensure_content_artifacts_outside_sources(&db, project.id, &project_dir)?;
    let run = resolve_completed_extraction_run(&db, project.id, run_id)?;
    let index = extraction::latest_search_index(&db, run.id)?.ok_or_else(|| {
        DfError::Validation(
            "the extraction run has no search index; run `content build` first".to_string(),
        )
    })?;
    let hits = df_search::search_index(&project_dir, &index, request)?;
    Ok(ContentSearchOutcome {
        run_id: run.id.to_string(),
        index: search_index_view(&index),
        query: request.query.clone(),
        hits,
    })
}

/// Execute bounded, read-only SQL against the newest registered Parquet
/// snapshot using only the trusted sibling analytical worker.
pub fn query_project_content(
    project_dir: &Path,
    run_id: Option<&str>,
    sql: &str,
    options: QueryOptions,
) -> DfResult<ContentQueryOutcome> {
    query_project_content_with_worker(project_dir, run_id, sql, options, None)
}

/// Same query boundary with an explicit absolute worker path. This exists for
/// development and managed deployments; PATH and environment discovery remain
/// disabled and there is no in-process fallback for untrusted SQL.
pub fn query_project_content_with_worker(
    project_dir: &Path,
    run_id: Option<&str>,
    sql: &str,
    options: QueryOptions,
    query_worker: Option<&Path>,
) -> DfResult<ContentQueryOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    ensure_content_artifacts_outside_sources(&db, project.id, &project_dir)?;
    let run = resolve_completed_extraction_run(&db, project.id, run_id)?;
    let snapshot = extraction::latest_analytical_snapshot(&db, run.id)?.ok_or_else(|| {
        DfError::Validation(
            "the extraction run has no analytical snapshot; run `content build` first".to_string(),
        )
    })?;
    drop(db);
    let requested_memory = u64::try_from(options.memory_limit_bytes).map_err(|_| {
        DfError::Validation("analytical memory limit does not fit worker protocol".to_string())
    })?;
    let worker_memory = (1024_u64 * 1024 * 1024).max(requested_memory);
    let worker = resolve_query_worker(query_worker)?.with_memory_limit_bytes(worker_memory);
    let result = df_query::query_snapshot_isolated(&project_dir, &snapshot, sql, options, &worker)?;
    Ok(ContentQueryOutcome {
        run_id: run.id.to_string(),
        snapshot: analytical_snapshot_view(&snapshot),
        result,
    })
}

/// Generate and validate the plan for the analysed snapshot (§26).
/// Ends in `PLAN_READY`.
///
/// `policy` decides what happens to exact duplicates (§15.4); it defaults to
/// `REPORT_ONLY`, which copies every occurrence. No policy ever consolidates
/// a copy that sits in a protected boundary (rule 9).
pub fn create_plan(
    project_dir: &Path,
    actor: Actor,
    policy: DuplicatePolicy,
) -> DfResult<PlanOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_planner::create_plan(&mut db, actor, policy)
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
    execute_plan_with_options(project_dir, actor, &df_executor::ExecuteOptions::default())
}

/// `allow_degraded_destination` acknowledges writing to a filesystem
/// without physical identity guarantees (ADR-0036); refused otherwise.
pub fn execute_plan_with_options(
    project_dir: &Path,
    actor: Actor,
    options: &df_executor::ExecuteOptions,
) -> DfResult<ExecuteOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_executor::execute_plan(&mut db, actor, options, None)
}

/// Verify the executed plan from primary evidence (§28). Ends in
/// `COMPLETED`, `COMPLETED_WITH_WARNINGS` or `FAILED`.
pub fn verify_project_output(project_dir: &Path, actor: Actor) -> DfResult<VerifyOutcome> {
    verify_project_output_with_options(project_dir, actor, &df_verifier::VerifyOptions::default())
}

/// Verify with explicit tuning (parallel re-hash workers, M1.0.1). The
/// verdict and findings are identical for any worker count.
pub fn verify_project_output_with_options(
    project_dir: &Path,
    actor: Actor,
    options: &df_verifier::VerifyOptions,
) -> DfResult<VerifyOutcome> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    df_verifier::verify_project(&mut db, actor, options)
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

/// Reports require two independent pieces of evidence for the latest snapshot:
///
/// - a stable lifecycle state at or beyond ANALYZED; and
/// - the append-only completion marker written by the final analysis stage.
///
/// The state keeps half-built results hidden after a crash in ANALYZING. The
/// marker keeps a manually advanced or otherwise inconsistent state from
/// exposing empty tables as a genuine report.
fn ensure_snapshot_analysis_complete(
    db: &Db,
    project: &Project,
    snapshot_id: SnapshotId,
) -> DfResult<()> {
    let stable_after_analysis = matches!(
        project.state,
        ProjectState::Analyzed
            | ProjectState::Planning
            | ProjectState::PlanReady
            | ProjectState::PlanReview
            | ProjectState::PlanApproved
            | ProjectState::Executing
            | ProjectState::ExecutionPaused
            | ProjectState::Executed
            | ProjectState::Verifying
            | ProjectState::Completed
            | ProjectState::CompletedWithWarnings
            | ProjectState::Failed
            | ProjectState::Archived
    );
    if !stable_after_analysis {
        return Err(DfError::Validation(format!(
            "analysis for snapshot `{snapshot_id}` has not completed; run analyze before requesting reports"
        )));
    }
    df_db::analysis::require_current_analysis_completion(
        db,
        project.id,
        snapshot_id,
        project.profile.as_str(),
    )
}

/// Compute the exact-duplicate report of the latest complete snapshot.
pub fn duplicate_report(project_dir: &Path) -> DfResult<DuplicateReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
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
    /// Conservative, non-overlapping estimate of bytes occupied by redundant
    /// copies. Nested clone sets contribute at most once.
    pub redundant_bytes: u64,
    pub sets: Vec<TreeCloneSet>,
}

struct RedundantSubtree<'a> {
    components: Vec<String>,
    path: &'a str,
    signature: &'a str,
    bytes: u64,
}

fn clone_path_components(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect()
}

fn clone_paths_overlap(left: &[String], right: &[String]) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Estimate redundant clone bytes without counting a nested subtree twice.
///
/// Each set keeps its lexicographically first folder as the canonical copy.
/// The remaining copies are considered from the shallowest path outwards, so
/// a selected subtree excludes both its ancestors and descendants. The result
/// is deterministic and may deliberately undercount ambiguous overlaps.
fn non_overlapping_redundant_bytes(sets: &[TreeCloneSet]) -> u64 {
    let mut candidates = Vec::new();

    for set in sets {
        let mut folders: Vec<_> = set
            .folders
            .iter()
            .map(|path| (clone_path_components(path), path.as_str()))
            .collect();
        folders.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
        folders.dedup_by(|left, right| left.0 == right.0);

        candidates.extend(
            folders
                .into_iter()
                .skip(1)
                .map(|(components, path)| RedundantSubtree {
                    components,
                    path,
                    signature: &set.signature,
                    bytes: set.subtree_bytes,
                }),
        );
    }

    candidates.sort_by(|left, right| {
        left.components
            .len()
            .cmp(&right.components.len())
            .then_with(|| right.bytes.cmp(&left.bytes))
            .then_with(|| left.components.cmp(&right.components))
            .then_with(|| left.path.cmp(right.path))
            .then_with(|| left.signature.cmp(right.signature))
    });

    let mut selected = Vec::<Vec<String>>::new();
    let mut redundant_bytes = 0_u64;
    for candidate in candidates {
        if selected
            .iter()
            .any(|path| clone_paths_overlap(path, &candidate.components))
        {
            continue;
        }

        redundant_bytes = redundant_bytes.saturating_add(candidate.bytes);
        selected.push(candidate.components);
    }

    redundant_bytes
}

/// Compute the exact tree-clone report of the latest complete snapshot.
pub fn tree_clone_report(project_dir: &Path) -> DfResult<TreeCloneReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    let sets = df_db::structure::tree_clone_sets(&db, snapshot.id)?;
    let cloned_folders = sets.iter().map(|s| s.folders.len() as u64).sum();
    let redundant_bytes = non_overlapping_redundant_bytes(&sets);
    Ok(TreeCloneReport {
        snapshot_id: snapshot.id.to_string(),
        cloned_folders,
        redundant_bytes,
        sets,
    })
}

/// Folders that share content without being identical (RFC-0001 §19.3).
///
/// Evidence only, and the most important kind: a `PARTIAL_TREE_CLONE` has
/// unique content on **both** sides, so dropping either branch loses data
/// (§19.4). Nothing here proposes an action.
#[derive(Debug, Clone, Serialize)]
pub struct TreeRelationReport {
    pub snapshot_id: String,
    /// Pairs where both sides hold something the other does not.
    pub partial_clones: u64,
    /// Pairs where one subtree's content is wholly inside the other's.
    pub embedded: u64,
    /// Pairs whose only meaningful overlap is a repeated component.
    pub repeated_components: u64,
    pub relations: Vec<df_db::structure::TreeRelationView>,
}

/// Report the tree relations of the latest complete snapshot.
pub fn tree_relation_report(project_dir: &Path) -> DfResult<TreeRelationReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    let relations = df_db::structure::tree_relation_views(&db, snapshot.id)?;
    let partial_clones = relations
        .iter()
        .filter(|r| r.relationship == "PARTIAL_TREE_CLONE")
        .count() as u64;
    let embedded = relations
        .iter()
        .filter(|r| r.relationship == "TREE_EMBEDDED")
        .count() as u64;
    let repeated_components = relations
        .iter()
        .filter(|r| r.relationship == "REPEATED_COMPONENT_ONLY")
        .count() as u64;
    Ok(TreeRelationReport {
        snapshot_id: snapshot.id.to_string(),
        partial_clones,
        embedded,
        repeated_components,
        relations,
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
    pub protected_folders: Vec<df_db::context::ProtectedFolder>,
}

/// Report the generic folders of the latest complete snapshot.
pub fn context_report(project_dir: &Path) -> DfResult<ContextReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    let generic_folders = df_db::context::generic_folders(&db, snapshot.id)?;
    let protected_folders = df_db::context::protected_folders(&db, snapshot.id)?;
    Ok(ContextReport {
        snapshot_id: snapshot.id.to_string(),
        generic_folders,
        protected_folders,
    })
}

/// Structural anomalies of the completed latest analysis.
pub fn structural_anomaly_report(project_dir: &Path) -> DfResult<AnomalyReport> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    df_db::analysis::anomaly_report(&db, snapshot.id)
}

/// Human review queue for structural findings and review-class rules.
pub fn structural_review_queue(project_dir: &Path) -> DfResult<ReviewQueue> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    df_db::analysis::review_queue(&db, snapshot.id)
}

/// Append a review decision before planning. Decisions after a plan exists
/// would not change that immutable plan, so they are rejected explicitly.
pub fn decide_structural_review(
    project_dir: &Path,
    item_id: &str,
    decision: RuleAction,
    rationale: &str,
    actor: Actor,
) -> DfResult<ReviewQueue> {
    let project_dir = absolutize(project_dir)?;
    let marker = read_marker(&project_dir)?;
    let mut db = open_db(&project_dir, &marker)?;
    let project = repository::load_project(&db)?;
    if project.state != ProjectState::Analyzed {
        return Err(DfError::Validation(format!(
            "review decisions require ANALYZED state before planning (current {})",
            project.state
        )));
    }
    let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    ensure_snapshot_analysis_complete(&db, &project, snapshot.id)?;
    df_db::analysis::decide_review_item(&mut db, project.id, item_id, decision, rationale, actor)?;
    df_db::analysis::review_queue(&db, snapshot.id)
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

    fn clone_set(tag: char, folders: &[&str], subtree_bytes: u64) -> TreeCloneSet {
        TreeCloneSet {
            id: df_domain::TreeCloneSetId::new(),
            snapshot_id: SnapshotId::new(),
            signature: tag.to_string().repeat(64),
            relationship: df_domain::TreeRelationship::ExactClone,
            folders: folders.iter().map(|path| (*path).to_owned()).collect(),
            subtree_files: 1,
            subtree_bytes,
        }
    }

    #[test]
    fn redundant_tree_bytes_count_nested_clone_sets_once() {
        let sets = vec![
            clone_set('a', &["/archive/A", "/archive/B"], 100),
            clone_set('b', &["/archive/A/documents", "/archive/B/documents"], 40),
        ];

        assert_eq!(
            sets.iter().map(TreeCloneSet::redundant_bytes).sum::<u64>(),
            140
        );
        assert_eq!(non_overlapping_redundant_bytes(&sets), 100);

        let mut reordered = sets.clone();
        reordered.reverse();
        for set in &mut reordered {
            set.folders.reverse();
        }
        assert_eq!(non_overlapping_redundant_bytes(&reordered), 100);
    }

    #[test]
    fn redundant_tree_bytes_sum_disjoint_clone_sets() {
        let sets = vec![
            clone_set('a', &["/archive/A", "/archive/B"], 100),
            clone_set('b', &["/archive/C", "/archive/D"], 50),
        ];

        assert_eq!(non_overlapping_redundant_bytes(&sets), 150);
    }

    #[test]
    fn redundant_tree_bytes_keep_only_uncovered_copies_in_larger_sets() {
        let sets = vec![
            clone_set('a', &["/archive/A", "/archive/B"], 100),
            clone_set(
                'b',
                &[
                    "/archive/A/documents",
                    "/archive/B/documents",
                    "/archive/C/documents",
                ],
                40,
            ),
        ];

        assert_eq!(
            sets.iter().map(TreeCloneSet::redundant_bytes).sum::<u64>(),
            180
        );
        assert_eq!(non_overlapping_redundant_bytes(&sets), 140);
    }

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

    fn pseudo_random_bytes(length: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    #[cfg(windows)]
    fn make_junction(link: &Path, target: &Path) -> bool {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        matches!(status, Ok(status) if status.success()) && link.exists()
    }

    fn assert_analysis_reports_unavailable(project_dir: &Path) {
        let reports: [DfResult<()>; 4] = [
            duplicate_report(project_dir).map(|_| ()),
            tree_clone_report(project_dir).map(|_| ()),
            tree_relation_report(project_dir).map(|_| ()),
            context_report(project_dir).map(|_| ()),
        ];
        for report in reports {
            assert!(
                matches!(&report, Err(DfError::Validation(message))
                    if message.contains("has not completed")
                        || message.contains("has no completed structural analysis")),
                "analysis report should be unavailable, got {report:?}"
            );
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

    /// Original P0 reproducer: the source spelling is separate lexically but
    /// resolves to the output directory. Project creation must reject it before
    /// staging a database or writing anything into the physical source.
    #[cfg(windows)]
    #[test]
    fn create_rejects_a_source_junction_to_the_output_root() {
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("datos");
        let source_alias = tmp.path().join("alias");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("origen.txt"), b"source bytes").unwrap();
        if !make_junction(&source_alias, &output) {
            eprintln!("SKIP: this environment cannot create junctions (mklink /J failed)");
            return;
        }

        let mut req = request(tmp.path());
        req.output_root = output.clone();
        req.source_roots = vec![source_alias];
        let error = create_project(&req, Actor::Test).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("reparse point") || message.contains("overlap physically")),
            "unexpected error: {error:?}"
        );
        assert!(!req.project_dir.exists(), "no project may be staged");
        assert_eq!(
            std::fs::read_dir(&output).unwrap().count(),
            1,
            "project creation wrote into the physical source"
        );
        assert_eq!(
            std::fs::read(output.join("origen.txt")).unwrap(),
            b"source bytes"
        );
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
    fn a_profile_typo_is_rejected_before_creating_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        let mut req = request(tmp.path());
        req.profile = Some("legla".to_string());

        let error = create_project(&req, Actor::Test).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("legla")),
            "unexpected error: {error:?}"
        );
        assert!(!req.project_dir.exists());
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
    fn opening_rejects_an_unknown_persisted_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();

        let db = Db::open(&req.project_dir.join(PROJECT_DB_RELATIVE)).unwrap();
        db.conn_for_tests()
            .execute("UPDATE projects SET profile = 'legla'", [])
            .unwrap();
        drop(db);

        let error = open_project(&req.project_dir).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("legla")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn scan_hash_analyze_and_reports_through_the_facade() {
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

        assert_analysis_reports_unavailable(&req.project_dir);

        let analysis = analyze_project(&req.project_dir, Actor::Test).expect("analyze");
        assert_eq!(analysis.state, "ANALYZED");

        let report = duplicate_report(&req.project_dir).expect("duplicates");
        assert_eq!(report.sets.len(), 1);
        assert_eq!(report.redundant_files, 1);
        assert_eq!(report.redundant_bytes, 9);
        tree_clone_report(&req.project_dir).expect("tree clones");
        tree_relation_report(&req.project_dir).expect("tree relations");
        context_report(&req.project_dir).expect("context");

        let status = project_status(&req.project_dir).expect("status");
        assert_eq!(status.state, "ANALYZED");
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
    fn similarity_is_available_through_facade_status_and_report() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        let first = pseudo_random_bytes(512 * 1024, 0x1234);
        let mut second = first.clone();
        second[220 * 1024..228 * 1024].fill(0x5A);
        std::fs::write(origin.join("informe-v1.bin"), &first).unwrap();
        std::fs::write(origin.join("informe-v2.bin"), &second).unwrap();

        let mut req = request(tmp.path());
        req.source_roots = vec![origin.clone()];
        create_project(&req, Actor::Test).unwrap();
        scan_project(&req.project_dir, Actor::Test).unwrap();
        hash_project(&req.project_dir, Actor::Test).unwrap();
        analyze_project(&req.project_dir, Actor::Test).unwrap();

        let options = SimilarityOptions {
            threshold: 0.25,
            min_shared_chunks: 1,
            min_shared_bytes: 8 * 1024,
            ..SimilarityOptions::default()
        };
        let outcome =
            analyze_similarity_with_options(&req.project_dir, Actor::Test, &options).unwrap();
        assert_eq!(outcome.status, "COMPLETED");
        assert_eq!(outcome.counters.relations_total, 1);

        let report = similarity_report(&req.project_dir).unwrap();
        assert!(report.evidence_only);
        assert_eq!(report.status.relationships.len(), 1);
        assert!(report.status.relationships[0].path_a.contains("informe-v"));

        let status = project_status(&req.project_dir).unwrap();
        let similarity = status.similarity.expect("M0.3 status is visible");
        assert_eq!(similarity.run_id, outcome.run_id);
        assert_eq!(similarity.relationships.len(), 1);

        // The complete read-only pipeline left the origin byte-for-byte intact.
        assert_eq!(std::fs::read(origin.join("informe-v1.bin")).unwrap(), first);
        assert_eq!(
            std::fs::read(origin.join("informe-v2.bin")).unwrap(),
            second
        );
    }

    #[test]
    fn completed_analysis_reports_remain_available_after_terminal_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("evidencia.txt"), b"contenido").unwrap();

        let mut req = request(tmp.path());
        req.source_roots = vec![origin];
        create_project(&req, Actor::Test).unwrap();
        scan_project(&req.project_dir, Actor::Test).unwrap();
        hash_project(&req.project_dir, Actor::Test).unwrap();
        analyze_project(&req.project_dir, Actor::Test).unwrap();

        let mut db = Db::open(&req.project_dir.join(PROJECT_DB_RELATIVE)).unwrap();
        repository::update_project_state(&mut db, ProjectState::Planning, Actor::Test).unwrap();
        repository::update_project_state(&mut db, ProjectState::Failed, Actor::Test).unwrap();
        drop(db);

        duplicate_report(&req.project_dir).expect("duplicate report after failure");
        tree_clone_report(&req.project_dir).expect("tree report after failure");
        structural_anomaly_report(&req.project_dir).expect("anomaly report after failure");
        structural_review_queue(&req.project_dir).expect("review queue after failure");
        assert_eq!(project_status(&req.project_dir).unwrap().state, "FAILED");
    }

    #[test]
    fn reports_stay_hidden_during_a_crashed_analysis_and_work_after_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("uno.txt"), b"contenido").unwrap();
        std::fs::write(origin.join("dos.txt"), b"contenido").unwrap();

        let mut req = request(tmp.path());
        req.source_roots = vec![origin];
        create_project(&req, Actor::Test).unwrap();
        scan_project(&req.project_dir, Actor::Test).unwrap();
        hash_project(&req.project_dir, Actor::Test).unwrap();

        // Simulate a process dying after ANALYZING and the first analysis
        // repository committed. No table or state is changed with raw SQL.
        let mut db = Db::open(&req.project_dir.join(PROJECT_DB_RELATIVE)).unwrap();
        let project = repository::load_project(&db).unwrap();
        let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .unwrap();
        repository::update_project_state(&mut db, ProjectState::Analyzing, Actor::Test).unwrap();
        df_db::plans::materialize_duplicate_sets(&mut db, project.id, snapshot.id, Actor::Test)
            .unwrap();
        drop(db);

        assert_analysis_reports_unavailable(&req.project_dir);

        let resumed = analyze_project(&req.project_dir, Actor::Test).unwrap();
        assert_eq!(resumed.state, "ANALYZED");
        duplicate_report(&req.project_dir).expect("duplicates after resume");
        tree_clone_report(&req.project_dir).expect("tree clones after resume");
        tree_relation_report(&req.project_dir).expect("tree relations after resume");
        context_report(&req.project_dir).expect("context after resume");
    }

    #[test]
    fn reports_require_final_stage_evidence_even_if_state_says_analyzed() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("uno.txt"), b"contenido").unwrap();

        let mut req = request(tmp.path());
        req.source_roots = vec![origin];
        create_project(&req, Actor::Test).unwrap();
        scan_project(&req.project_dir, Actor::Test).unwrap();
        hash_project(&req.project_dir, Actor::Test).unwrap();

        // Public state transitions alone cannot manufacture analysis evidence.
        let mut db = Db::open(&req.project_dir.join(PROJECT_DB_RELATIVE)).unwrap();
        repository::update_project_state(&mut db, ProjectState::Analyzing, Actor::Test).unwrap();
        repository::update_project_state(&mut db, ProjectState::Analyzed, Actor::Test).unwrap();
        drop(db);

        assert_analysis_reports_unavailable(&req.project_dir);
    }

    #[cfg(windows)] // drives execution, which is Windows-only for now
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

        let plan =
            create_plan(&req.project_dir, Actor::Test, DuplicatePolicy::ReportOnly).expect("plan");
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

    /// P1-2: a failed creation must leave nothing behind — no half-project, no
    /// staging litter — so retrying works.
    #[test]
    fn a_failed_creation_leaves_no_trace_and_retrying_works() {
        let tmp = tempfile::tempdir().unwrap();

        // Force a failure *after* validation: the project's parent is a file,
        // so the directory cannot exist (validation passes) but the staging
        // build cannot create anything either. `create_dir_all` would happily
        // invent a missing parent, so a merely-absent parent proves nothing.
        let blocker = tmp.path().join("soy-un-archivo");
        std::fs::write(&blocker, b"x").unwrap();
        let mut req = request(tmp.path());
        req.project_dir = blocker.join("proyecto");

        assert!(
            create_project(&req, Actor::Test).is_err(),
            "creation must fail when the project's parent is a file"
        );
        assert!(!req.project_dir.exists(), "no half-project may survive");

        // No staging directory survived next to the intended home.
        let litter: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".init-"))
            .collect();
        assert!(litter.is_empty(), "staging directories were left behind");

        // A valid retry then succeeds.
        let good = request(tmp.path());
        assert!(create_project(&good, Actor::Test).is_ok());
        assert!(open_project(&good.project_dir).is_ok());
    }

    /// P1-2: the marker appears only over a sound database — never before.
    #[test]
    fn the_marker_and_the_database_appear_together() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();
        // Both, or neither: a directory with a marker but no database (or the
        // reverse) is exactly what the staging dance prevents.
        assert!(req.project_dir.join(PROJECT_MARKER_FILE).is_file());
        assert!(req.project_dir.join(PROJECT_DB_RELATIVE).is_file());
        assert!(open_project(&req.project_dir).is_ok());
    }

    /// P1-2: an existing non-empty directory is never touched, let alone
    /// cleaned. The user's data outranks our convenience.
    #[test]
    fn a_preexisting_non_empty_directory_is_never_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        std::fs::create_dir_all(&req.project_dir).unwrap();
        let precious = req.project_dir.join("no-me-borres.txt");
        std::fs::write(&precious, b"datos del usuario").unwrap();

        assert!(create_project(&req, Actor::Test).is_err());
        // Still there, untouched.
        assert_eq!(std::fs::read(&precious).unwrap(), b"datos del usuario");
    }

    /// Rewrite one field of a project's marker, for tamper tests.
    fn tamper_marker(project_dir: &Path, field: &str, value: serde_json::Value) {
        let path = project_dir.join(PROJECT_MARKER_FILE);
        let mut marker: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        marker[field] = value;
        std::fs::write(&path, marker.to_string()).unwrap();
    }

    /// P1-3: a marker must never be able to point the engine at another
    /// database. On Windows `join` with an absolute path discards the base, so
    /// this is not theoretical.
    #[test]
    fn a_marker_cannot_redirect_the_database_outside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();

        for attempt in [
            "../../otro.sqlite",
            "..\\..\\otro.sqlite",
            "C:\\evil.sqlite",
            "/etc/passwd",
            "\\\\?\\C:\\evil.sqlite",
            "\\\\servidor\\share\\evil.sqlite",
            "state\\dataforge.sqlite", // ambiguous separator: not the constant
            "state/otro.sqlite",
            "",
        ] {
            tamper_marker(&req.project_dir, "database_path", attempt.into());
            let err = open_project(&req.project_dir).unwrap_err();
            assert!(
                matches!(err, DfError::Validation(_)),
                "`{attempt}` must be rejected, got {err:?}"
            );
        }
    }

    /// P1-3: a marker from a newer DataForge is refused with a clear message
    /// rather than opened on a guess.
    #[test]
    fn a_future_marker_version_is_rejected_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();

        tamper_marker(&req.project_dir, "schema_version", "2.0.0".into());
        let err = open_project(&req.project_dir).unwrap_err();
        match err {
            DfError::Validation(message) => {
                assert!(message.contains("newer DataForge"), "unclear: {message}");
                assert!(message.contains("Upgrade"), "no remedy offered: {message}");
            }
            other => panic!("expected a validation error, got {other:?}"),
        }

        // A newer minor is fine: minor/patch are additive.
        tamper_marker(&req.project_dir, "schema_version", "1.9.3".into());
        assert!(open_project(&req.project_dir).is_ok());

        // Garbage is rejected too.
        tamper_marker(
            &req.project_dir,
            "schema_version",
            "no-soy-una-version".into(),
        );
        assert!(open_project(&req.project_dir).is_err());
    }

    /// P1-3: opening must not silently create a database that is missing —
    /// `Connection::open` would happily do exactly that.
    #[test]
    fn opening_never_creates_a_missing_database() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();

        let db_path = req.project_dir.join(PROJECT_DB_RELATIVE);
        std::fs::remove_file(&db_path).unwrap();

        let err = open_project(&req.project_dir).unwrap_err();
        assert!(matches!(err, DfError::NotFound(_)), "{err:?}");
        assert!(
            !db_path.exists(),
            "opening must not conjure an empty database into existence"
        );
    }

    #[test]
    fn a_marker_with_a_non_uuid_project_id_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let req = request(tmp.path());
        create_project(&req, Actor::Test).unwrap();
        tamper_marker(&req.project_dir, "project_id", "no-soy-un-uuid".into());
        assert!(matches!(
            open_project(&req.project_dir).unwrap_err(),
            DfError::Validation(_)
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

    #[test]
    fn extraction_config_json_and_digest_are_the_same_canonical_bytes() {
        let limits = ExtractionLimits::default();
        let spec = extraction_spec(ProjectId::new(), SnapshotId::new(), &limits).unwrap();
        assert_eq!(spec.config_json, serde_json::to_string(&limits).unwrap());
        assert_eq!(spec.config_digest, limits.digest().unwrap());
        assert_eq!(
            spec.config_digest,
            hex::encode(Sha256::digest(spec.config_json.as_bytes()))
        );
    }

    #[test]
    fn explicit_pdf_worker_never_uses_a_relative_or_path_searched_name() {
        let options = ContentExtractionOptions {
            pdf_worker: Some(PathBuf::from("df-extract-worker.exe")),
            ..ContentExtractionOptions::default()
        };
        assert!(
            matches!(options.validate(), Err(DfError::Validation(message)) if message.contains("absolute") && message.contains("PATH"))
        );
    }

    #[test]
    fn explicit_query_worker_never_uses_a_relative_or_path_searched_name() {
        assert!(matches!(
            resolve_query_worker(Some(Path::new("df-query-worker.exe"))),
            Err(DfError::Validation(message))
                if message.contains("absolute") && message.contains("PATH")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn verified_source_checks_canonical_sha_and_never_needs_a_display_path() {
        let temp = tempfile::tempdir().unwrap();
        let relative = PathBuf::from("expediente.txt");
        let path = temp.path().join(&relative);
        std::fs::write(&path, b"canonical evidence").unwrap();
        let fingerprint = df_fs_safety::capture_fingerprint(&path).unwrap();
        let source = ExtractionContentSource {
            content_id: df_domain::ContentId::new(),
            size_bytes: 18,
            sha256: hex::encode(Sha256::digest(b"canonical evidence")),
            root_path: temp.path().to_path_buf(),
            raw_relative_path: Some(df_domain::RawPath::from_os_str(relative.as_os_str())),
            relative_path: "lossy display must not be opened.txt".to_string(),
            file_name: "expediente.txt".to_string(),
            fingerprint: fingerprint.token(),
            modified_at: None,
            reusable_representation_id: None,
        };
        let verified = read_verified_source(&source, 1024).unwrap();
        assert_eq!(verified.bytes, b"canonical evidence");
        assert_eq!(verified.sha256, source.sha256);

        std::fs::write(&path, b"different evidence").unwrap();
        assert!(matches!(
            read_verified_source(&source, 1024),
            Err(DfError::Conflict(_))
        ));
    }

    fn thread_message(id: &str, normalized_subject: &str, references: &[&str]) -> ThreadMessageRow {
        ThreadMessageRow {
            representation_id: RepresentationId::new(),
            message_id: Some(id.to_string()),
            in_reply_to: Vec::new(),
            references: references
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            sent_at: None,
            subject: None,
            normalized_subject: Some(normalized_subject.to_string()),
            body_sha256: None,
        }
    }

    #[test]
    fn mail_threading_prefers_explicit_references_over_subject_fallback() {
        let root = thread_message("<root@example>", "topic", &[]);
        let fallback = thread_message("<other@example>", "topic", &[]);
        let child = thread_message("<child@example>", "topic", &["root@example"]);
        let root_id = root.representation_id;
        let child_id = child.representation_id;
        let run_id = ExtractionRunId::new();
        let messages = [root, fallback, child];
        let threads = reconstruct_mail_threads(run_id, &messages);
        assert_eq!(threads.len(), 1);
        let child = threads[0]
            .members
            .iter()
            .find(|member| member.representation_id == child_id)
            .unwrap();
        assert_eq!(child.parent_representation_id, Some(root_id));
        assert_eq!(
            threads,
            reconstruct_mail_threads(run_id, &messages),
            "thread IDs and members must be deterministic across replay"
        );
    }

    #[cfg(windows)]
    #[test]
    fn content_intelligence_round_trips_through_the_public_facade() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(
            source.join("brief.txt"),
            b"alpha contract evidence for searchable content",
        )
        .unwrap();
        let mut create = request(temp.path());
        create.source_roots.push(source);
        create_project(&create, Actor::Test).unwrap();
        scan_project(&create.project_dir, Actor::Test).unwrap();
        hash_project(&create.project_dir, Actor::Test).unwrap();
        analyze_project(&create.project_dir, Actor::Test).unwrap();

        let extracted = extract_project_content(
            &create.project_dir,
            Actor::Test,
            &ContentExtractionOptions::default(),
        )
        .unwrap();
        assert_eq!(extracted.status, "COMPLETED");
        assert_eq!(extracted.counters.contents_total, 1);
        assert_eq!(extracted.counters.extracted, 1);

        let replay = extract_project_content(
            &create.project_dir,
            Actor::Test,
            &ContentExtractionOptions::default(),
        )
        .unwrap();
        assert_eq!(replay.run_id, extracted.run_id);
        assert_eq!(replay.processed_this_invocation, 0);

        let artifacts = build_content_artifacts(
            &create.project_dir,
            Some(&extracted.run_id),
            SearchBuildOptions::default(),
            SnapshotBuildOptions::default(),
            Actor::Test,
        )
        .unwrap();
        assert_eq!(artifacts.search_index.documents, 1);
        assert_eq!(artifacts.analytical_snapshot.rows, 1);

        let search = search_project_content(
            &create.project_dir,
            Some(&extracted.run_id),
            &SearchRequest {
                query: "alpha contract".to_string(),
                limit: 10,
                offset: 0,
                snippet_chars: 200,
            },
        )
        .unwrap();
        assert_eq!(search.hits.len(), 1);
        assert_eq!(search.hits[0].file_name, "brief.txt");
    }
}

/// Frozen contract inventory (M0.9, ADR-0037). Every versioned schema,
/// algorithm, ABI and the migration chain is pinned here: this test fails
/// if any of them changes. The policy is that a frozen version is bumped
/// with a new value and an ADR, never edited in place — so a change that
/// trips this test is either that deliberate bump (update the expectation
/// in the same commit as the ADR) or an accident to revert.
#[cfg(test)]
mod frozen_contracts {
    #[test]
    fn schema_algorithm_and_abi_versions_are_frozen() {
        // Persistence and profile contracts.
        assert_eq!(df_db::migrations::MIGRATIONS.len(), 19, "migration count");
        assert_eq!(df_db::migrations::MIGRATIONS[0].name, "foundation");
        assert_eq!(df_db::migrations::MIGRATIONS[18].name, "incremental_reuse");
        // Versions are unique and consecutive from 1.
        for (index, migration) in df_db::migrations::MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version, index as i64 + 1, "migration numbering");
        }
        assert_eq!(df_domain::PROFILE_SCHEMA, "dataforge.profile");
        assert_eq!(df_domain::PROFILE_SCHEMA_VERSION, "1.1.0");
        assert_eq!(super::MARKER_SCHEMA_VERSION, "1.0.0");

        // Similarity (M0.3).
        assert_eq!(
            df_similarity::ALGORITHM_FAMILY,
            "fastcdc-v2020-l1-minhash-v1"
        );

        // Content intelligence (M0.4).
        assert_eq!(df_extract::EXTRACTOR_VERSION, "0.2.0+content-v1");
        assert_eq!(df_search::SEARCH_SCHEMA_VERSION, "m0.4-tantivy-v1");
        assert_eq!(df_query::ANALYTICAL_SCHEMA_VERSION, "m0.4-parquet-v1");

        // Media intelligence (M0.5).
        assert_eq!(
            df_media::ANALYSIS_CONTRACT_VERSION,
            "dataforge.media-analysis.v1"
        );
        assert_eq!(df_media::IMAGE_ALGORITHM_VERSION, "dct-phash64-v1");
        assert_eq!(
            df_media::AUDIO_ALGORITHM_VERSION,
            "rusty-chromaprint-0.3.0-test2-v1"
        );
        assert_eq!(df_media::VIDEO_ALGORITHM_VERSION, "sampled-dct-phash64-v1");

        // Plugin ABI (M0.6).
        assert_eq!(df_plugin::HOST_ABI_VERSION, "0.1.0");
        assert_eq!(
            df_plugin::MANIFEST_SCHEMA_VERSION,
            "dataforge.plugin-manifest/0.1.0"
        );
        assert_eq!(
            df_plugin::INPUT_SCHEMA_VERSION,
            "dataforge.plugin-input/0.1.0"
        );
        assert_eq!(
            df_plugin::OUTPUT_SCHEMA_ID,
            "dataforge.plugin-findings/0.1.0"
        );

        // Assisted intelligence (M0.7).
        assert_eq!(df_ai::REQUEST_SCHEMA_VERSION, "dataforge.ai-request/0.7.0");
        assert_eq!(
            df_ai::DISCLOSURE_SCHEMA_VERSION,
            "dataforge.ai-disclosure/0.7.0"
        );
        assert_eq!(df_ai::AUDIT_SCHEMA_VERSION, "dataforge.ai-audit/0.7.0");
        assert_eq!(df_ai::OUTPUT_SCHEMA_ID, "dataforge.ai-suggestions/0.7.0");
        assert_eq!(
            df_ai::PROMPT_VERSION,
            "dataforge.assisted-intelligence-prompt/0.7.0"
        );
    }
}
