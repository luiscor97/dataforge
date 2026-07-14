//! Safe execution of approved plans (RFC-0001 §12.9, §27).
//!
//! Per-file protocol (§27.1): validate the source fingerprint, reserve the
//! destination, stream-copy into a partial file while hashing, flush,
//! compare against the expected content hashes, re-validate the source,
//! atomically finalize, record the result.
//!
//! Guarantees:
//! - the origin is only ever read (rule 1);
//! - nothing is overwritten (rule 3): finalize uses a rename that fails if
//!   the destination appeared meanwhile, and collisions resolve by
//!   `SKIP_REPRESENTED` (same hash) or a deterministic suffix (§27.3);
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

    let output_root = project.output_root.clone();
    std::fs::create_dir_all(&output_root).map_err(|e| DfError::io(&output_root, e))?;

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
            let outcome = run_operation(&output_root, &operation, options);
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
    output_root: &Path,
    operation: &ExecutableOperation,
    options: &ExecuteOptions,
) -> OperationOutcome {
    let started_at = chrono::Utc::now();
    let destination = output_root.join(&operation.destination_relative_path);

    let result = match operation.operation_type {
        OperationType::CreateDirectory => create_directory(&destination, operation),
        _ => copy_file(output_root, &destination, operation, options),
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
}

fn create_directory(
    destination: &Path,
    operation: &ExecutableOperation,
) -> Result<OperationOutcome, OperationFailure> {
    if destination.is_file() {
        return Err(OperationFailure::fatal(
            OperationErrorCode::DestinationChanged,
            format!(
                "destination `{}` exists and is a file, not a directory",
                destination.display()
            ),
        ));
    }
    std::fs::create_dir_all(destination)
        .map_err(|e| OperationFailure::from_io(&e, "creating directory"))?;
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
    output_root: &Path,
    planned_destination: &Path,
    operation: &ExecutableOperation,
    options: &ExecuteOptions,
) -> Result<OperationOutcome, OperationFailure> {
    let source = source_path(operation)?;
    let expected_sha256 = operation.expected_sha256.as_deref().ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "copy operation without an expected content hash",
        )
    })?;

    // 1. Validate the source fingerprint (§27.1).
    let pre = current_fingerprint(&source).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => OperationFailure::fatal(
            OperationErrorCode::SourceMissing,
            format!("source `{}` no longer exists", source.display()),
        ),
        _ => OperationFailure::from_io(&e, "reading source metadata"),
    })?;
    if Some(pre.as_str()) != operation.source_fingerprint.as_deref() {
        return Err(OperationFailure::fatal(
            OperationErrorCode::SourceChanged,
            "source changed since the snapshot was taken (RFC-0001 §27.5)",
        ));
    }

    // 2. Reserve the destination (§27.3): never overwrite.
    let (destination, skip) = resolve_collision(planned_destination, expected_sha256, options)?;
    if skip {
        return Ok(OperationOutcome {
            execution_state: ExecutionState::Completed,
            outcome: "SKIP_REPRESENTED".to_string(),
            error_code: None,
            detail: Some(
                "destination already holds identical content (RFC-0001 §27.3)".to_string(),
            ),
            final_relative_path: relative_to(output_root, &destination),
            bytes_copied: 0,
            sha256: Some(expected_sha256.to_string()),
            blake3: operation.expected_blake3.clone(),
            started_at: chrono::Utc::now(),
        });
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| OperationFailure::from_io(&e, "creating destination directory"))?;
    }

    // 3–5. Partial file, streamed copy with both hashes, flush (§27.1–27.2).
    let partial = partial_path(&destination, operation);
    let copy = stream_copy(&source, &partial, options.copy_buffer_bytes)
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
        Ok(post) if post == pre => {}
        _ => {
            remove_own_partial(&partial);
            return Err(OperationFailure::fatal(
                OperationErrorCode::SourceChanged,
                "source changed while copying (RFC-0001 §27.5)",
            ));
        }
    }

    // 8. Atomic finalize: rename fails if the destination appeared meanwhile.
    if destination.exists() {
        remove_own_partial(&partial);
        return Err(OperationFailure::fatal(
            OperationErrorCode::DestinationChanged,
            format!(
                "destination `{}` appeared during the copy",
                destination.display()
            ),
        ));
    }
    if let Err(error) = std::fs::rename(&partial, &destination) {
        remove_own_partial(&partial);
        return Err(OperationFailure::from_io(&error, "finalizing copy"));
    }

    Ok(OperationOutcome {
        execution_state: ExecutionState::Completed,
        outcome: "COPIED".to_string(),
        error_code: None,
        detail: None,
        final_relative_path: relative_to(output_root, &destination),
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
    let relative = operation.source_relative_path.as_deref().ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "copy operation without a source path",
        )
    })?;
    Ok(extend_long_path(root.join(relative)))
}

/// §27.3: destination exists with the same hash → skip as represented;
/// different hash → deterministic suffix; never overwrite.
fn resolve_collision(
    planned: &Path,
    expected_sha256: &str,
    options: &ExecuteOptions,
) -> Result<(PathBuf, bool), OperationFailure> {
    if !planned.exists() {
        return Ok((planned.to_path_buf(), false));
    }
    let existing_sha = hash_existing(planned, options.copy_buffer_bytes)
        .map_err(|e| OperationFailure::from_io(&e, "hashing existing destination"))?;
    if existing_sha == expected_sha256 {
        return Ok((planned.to_path_buf(), true));
    }
    let suffixed = suffixed_path(planned, expected_sha256);
    if !suffixed.exists() {
        return Ok((suffixed, false));
    }
    let suffixed_sha = hash_existing(&suffixed, options.copy_buffer_bytes)
        .map_err(|e| OperationFailure::from_io(&e, "hashing existing suffixed destination"))?;
    if suffixed_sha == expected_sha256 {
        return Ok((suffixed, true));
    }
    Err(OperationFailure::fatal(
        OperationErrorCode::DestinationChanged,
        format!(
            "both `{}` and its deterministic suffix exist with different content",
            planned.display()
        ),
    ))
}

/// `.<name>.dataforge-partial-<operation-id>` next to the destination (§27.2).
fn partial_path(destination: &Path, operation: &ExecutableOperation) -> PathBuf {
    let name = destination
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "artefact".to_string());
    destination.with_file_name(format!(
        ".{name}.dataforge-partial-{}",
        operation.operation_id
    ))
}

/// Deterministic suffix before the extension (§27.3), matching the planner.
fn suffixed_path(destination: &Path, sha256: &str) -> PathBuf {
    let tag = &sha256[..8.min(sha256.len())];
    let stem = destination
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match destination.extension() {
        Some(ext) => format!("{stem}~df-{tag}.{}", ext.to_string_lossy()),
        None => format!("{stem}~df-{tag}"),
    };
    destination.with_file_name(name)
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

fn stream_copy(
    source: &Path,
    partial: &Path,
    buffer_bytes: usize,
) -> std::io::Result<StreamedCopy> {
    let mut reader = std::fs::File::open(source)?;
    let mut writer = std::fs::File::create(partial)?;
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

fn current_fingerprint(path: &Path) -> std::io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(FileFingerprint {
        size_bytes: metadata.len(),
        modified_at_fs: metadata.modified().ok().map(Into::into),
    }
    .token())
}

fn relative_to(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
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
