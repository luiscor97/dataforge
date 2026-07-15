//! Safe execution of approved plans (RFC-0001 §12.9, §27).
//!
//! Per-file protocol (§27.1): validate the source fingerprint, reserve the
//! destination, stream-copy into a partial file while hashing, flush,
//! compare against the expected content hashes, re-validate the source,
//! atomically finalize, record the result.
//!
//! Guarantees:
//! - the origin is only ever read (rule 1);
//! - every write goes through `df-fs-safety` (ADR-0017): the output root is
//!   validated and physically identified, and no destination is ever reached
//!   through a symlink, junction or mount point;
//! - nothing is overwritten (rule 3): finalize uses a platform primitive that
//!   *refuses* to replace (ADR-0021) rather than a racy `exists()` check, and
//!   collisions resolve by `SKIP_REPRESENTED` (same hash) or a deterministic
//!   suffix (§27.3);
//! - every attempt is journaled append-only (`operation_results`);
//! - a killed run resumes: `RUNNING` and `FAILED_RETRYABLE` operations are
//!   picked up again, `COMPLETED` ones are never re-executed (§27.4);
//! - the only files ever removed are this executor's own partial files
//!   from a failed or interrupted attempt — never user data.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use df_db::plans::{self, ExecutableOperation, OperationOutcome};
use df_db::{repository, Db};
use df_domain::{
    Actor, ExecutionState, FileFingerprint, OperationErrorCode, OperationType, ProjectState,
};
use df_error::{DfError, DfResult};
use df_fs_safety::{FsSafetyError, SafeOutputRoot, SafeRelativePath};
use serde::Serialize;
use sha2::Digest;

/// Tuning knobs of one execution run.
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    /// Bytes per I/O call while streaming a copy.
    pub copy_buffer_bytes: usize,
    /// Operations fetched from the plan per round trip.
    pub operation_batch: u32,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            copy_buffer_bytes: 1024 * 1024,
            operation_batch: 256,
        }
    }
}

/// Result of one execution run.
#[derive(Debug, Clone, Serialize)]
pub struct ExecuteOutcome {
    pub plan_id: String,
    pub completed: u64,
    pub failed_retryable: u64,
    pub failed_final: u64,
    pub pending: u64,
    pub bytes_copied: u64,
    pub cancelled: bool,
    /// Project state after the run: `EXECUTED` when every operation reached
    /// a terminal state, `EXECUTION_PAUSED` when work remains.
    pub state: String,
}

/// Execute the approved plan; resumable and cancellable.
pub fn execute_plan(
    db: &mut Db,
    actor: Actor,
    options: &ExecuteOptions,
    cancel: Option<&AtomicBool>,
) -> DfResult<ExecuteOutcome> {
    if options.copy_buffer_bytes == 0 || options.operation_batch == 0 {
        return Err(DfError::Validation(
            "copy_buffer_bytes and operation_batch must be at least 1".to_string(),
        ));
    }

    let project = repository::load_project(db)?;
    match project.state {
        ProjectState::PlanApproved | ProjectState::ExecutionPaused => {}
        other => {
            return Err(DfError::Validation(format!(
                "cannot execute a project in state {other} \
                 (expected PLAN_APPROVED or EXECUTION_PAUSED)"
            )));
        }
    }
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;
    if plan.status != df_domain::PlanStatus::Approved {
        return Err(DfError::Validation(format!(
            "the current plan is {}, not APPROVED",
            plan.status.as_str()
        )));
    }

    // The output root is validated and physically identified before a single
    // byte is written; on a platform without a safe implementation this errors
    // out instead of executing unprotected (ADR-0017).
    let output_root = project.output_root.clone();
    let safe_root = SafeOutputRoot::validate(&output_root)?;

    repository::update_project_state(db, ProjectState::Executing, actor)?;

    let mut attempted: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut bytes_copied: u64 = 0;
    let mut cancelled = false;
    'run: loop {
        let batch = plans::executable_operations(db, plan.id, options.operation_batch)?;
        // Operations already attempted in this run stay for the *next* run.
        let fresh: Vec<ExecutableOperation> = batch
            .into_iter()
            .filter(|op| !attempted.contains(&op.operation_id.to_string()))
            .collect();
        if fresh.is_empty() {
            break;
        }
        for operation in fresh {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                cancelled = true;
                break 'run;
            }
            attempted.insert(operation.operation_id.to_string());
            plans::mark_operation_running(db, operation.operation_id)?;
            let outcome = run_operation(&safe_root, &operation, options);
            bytes_copied += outcome.bytes_copied;
            plans::record_operation_outcome(db, operation.operation_id, &outcome)?;
        }
    }

    let progress = plans::plan_progress(db, plan.id)?;
    let all_terminal =
        progress.pending == 0 && progress.running == 0 && progress.failed_retryable == 0;
    let (event_type, next_state) = if cancelled || !all_terminal {
        (plans::EVENT_EXECUTION_PAUSED, ProjectState::ExecutionPaused)
    } else {
        (plans::EVENT_EXECUTION_COMPLETED, ProjectState::Executed)
    };
    let payload = serde_json::json!({
        "plan_id": plan.id.to_string(),
        "completed": progress.completed,
        "failed_retryable": progress.failed_retryable,
        "failed_final": progress.failed_final,
        "pending": progress.pending,
        "bytes_copied": bytes_copied,
        "cancelled": cancelled,
    });
    plans::emit_event(db, project.id, event_type, &payload, actor)?;
    let project = repository::update_project_state(db, next_state, actor)?;

    Ok(ExecuteOutcome {
        plan_id: plan.id.to_string(),
        completed: progress.completed,
        failed_retryable: progress.failed_retryable,
        failed_final: progress.failed_final,
        pending: progress.pending + progress.running,
        bytes_copied,
        cancelled,
        state: project.state.as_str().to_string(),
    })
}

/// Execute one operation against the filesystem. Never returns `Err`: every
/// failure becomes a journaled outcome (§27.5).
fn run_operation(
    safe_root: &SafeOutputRoot,
    operation: &ExecutableOperation,
    options: &ExecuteOptions,
) -> OperationOutcome {
    let started_at = chrono::Utc::now();

    // The destination is only ever a validated relative path resolved through
    // the safe boundary; the raw string from the plan never becomes a path by
    // itself (ADR-0017).
    let result = match SafeRelativePath::parse(Path::new(&operation.destination_relative_path)) {
        Ok(relative) => match operation.operation_type {
            OperationType::CreateDirectory => create_directory(safe_root, &relative, operation),
            _ => copy_file(safe_root, &relative, operation, options),
        },
        Err(error) => Err(OperationFailure::from_fs_safety(error)),
    };

    match result {
        Ok(mut outcome) => {
            outcome.started_at = started_at;
            outcome
        }
        Err(failure) => OperationOutcome {
            execution_state: failure.state,
            outcome: failure.code.as_str().to_string(),
            error_code: Some(failure.code),
            detail: Some(failure.detail),
            final_relative_path: None,
            bytes_copied: 0,
            sha256: None,
            blake3: None,
            started_at,
        },
    }
}

struct OperationFailure {
    code: OperationErrorCode,
    state: ExecutionState,
    detail: String,
}

impl OperationFailure {
    fn from_io(error: &std::io::Error, context: &str) -> Self {
        let (code, state) = match error.kind() {
            std::io::ErrorKind::NotFound => (
                OperationErrorCode::SourceMissing,
                ExecutionState::FailedFinal,
            ),
            std::io::ErrorKind::PermissionDenied => (
                OperationErrorCode::PermissionDenied,
                ExecutionState::FailedFinal,
            ),
            std::io::ErrorKind::StorageFull => {
                (OperationErrorCode::NoSpace, ExecutionState::FailedRetryable)
            }
            _ => (OperationErrorCode::IoError, ExecutionState::FailedRetryable),
        };
        Self {
            code,
            state,
            detail: format!("{context}: {error}"),
        }
    }

    fn fatal(code: OperationErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            state: ExecutionState::FailedFinal,
            detail: detail.into(),
        }
    }

    /// Map a filesystem-safety refusal onto a journaled outcome.
    ///
    /// A link, an escape or an identity change is a *final* failure: retrying
    /// would just hit the same booby-trapped tree, and it needs a human to
    /// look at the output. A destination that appeared is also final for this
    /// operation (§27.3 resolves it by suffix on the next plan, not by
    /// retrying blindly). Only plain I/O keeps its own retryable semantics.
    fn from_fs_safety(error: FsSafetyError) -> Self {
        match error {
            FsSafetyError::ReparsePoint { .. }
            | FsSafetyError::OutsideOutputRoot { .. }
            | FsSafetyError::OutputRootIdentityChanged { .. } => Self {
                code: OperationErrorCode::InvalidPath,
                state: ExecutionState::FailedFinal,
                detail: error.to_string(),
            },
            FsSafetyError::DestinationExists { .. } => Self {
                code: OperationErrorCode::DestinationChanged,
                state: ExecutionState::FailedFinal,
                detail: error.to_string(),
            },
            FsSafetyError::InvalidRelativePath { .. }
            | FsSafetyError::UnsupportedPlatform { .. } => Self {
                code: OperationErrorCode::InvalidPath,
                state: ExecutionState::FailedFinal,
                detail: error.to_string(),
            },
            FsSafetyError::Io { ref source, .. } => {
                let mut failure = Self::from_io(source, "filesystem safety");
                failure.detail = error.to_string();
                failure
            }
        }
    }
}

fn create_directory(
    safe_root: &SafeOutputRoot,
    relative: &SafeRelativePath,
    operation: &ExecutableOperation,
) -> Result<OperationOutcome, OperationFailure> {
    // Checks every level as it creates it, and refuses to walk through a
    // reparse point (ADR-0017) — unlike create_dir_all, which happily does.
    safe_root
        .create_directory_secure(relative)
        .map_err(OperationFailure::from_fs_safety)?;
    Ok(OperationOutcome {
        execution_state: ExecutionState::Completed,
        outcome: "DIRECTORY_CREATED".to_string(),
        error_code: None,
        detail: None,
        final_relative_path: Some(operation.destination_relative_path.clone()),
        bytes_copied: 0,
        sha256: None,
        blake3: None,
        started_at: chrono::Utc::now(),
    })
}

fn copy_file(
    safe_root: &SafeOutputRoot,
    planned_relative: &SafeRelativePath,
    operation: &ExecutableOperation,
    options: &ExecuteOptions,
) -> Result<OperationOutcome, OperationFailure> {
    // Resolving proves the planned destination is reachable without crossing a
    // single link, and re-checks the output root's physical identity.
    let planned_destination = safe_root
        .resolve_destination_without_following_links(planned_relative)
        .map_err(OperationFailure::from_fs_safety)?
        .absolute()
        .to_path_buf();
    let planned_destination = planned_destination.as_path();
    let source = source_path(operation)?;
    let expected_sha256 = operation.expected_sha256.as_deref().ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "copy operation without an expected content hash",
        )
    })?;

    // 1. Validate the source against the fingerprint frozen in the manifest
    // (§27.1). Parsed, not string-compared: a v1 token from an older snapshot
    // must not masquerade as a v2 match (ADR-0019).
    let pre = current_fingerprint(&source)?;
    let approved = operation
        .source_fingerprint
        .as_deref()
        .map(FileFingerprint::parse)
        .transpose()
        .map_err(|error| {
            OperationFailure::fatal(
                OperationErrorCode::InvalidPath,
                format!("unreadable approved fingerprint: {error}"),
            )
        })?
        .ok_or_else(|| {
            OperationFailure::fatal(
                OperationErrorCode::InvalidPath,
                "copy operation without an approved source fingerprint",
            )
        })?;
    if FileFingerprint::compare(&approved, &pre).is_changed() {
        return Err(OperationFailure::fatal(
            OperationErrorCode::SourceChanged,
            "source changed since the snapshot was taken (RFC-0001 §27.5)",
        ));
    }

    // 2. Reserve the destination (§27.3): never overwrite.
    let (relative, skip) = resolve_collision(
        safe_root,
        planned_relative,
        planned_destination,
        expected_sha256,
        options,
    )?;
    let destination = safe_root
        .resolve_destination_without_following_links(&relative)
        .map_err(OperationFailure::from_fs_safety)?
        .absolute()
        .to_path_buf();
    if skip {
        return Ok(OperationOutcome {
            execution_state: ExecutionState::Completed,
            outcome: "SKIP_REPRESENTED".to_string(),
            error_code: None,
            detail: Some(
                "destination already holds identical content (RFC-0001 §27.3)".to_string(),
            ),
            final_relative_path: Some(relative.to_path().to_string_lossy().into_owned()),
            bytes_copied: 0,
            sha256: Some(expected_sha256.to_string()),
            blake3: operation.expected_blake3.clone(),
            started_at: chrono::Utc::now(),
        });
    }

    if let Some(parent) = relative.parent() {
        safe_root
            .create_directory_secure(&parent)
            .map_err(OperationFailure::from_fs_safety)?;
    }

    // 3–5. Partial file, streamed copy with both hashes, flush (§27.1–27.2).
    // The partial is created through the safe boundary with create_new, so it
    // can neither follow a link nor reuse someone else's file.
    let partial_relative = relative
        .with_file_name(&partial_file_name(&relative, operation))
        .map_err(OperationFailure::from_fs_safety)?;
    let partial = safe_root
        .resolve_destination_without_following_links(&partial_relative)
        .map_err(OperationFailure::from_fs_safety)?
        .absolute()
        .to_path_buf();
    let handle = safe_root
        .create_partial_secure(&partial_relative)
        .map_err(OperationFailure::from_fs_safety)?;
    let copy = stream_copy(&source, handle, options.copy_buffer_bytes)
        .map_err(|e| OperationFailure::from_io(&e, "copying"));
    let copy = match copy {
        Ok(copy) => copy,
        Err(failure) => {
            remove_own_partial(&partial);
            return Err(failure);
        }
    };

    // 6. Compare against the identity recorded at hash time (§27.1).
    if copy.sha256 != expected_sha256 {
        remove_own_partial(&partial);
        return Err(OperationFailure::fatal(
            OperationErrorCode::HashMismatch,
            format!(
                "copied bytes hash to {} but the snapshot recorded {expected_sha256}",
                copy.sha256
            ),
        ));
    }

    // 7. The source must not have changed while we read it (§14.5).
    match current_fingerprint(&source) {
        Ok(post) if !FileFingerprint::compare(&pre, &post).is_changed() => {}
        _ => {
            remove_own_partial(&partial);
            return Err(OperationFailure::fatal(
                OperationErrorCode::SourceChanged,
                "source changed while copying (RFC-0001 §27.5)",
            ));
        }
    }

    // 8. Finalize. The no-overwrite guarantee comes from the platform, not
    // from a prior exists() check, which would be a race (ADR-0021): if the
    // destination appeared during the copy, the kernel itself refuses.
    if let Err(error) = df_fs_safety::finalize_no_replace(&partial, &destination) {
        remove_own_partial(&partial);
        return Err(OperationFailure::from_fs_safety(error));
    }

    Ok(OperationOutcome {
        execution_state: ExecutionState::Completed,
        outcome: "COPIED".to_string(),
        error_code: None,
        detail: None,
        // The relative path is the one we resolved and proved safe, not a
        // string re-derived from the absolute path.
        final_relative_path: Some(relative.to_path().to_string_lossy().into_owned()),
        bytes_copied: copy.bytes,
        sha256: Some(copy.sha256),
        blake3: Some(copy.blake3),
        started_at: chrono::Utc::now(),
    })
}

fn source_path(operation: &ExecutableOperation) -> Result<PathBuf, OperationFailure> {
    let root = operation.source_root_path.as_ref().ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "copy operation without a source root",
        )
    })?;
    // The raw form is authoritative (ADR-0020): the display string may have
    // been damaged by `to_string_lossy`, and a U+FFFD is a real character — it
    // would name a different file, or none. Only fall back to the display
    // string for snapshots taken before v0.1.1, which have no raw form.
    let relative = match operation.source_raw_relative_path.as_ref() {
        Some(raw) => PathBuf::from(raw.to_os_string()),
        None => {
            let display = operation.source_relative_path.as_deref().ok_or_else(|| {
                OperationFailure::fatal(
                    OperationErrorCode::InvalidPath,
                    "copy operation without a source path",
                )
            })?;
            PathBuf::from(display)
        }
    };
    Ok(extend_long_path(root.join(relative)))
}

/// §27.3: destination exists with the same hash → skip as represented;
/// different hash → deterministic suffix; never overwrite.
/// Decide the final destination when something already sits there (§27.3).
///
/// Works on *relative* paths so every candidate is re-resolved through the
/// safe boundary; the absolute form is only used to read what is already there.
fn resolve_collision(
    safe_root: &SafeOutputRoot,
    planned_relative: &SafeRelativePath,
    planned_absolute: &Path,
    expected_sha256: &str,
    options: &ExecuteOptions,
) -> Result<(SafeRelativePath, bool), OperationFailure> {
    if !planned_absolute.exists() {
        return Ok((planned_relative.clone(), false));
    }
    let existing_sha = hash_existing(planned_absolute, options.copy_buffer_bytes)
        .map_err(|e| OperationFailure::from_io(&e, "hashing existing destination"))?;
    if existing_sha == expected_sha256 {
        return Ok((planned_relative.clone(), true));
    }
    let suffixed_relative = planned_relative
        .with_file_name(&suffixed_file_name(planned_relative, expected_sha256))
        .map_err(OperationFailure::from_fs_safety)?;
    let suffixed_absolute = safe_root
        .resolve_destination_without_following_links(&suffixed_relative)
        .map_err(OperationFailure::from_fs_safety)?
        .absolute()
        .to_path_buf();
    if !suffixed_absolute.exists() {
        return Ok((suffixed_relative, false));
    }
    let suffixed_sha = hash_existing(&suffixed_absolute, options.copy_buffer_bytes)
        .map_err(|e| OperationFailure::from_io(&e, "hashing existing suffixed destination"))?;
    if suffixed_sha == expected_sha256 {
        return Ok((suffixed_relative, true));
    }
    Err(OperationFailure::fatal(
        OperationErrorCode::DestinationChanged,
        format!(
            "both `{}` and its deterministic suffix exist with different content",
            planned_absolute.display()
        ),
    ))
}

/// `.<name>.dataforge-partial-<operation-id>` next to the destination (§27.2).
fn partial_file_name(relative: &SafeRelativePath, operation: &ExecutableOperation) -> String {
    format!(
        ".{}.dataforge-partial-{}",
        relative.file_name(),
        operation.operation_id
    )
}

/// Deterministic suffix before the extension (§27.3), matching the planner.
fn suffixed_file_name(relative: &SafeRelativePath, sha256: &str) -> String {
    let tag = &sha256[..8.min(sha256.len())];
    let name = relative.file_name();
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}~df-{tag}.{ext}"),
        _ => format!("{name}~df-{tag}"),
    }
}

/// Remove a partial file this run created. Own artefacts only — the path
/// always comes from [`partial_path`].
fn remove_own_partial(partial: &Path) {
    let _ = std::fs::remove_file(partial);
}

struct StreamedCopy {
    bytes: u64,
    sha256: String,
    blake3: String,
}

/// Stream the source into an already-opened partial, hashing as we go.
///
/// The writer arrives from `df-fs-safety::create_partial_secure`, so this
/// function never opens a destination path itself — that is the whole point of
/// the boundary (ADR-0017). The source is opened read-only (rule 1).
fn stream_copy(
    source: &Path,
    mut writer: std::fs::File,
    buffer_bytes: usize,
) -> std::io::Result<StreamedCopy> {
    let mut reader = std::fs::File::open(source)?;
    let mut sha = sha2::Sha256::new();
    let mut blake = blake3::Hasher::new();
    let mut buffer = vec![0u8; buffer_bytes];
    let mut bytes: u64 = 0;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        sha.update(&buffer[..read]);
        blake.update(&buffer[..read]);
        bytes += read as u64;
    }
    // Flush to stable storage before the atomic rename (§27.1).
    writer.sync_all()?;
    Ok(StreamedCopy {
        bytes,
        sha256: hex::encode(sha.finalize()),
        blake3: blake.finalize().to_hex().to_string(),
    })
}

fn hash_existing(path: &Path, buffer_bytes: usize) -> std::io::Result<String> {
    let mut reader = std::fs::File::open(path)?;
    let mut sha = sha2::Sha256::new();
    let mut buffer = vec![0u8; buffer_bytes];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha.update(&buffer[..read]);
    }
    Ok(hex::encode(sha.finalize()))
}

/// Current v2 fingerprint of the source (ADR-0019).
fn current_fingerprint(path: &Path) -> Result<FileFingerprint, OperationFailure> {
    df_fs_safety::capture_fingerprint(path).map_err(|error| match &error {
        FsSafetyError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            OperationFailure::fatal(
                OperationErrorCode::SourceMissing,
                format!("source `{}` no longer exists", path.display()),
            )
        }
        _ => OperationFailure::from_fs_safety(error),
    })
}

#[cfg(windows)]
fn extend_long_path(path: PathBuf) -> PathBuf {
    const LEGACY_MAX_PATH: usize = 260;
    let text = path.as_os_str().to_string_lossy();
    if text.len() >= LEGACY_MAX_PATH && !text.starts_with(r"\\") {
        return PathBuf::from(format!(r"\\?\{text}"));
    }
    path
}

#[cfg(not(windows))]
fn extend_long_path(path: PathBuf) -> PathBuf {
    path
}

#[cfg(test)]
mod tests {
    use df_domain::{ProfileRef, Project, SourceRoot};
    use df_hash::{hash_project, HashOptions};
    use df_planner::{analyze_project, approve_plan, create_plan};
    use df_scan::{scan_project, ScanOptions};

    use super::*;

    struct Fixture {
        db: Db,
        origin: PathBuf,
        output: PathBuf,
    }

    fn approved_project(tmp: &Path) -> Fixture {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(origin.join("sub")).unwrap();
        std::fs::create_dir_all(origin.join("vacía")).unwrap();
        std::fs::write(origin.join("a.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("sub").join("b.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("c.txt"), b"different").unwrap();

        let output = tmp.join("salida");
        let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Prueba execute",
            ProfileRef::default(),
            output.clone(),
            tmp.join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin.clone())];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        analyze_project(&mut db, Actor::Test).unwrap();
        create_plan(&mut db, Actor::Test).unwrap();
        approve_plan(&mut db, Actor::Test).unwrap();
        Fixture { db, origin, output }
    }

    fn no_partials_left(root: &Path) -> bool {
        let mut queue = vec![root.to_path_buf()];
        while let Some(dir) = queue.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                if entry.metadata().unwrap().is_dir() {
                    queue.push(entry.path());
                } else if entry
                    .file_name()
                    .to_string_lossy()
                    .contains("dataforge-partial")
                {
                    return false;
                }
            }
        }
        true
    }

    /// Create a directory junction with `mklink /J`. Returns false when the
    /// environment forbids it, so a test can skip *loudly* (the encargo
    /// forbids silent passes).
    #[cfg(windows)]
    fn make_junction(link: &Path, target: &Path) -> bool {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        matches!(status, Ok(s) if s.success()) && link.exists()
    }

    /// Threat T1: a junction planted inside the output must not redirect a
    /// single byte outside it (ADR-0017).
    #[cfg(windows)]
    #[test]
    fn a_junction_inside_the_output_never_redirects_a_write() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());

        // The plan writes into `salida\origen\...`. Plant `salida\origen` as a
        // junction to somewhere else entirely, exactly like inherited material.
        let outside = tmp.path().join("fuera");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&fx.output).unwrap();
        let planted = fx.output.join("origen");
        if !make_junction(&planted, &outside) {
            eprintln!("SKIP: this environment cannot create junctions (mklink /J failed)");
            return;
        }

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        // Every operation that had to go through the junction fails, typed.
        assert!(outcome.failed_final > 0, "the junction must be refused");
        assert_eq!(
            outcome.completed, 0,
            "nothing may be written through a link"
        );

        // The point of the whole exercise: nothing landed outside the output.
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            0,
            "a write escaped the output root through the junction"
        );

        // The origin is untouched (rule 1).
        assert_eq!(
            std::fs::read(fx.origin.join("a.txt")).unwrap(),
            b"same bytes"
        );

        // The run ends FAILED, not silently "done": the refusal is journaled
        // (its typed code is covered by `fs_safety_refusals_map_to_typed_final_failures`).
        assert!(!fx.output.join("origen").join("a.txt").exists());
    }

    /// Threat T5 / P0-3: after approval, the live inventory is evidence, not
    /// the contract. Editing `content_objects` must not change one byte of
    /// what the executor does.
    #[test]
    fn editing_content_objects_after_approval_does_not_change_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());

        // Forge one content identity in the live table (only one: sha256 is
        // uniquely indexed, so forging them all to the same value would fail
        // for an uninteresting reason). Before ADR-0018 the executor read
        // expected_sha256 straight from here, so this would have made that copy
        // fail with HASH_MISMATCH — the tables dictated the run and the plan
        // hash never noticed.
        let changed = fx
            .db
            .conn_for_tests()
            .execute(
                "UPDATE content_objects SET sha256 = ?1
                 WHERE id = (SELECT id FROM content_objects WHERE sha256 IS NOT NULL LIMIT 1)",
                [&"e".repeat(64)],
            )
            .unwrap();
        assert!(changed > 0, "the test must actually tamper with something");

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        // The run is unaffected: it executed the frozen manifest.
        assert_eq!(outcome.state, "EXECUTED");
        assert_eq!(outcome.failed_final, 0, "the forged table changed the run");
        assert_eq!(outcome.completed, 6);
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("a.txt")).unwrap(),
            b"same bytes"
        );
    }

    /// Threat T5 / P0-3: same for the source location. Repointing a source
    /// root after approval must not redirect what gets read.
    #[test]
    fn repointing_a_source_root_after_approval_does_not_change_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());

        // A decoy tree with different content at the same relative paths.
        let decoy = tmp.path().join("señuelo");
        std::fs::create_dir_all(decoy.join("sub")).unwrap();
        std::fs::write(decoy.join("a.txt"), b"CONTENIDO FALSO").unwrap();

        let changed = fx
            .db
            .conn_for_tests()
            .execute(
                "UPDATE source_roots SET absolute_path = ?1",
                [decoy.to_string_lossy().as_ref()],
            )
            .unwrap();
        assert!(changed > 0, "the test must actually tamper with something");

        execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        // The copy came from the approved root, not the decoy.
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("a.txt")).unwrap(),
            b"same bytes",
            "the executor followed the repointed live table instead of the manifest"
        );
    }

    /// P0-5: the executor reopens the source from the raw path, not from the
    /// lossy display string. Uses names that survive display but exercise the
    /// whole raw pipeline end to end.
    #[test]
    fn sources_are_reopened_through_their_raw_path() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        // Names that a lossy round-trip could plausibly mangle.
        std::fs::write(origin.join("acta ñ 文件 🎉.txt"), b"unicode").unwrap();
        std::fs::write(origin.join("normal.txt"), b"plain").unwrap();

        let output = tmp.path().join("salida");
        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = df_domain::Project::new(
            "Raw paths",
            df_domain::ProfileRef::default(),
            output.clone(),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![df_domain::SourceRoot::new(project.id, origin.clone())];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        df_scan::scan_project(&mut db, Actor::Test, &df_scan::ScanOptions::default(), None)
            .unwrap();
        df_hash::hash_project(&mut db, Actor::Test, &df_hash::HashOptions::default(), None)
            .unwrap();
        df_planner::analyze_project(&mut db, Actor::Test).unwrap();
        df_planner::create_plan(&mut db, Actor::Test).unwrap();
        df_planner::approve_plan(&mut db, Actor::Test).unwrap();

        // The approved manifest must carry the raw path (§P0-5), not just the
        // display string.
        let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
        let manifest = plans::manifest(&db, plan.id).unwrap();
        let unicode_entry = manifest
            .iter()
            .find(|e| {
                e.source_relative_path_exact
                    .as_deref()
                    .is_some_and(|p| p.contains("acta"))
            })
            .expect("the unicode file is in the manifest");
        let raw = unicode_entry
            .source_raw_relative_path
            .as_ref()
            .expect("the manifest must freeze the raw path");
        assert_eq!(raw.display(), "acta ñ 文件 🎉.txt");
        assert!(!raw.is_lossy());

        let outcome = execute_plan(&mut db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.failed_final, 0, "raw-path copies must not fail");

        // The bytes arrived, so the source really was reopened.
        assert_eq!(
            std::fs::read(output.join("origen").join("acta ñ 文件 🎉.txt")).unwrap(),
            b"unicode"
        );
    }

    /// The encargo requires the refusal to be *typed*, not a generic I/O blob.
    #[test]
    fn fs_safety_refusals_map_to_typed_final_failures() {
        let reparse = OperationFailure::from_fs_safety(FsSafetyError::ReparsePoint {
            path: PathBuf::from("D:/salida/origen"),
        });
        assert_eq!(reparse.code, OperationErrorCode::InvalidPath);
        assert_eq!(reparse.state, ExecutionState::FailedFinal);

        let escape = OperationFailure::from_fs_safety(FsSafetyError::OutsideOutputRoot {
            resolved: PathBuf::from("C:/fuera/x"),
            root: PathBuf::from("D:/salida"),
        });
        assert_eq!(escape.code, OperationErrorCode::InvalidPath);
        assert_eq!(escape.state, ExecutionState::FailedFinal);

        let exists = OperationFailure::from_fs_safety(FsSafetyError::DestinationExists {
            path: PathBuf::from("D:/salida/x.txt"),
        });
        assert_eq!(exists.code, OperationErrorCode::DestinationChanged);
        assert_eq!(exists.state, ExecutionState::FailedFinal);

        let swapped = OperationFailure::from_fs_safety(FsSafetyError::OutputRootIdentityChanged {
            root: PathBuf::from("D:/salida"),
        });
        assert_eq!(swapped.code, OperationErrorCode::InvalidPath);
        assert_eq!(swapped.state, ExecutionState::FailedFinal);
    }

    /// Threat T4: a destination that appears before the finalize must never be
    /// overwritten — the guarantee comes from the platform (ADR-0021).
    #[test]
    fn a_preexisting_destination_with_other_content_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());

        // Someone else's file already sits exactly where a copy is planned.
        std::fs::create_dir_all(fx.output.join("origen")).unwrap();
        let squatted = fx.output.join("origen").join("a.txt");
        std::fs::write(&squatted, b"ajeno, no tocar").unwrap();

        execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        // Untouched, byte for byte: the copy went to a suffixed name instead
        // (§27.3) or failed, but it never replaced this.
        assert_eq!(std::fs::read(&squatted).unwrap(), b"ajeno, no tocar");
        assert!(no_partials_left(&fx.output));
    }

    #[test]
    fn execute_produces_a_verified_mirror_without_touching_the_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "EXECUTED");
        assert!(!outcome.cancelled);
        assert_eq!(outcome.failed_retryable, 0);
        assert_eq!(outcome.failed_final, 0);
        assert_eq!(outcome.pending, 0);
        // 3 copies + 3 directories (origen, origen\sub, origen\vacía).
        assert_eq!(outcome.completed, 6);

        let copied = fx.output.join("origen").join("a.txt");
        assert_eq!(std::fs::read(&copied).unwrap(), b"same bytes");
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("sub").join("b.txt")).unwrap(),
            b"same bytes"
        );
        assert!(fx.output.join("origen").join("vacía").is_dir());
        assert!(no_partials_left(&fx.output));

        // Origin untouched: same 4 top-level entries, same content.
        assert_eq!(
            std::fs::read(fx.origin.join("a.txt")).unwrap(),
            b"same bytes"
        );
        assert_eq!(std::fs::read_dir(&fx.origin).unwrap().count(), 4);
    }

    #[test]
    fn execution_is_resumable_after_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());

        let cancel = AtomicBool::new(true);
        let outcome = execute_plan(
            &mut fx.db,
            Actor::Test,
            &ExecuteOptions::default(),
            Some(&cancel),
        )
        .unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.state, "EXECUTION_PAUSED");
        assert_eq!(outcome.completed, 0);

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "EXECUTED");
        assert_eq!(outcome.completed, 6);
    }

    #[test]
    fn completed_operations_never_rerun() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        // A finished project rejects further execution by state machine.
        let err =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[test]
    fn identical_preexisting_destination_is_skip_represented() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        std::fs::create_dir_all(fx.output.join("origen")).unwrap();
        std::fs::write(fx.output.join("origen").join("c.txt"), b"different").unwrap();

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "EXECUTED");
        assert_eq!(outcome.failed_final, 0);
        // The pre-existing identical file was not rewritten (rule 3): its
        // content is intact and the run copied fewer bytes than the total.
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("c.txt")).unwrap(),
            b"different"
        );
    }

    #[test]
    fn conflicting_preexisting_destination_gets_a_deterministic_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        std::fs::create_dir_all(fx.output.join("origen")).unwrap();
        std::fs::write(fx.output.join("origen").join("c.txt"), b"other content").unwrap();

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "EXECUTED");
        assert_eq!(outcome.failed_final, 0);
        // The pre-existing file is untouched (rule 3)…
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("c.txt")).unwrap(),
            b"other content"
        );
        // …and the copy landed under the deterministic suffix.
        let sha = hex::encode(sha2::Sha256::digest(b"different"));
        let suffixed = fx
            .output
            .join("origen")
            .join(format!("c~df-{}.txt", &sha[..8]));
        assert_eq!(std::fs::read(&suffixed).unwrap(), b"different");
    }

    #[test]
    fn a_source_changed_after_approval_fails_that_operation_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        std::fs::write(fx.origin.join("c.txt"), b"changed after approval!").unwrap();

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.failed_final, 1);
        assert_eq!(outcome.completed, 5);
        // All operations terminal → EXECUTED; verification will judge it.
        assert_eq!(outcome.state, "EXECUTED");
        assert!(!fx.output.join("origen").join("c.txt").exists());
        assert!(no_partials_left(&fx.output));
    }

    #[test]
    fn execution_events_land_in_a_valid_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        let project = repository::load_project(&fx.db).unwrap();
        let events = repository::list_events(&fx.db, project.id).unwrap();
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"PLAN_APPROVED"));
        assert!(types.contains(&"EXECUTION_COMPLETED"));
        df_ledger::verify_chain(&events).expect("ledger stays valid");
    }
}
