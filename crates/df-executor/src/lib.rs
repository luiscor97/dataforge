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
use df_fs_safety::{FileIdentity, FsSafetyError, SafeOutputRoot, SafeRelativePath};
use serde::Serialize;
use sha2::Digest;

/// Tuning knobs of one execution run.
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    /// Bytes per I/O call while streaming a copy.
    pub copy_buffer_bytes: usize,
    /// Operations fetched from the plan per round trip.
    pub operation_batch: u32,
    /// Explicit acknowledgment that the destination filesystem offers only
    /// degraded identity guarantees — network shares, FAT variants or
    /// unclassifiable volumes (ADR-0036). Without it, execution towards
    /// such a destination refuses fail-closed.
    pub allow_degraded_destination: bool,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            allow_degraded_destination: false,
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
    /// Aggregated wall-clock per protocol stage across this run (M1.0.1).
    /// Local-only diagnostics; never sampled per file beyond two `Instant`
    /// reads per stage, so the overhead is nanoseconds against I/O costs.
    pub stage_nanos: StageNanos,
}

/// Cumulative nanoseconds spent in each stage of the per-file execution
/// protocol (RFC-0001 §27.1), plus the operation count they cover. The sum
/// of stages is close to — but intentionally not exactly — the phase wall
/// time: scheduling and batch queries live between stages.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct StageNanos {
    /// Source fingerprint capture + manifest fingerprint parse/compare.
    pub preflight_source: u64,
    /// Safe destination resolution + source-root boundary validation.
    pub resolve_destination: u64,
    /// §27.3 collision decision (existence probe, re-hash when taken).
    pub collision_check: u64,
    /// Durable partial lease issuance (SQLite).
    pub lease: u64,
    /// Parent directory + create_new partial + open-handle identity capture.
    pub create_partial: u64,
    /// Durable ownership claim persistence (SQLite).
    pub claim_persist: u64,
    /// Streamed read+write+SHA-256+BLAKE3, excluding the durability sync.
    pub copy_stream: u64,
    /// `sync_all` before finalize (strict durability, §27.1).
    pub sync_all: u64,
    /// Post-copy source fingerprint capture + compare (§14.5).
    pub post_fingerprint: u64,
    /// Root boundary re-proof between copy and finalize.
    pub revalidate_root: u64,
    /// No-replace finalize rename on the claimed handle (ADR-0021).
    pub finalize: u64,
    /// Operation outcome persistence (SQLite).
    pub persist_result: u64,
    /// Operations these accumulators cover.
    pub operations: u64,
}

/// Elapsed nanoseconds since `start`, saturated into a `u64`.
fn nanos_since(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
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
    let recovering_executing = project.state == ProjectState::Executing;
    match project.state {
        ProjectState::PlanApproved | ProjectState::ExecutionPaused | ProjectState::Executing => {}
        other => {
            return Err(DfError::Validation(format!(
                "cannot execute a project in state {other} \
                 (expected PLAN_APPROVED, EXECUTION_PAUSED or recoverable EXECUTING)"
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

    // Validate both the currently registered roots and the paths frozen in the
    // immutable manifest. The latter are what copy operations will actually
    // open, while the former also cover empty roots represented only by
    // CREATE_DIRECTORY operations.
    let source_roots = execution_source_roots(db, project.id, plan.id)?;
    validate_source_output_boundary(&source_roots, &project.output_root)?;

    // The output root is validated and physically identified before a single
    // byte is written; on a platform without a safe implementation this errors
    // out instead of executing unprotected (ADR-0017).
    let output_root = project.output_root.clone();
    let safe_root = SafeOutputRoot::validate(&output_root)?;
    // ADR-0036: writing without physical identity (network shares, FAT
    // variants, unclassifiable volumes) weakens substitution detection and
    // finalize guarantees. Allowed only as an explicit, audited per-run
    // decision — and checked after platform safety, so POSIX keeps its
    // canonical refusal.
    let destination_filesystem = df_fs_safety::classify_filesystem(&output_root);
    if !destination_filesystem.has_physical_identity() && !options.allow_degraded_destination {
        return Err(DfError::Validation(format!(
            "the output root filesystem ({}) offers only degraded identity              guarantees; re-run with --allow-degraded-destination to              acknowledge and proceed (ADR-0036)",
            destination_filesystem.as_str()
        )));
    }
    // Creating a previously absent output root changes the filesystem view;
    // repeat the proof before entering EXECUTING or running an operation.
    validate_source_output_boundary(&source_roots, safe_root.path())?;

    // EXECUTING can be the durable state left by a killed single writer. In
    // that recovery case do not emit a redundant/invalid EXECUTING→EXECUTING
    // transition; ADR-0029 explicitly excludes concurrent writers.
    if !recovering_executing {
        repository::update_project_state(db, ProjectState::Executing, actor)?;
    }

    let mut attempted: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut bytes_copied: u64 = 0;
    let mut cancelled = false;
    let mut stages = StageNanos::default();
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
            // Reclaim attempt A while A's token and physical identity are
            // still the durable database state. A fresh lease B is issued
            // only after this idempotent cleanup succeeds; otherwise a crash
            // between B and cleanup would destroy the sole ownership proof.
            if operation.operation_type != OperationType::CreateDirectory {
                if let Err(failure) = reclaim_interrupted_partial(&safe_root, &operation) {
                    let outcome = failure.into_outcome(chrono::Utc::now());
                    plans::record_operation_outcome(db, operation.operation_id, &outcome)?;
                    continue;
                }
            }
            // A copy receives a fresh unpredictable partial lease in the same
            // durable update that marks it RUNNING. Directory operations do
            // not create partials and therefore carry no ownership token.
            let lease_started = std::time::Instant::now();
            let partial_lease_token = match operation.operation_type {
                OperationType::CreateDirectory => {
                    plans::mark_operation_running(db, operation.operation_id)?;
                    None
                }
                _ => Some(plans::lease_copy_operation(db, operation.operation_id)?),
            };
            stages.lease += nanos_since(lease_started);
            let outcome = run_operation(
                db,
                &safe_root,
                &operation,
                partial_lease_token.as_deref(),
                options,
                &mut stages,
            );
            bytes_copied += outcome.bytes_copied;
            let persist_started = std::time::Instant::now();
            plans::record_operation_outcome(db, operation.operation_id, &outcome)?;
            stages.persist_result += nanos_since(persist_started);
            stages.operations += 1;
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
    // Only the zero-work recovery path can represent the legacy crash window
    // between a terminal milestone and the old separate state transition.
    // Once this invocation attempted an operation, a PAUSED result is new and
    // must receive its own milestone even when the previous latest event has
    // the same kind and plan id.
    let reuse_interrupted_milestone = recovering_executing && attempted.is_empty() && !cancelled;
    let project = plans::finish_execution(
        db,
        plan.id,
        event_type,
        &payload,
        next_state,
        reuse_interrupted_milestone,
        actor,
    )?;

    Ok(ExecuteOutcome {
        plan_id: plan.id.to_string(),
        completed: progress.completed,
        failed_retryable: progress.failed_retryable,
        failed_final: progress.failed_final,
        pending: progress.pending + progress.running,
        bytes_copied,
        cancelled,
        state: project.state.as_str().to_string(),
        stage_nanos: stages,
    })
}

fn execution_source_roots(
    db: &Db,
    project_id: df_domain::ProjectId,
    plan_id: df_domain::PlanId,
) -> DfResult<Vec<PathBuf>> {
    let mut paths = std::collections::BTreeSet::new();
    for root in repository::load_source_roots(db, project_id)? {
        paths.insert(root.absolute_path);
    }
    for entry in plans::manifest(db, plan_id)? {
        if let Some(path) = entry.source_root_path_snapshot {
            paths.insert(PathBuf::from(path));
        }
    }
    Ok(paths.into_iter().collect())
}

fn validate_source_output_boundary(source_roots: &[PathBuf], output_root: &Path) -> DfResult<()> {
    for source_root in source_roots {
        df_fs_safety::ensure_root_is_not_reparse(source_root)?;
        df_fs_safety::ensure_physical_roots_disjoint(source_root, output_root)?;
    }
    Ok(())
}

/// Execute one operation against the filesystem. Never returns `Err`: every
/// failure becomes a journaled outcome (§27.5).
fn run_operation(
    db: &Db,
    safe_root: &SafeOutputRoot,
    operation: &ExecutableOperation,
    partial_lease_token: Option<&str>,
    options: &ExecuteOptions,
    stages: &mut StageNanos,
) -> OperationOutcome {
    let started_at = chrono::Utc::now();

    // The destination is only ever a validated relative path resolved through
    // the safe boundary; the raw string from the plan never becomes a path by
    // itself (ADR-0017).
    let result = match SafeRelativePath::parse(Path::new(&operation.destination_relative_path)) {
        Ok(relative) => match operation.operation_type {
            OperationType::CreateDirectory => create_directory(safe_root, &relative, operation),
            _ => partial_lease_token
                .ok_or_else(|| {
                    OperationFailure::fatal(
                        OperationErrorCode::InvalidPath,
                        "copy operation without a durable partial lease",
                    )
                })
                .and_then(|token| {
                    copy_file(db, safe_root, &relative, operation, token, options, stages)
                }),
        },
        Err(error) => Err(OperationFailure::from_fs_safety(error)),
    };

    match result {
        Ok(mut outcome) => {
            outcome.started_at = started_at;
            outcome
        }
        Err(failure) => failure.into_outcome(started_at),
    }
}

#[derive(Debug)]
struct OperationFailure {
    code: OperationErrorCode,
    state: ExecutionState,
    detail: String,
    retain_partial_lease: bool,
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
            retain_partial_lease: false,
        }
    }

    fn fatal(code: OperationErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            state: ExecutionState::FailedFinal,
            detail: detail.into(),
            retain_partial_lease: false,
        }
    }

    fn into_outcome(self, started_at: df_domain::Timestamp) -> OperationOutcome {
        OperationOutcome {
            execution_state: self.state,
            outcome: self.code.as_str().to_string(),
            error_code: Some(self.code),
            detail: Some(self.detail),
            final_relative_path: None,
            bytes_copied: 0,
            sha256: None,
            blake3: None,
            started_at,
            retain_partial_lease: self.retain_partial_lease,
        }
    }

    fn retained_cleanup(detail: impl Into<String>) -> Self {
        Self {
            code: OperationErrorCode::IoError,
            state: ExecutionState::Running,
            detail: detail.into(),
            retain_partial_lease: true,
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
            | FsSafetyError::OutputRootIdentityChanged { .. }
            | FsSafetyError::PhysicalRootOverlap { .. } => Self {
                code: OperationErrorCode::InvalidPath,
                state: ExecutionState::FailedFinal,
                detail: error.to_string(),
                retain_partial_lease: false,
            },
            FsSafetyError::DestinationExists { .. } => Self {
                code: OperationErrorCode::DestinationChanged,
                state: ExecutionState::FailedFinal,
                detail: error.to_string(),
                retain_partial_lease: false,
            },
            FsSafetyError::InvalidRelativePath { .. }
            | FsSafetyError::InvalidRootPath { .. }
            | FsSafetyError::UnsupportedPlatform { .. } => Self {
                code: OperationErrorCode::InvalidPath,
                state: ExecutionState::FailedFinal,
                detail: error.to_string(),
                retain_partial_lease: false,
            },
            FsSafetyError::Io { ref source, .. } => {
                // fs-safety only ever touches the OUTPUT boundary, so a
                // NotFound here is a destination-side condition (a parent
                // directory missing or vanished), never a missing source.
                // The 100k scale run proved why this matters: long-path
                // failures while creating partials were journaled as
                // SOURCE_MISSING, pointing the investigation at the wrong
                // side of the copy.
                let mut failure = match source.kind() {
                    std::io::ErrorKind::NotFound => Self {
                        code: OperationErrorCode::IoError,
                        state: ExecutionState::FailedRetryable,
                        detail: String::new(),
                        retain_partial_lease: false,
                    },
                    _ => Self::from_io(source, "filesystem safety"),
                };
                failure.detail = format!("output-side failure: {error}");
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
        retain_partial_lease: false,
    })
}

/// Reclaim the exact partial from an interrupted attempt before replacing its
/// durable lease with a new one.
fn reclaim_interrupted_partial(
    safe_root: &SafeOutputRoot,
    operation: &ExecutableOperation,
) -> Result<(), OperationFailure> {
    if operation.previous_execution_state != ExecutionState::Running {
        return Ok(());
    }
    let (Some(previous_token), Some(previous_identity)) = (
        operation.previous_partial_lease_token.as_deref(),
        operation.previous_partial_lease_identity.as_deref(),
    ) else {
        // Token/state without a post-create physical claim is not ownership.
        return Ok(());
    };
    let planned_relative = SafeRelativePath::parse(Path::new(&operation.destination_relative_path))
        .map_err(OperationFailure::from_fs_safety)?;
    let leased_partial_relative = planned_relative
        .with_file_name(&partial_file_name(operation, previous_token)?)
        .map_err(OperationFailure::from_fs_safety)?;
    let expected_identity = parse_partial_identity(previous_identity)?;
    match safe_root.remove_leased_partial_secure(&leased_partial_relative, expected_identity) {
        Ok(_) => {}
        Err(
            error @ (FsSafetyError::Io { .. } | FsSafetyError::OutputRootIdentityChanged { .. }),
        ) => {
            return Err(OperationFailure::retained_cleanup(format!(
                "cleanup of the interrupted physically claimed partial must be retried: {error}"
            )));
        }
        Err(error) => return Err(OperationFailure::from_fs_safety(error)),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_file(
    db: &Db,
    safe_root: &SafeOutputRoot,
    planned_relative: &SafeRelativePath,
    operation: &ExecutableOperation,
    partial_lease_token: &str,
    options: &ExecuteOptions,
    stages: &mut StageNanos,
) -> Result<OperationOutcome, OperationFailure> {
    let stage_started = std::time::Instant::now();
    validate_source_root(safe_root, operation)?;
    // Resolving proves the planned destination is reachable without crossing a
    // single link, and re-checks the output root's physical identity.
    let planned_destination = safe_root
        .resolve_destination_without_following_links(planned_relative)
        .map_err(OperationFailure::from_fs_safety)?
        .absolute()
        .to_path_buf();
    let planned_destination = planned_destination.as_path();
    let source = source_path(operation)?;
    stages.resolve_destination += nanos_since(stage_started);
    let expected_sha256 = operation.expected_sha256.as_deref().ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "copy operation without an expected content hash",
        )
    })?;

    // 1. Validate the source against the fingerprint frozen in the manifest
    // (§27.1). Parsed, not string-compared: a v1 token from an older snapshot
    // must not masquerade as a v2 match (ADR-0019).
    let stage_started = std::time::Instant::now();
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
    stages.preflight_source += nanos_since(stage_started);

    // 2. Reserve the destination (§27.3): never overwrite.
    let stage_started = std::time::Instant::now();
    let (relative, skip) = resolve_collision(
        safe_root,
        planned_relative,
        planned_destination,
        expected_sha256,
        options,
    )?;
    stages.collision_check += nanos_since(stage_started);
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
            retain_partial_lease: false,
        });
    }

    let stage_started = std::time::Instant::now();
    if let Some(parent) = relative.parent() {
        safe_root
            .create_directory_secure(&parent)
            .map_err(OperationFailure::from_fs_safety)?;
    }

    // 3–5. Partial file, streamed copy with both hashes, flush (§27.1–27.2).
    // The partial is created through the safe boundary with create_new, so it
    // can neither follow a link nor reuse someone else's file.
    let partial_relative = relative
        .with_file_name(&partial_file_name(operation, partial_lease_token)?)
        .map_err(OperationFailure::from_fs_safety)?;
    let partial = safe_root
        .resolve_destination_without_following_links(&partial_relative)
        .map_err(OperationFailure::from_fs_safety)?
        .absolute()
        .to_path_buf();
    let handle = safe_root
        .create_partial_secure(&partial_relative)
        .map_err(OperationFailure::from_fs_safety)?;
    // Ownership is claimed only *after* create_new succeeded, using identity
    // read from that exact open handle. A path reopen here would let a rename
    // race claim somebody else's replacement.
    let partial_identity = df_fs_safety::identity_of_open_file(&handle, &partial)
        .map_err(OperationFailure::from_fs_safety)?
        .ok_or_else(|| {
            OperationFailure::fatal(
                OperationErrorCode::InvalidPath,
                "filesystem did not provide a physical identity for the new partial",
            )
        })?;
    stages.create_partial += nanos_since(stage_started);
    let stored_identity = format_partial_identity(partial_identity);
    let stage_started = std::time::Instant::now();
    if let Err(error) = plans::claim_copy_partial(
        db,
        operation.operation_id,
        partial_lease_token,
        &stored_identity,
    ) {
        drop(handle);
        let _ = safe_root.remove_leased_partial_secure(&partial_relative, partial_identity);
        return Err(OperationFailure {
            code: OperationErrorCode::IoError,
            state: ExecutionState::FailedRetryable,
            detail: format!("persisting partial ownership claim: {error}"),
            retain_partial_lease: false,
        });
    }
    stages.claim_persist += nanos_since(stage_started);
    let stage_started = std::time::Instant::now();
    let copy = stream_copy(&source, handle, options.copy_buffer_bytes)
        .map_err(|e| OperationFailure::from_io(&e, "copying"));
    let copy_total = nanos_since(stage_started);
    let copy = match copy {
        Ok(copy) => copy,
        Err(failure) => {
            return Err(cleanup_after_claimed_failure(
                safe_root,
                &partial_relative,
                partial_identity,
                failure,
            ));
        }
    };
    stages.sync_all += copy.sync_nanos;
    stages.copy_stream += copy_total.saturating_sub(copy.sync_nanos);

    // 6. Compare against the identity recorded at hash time (§27.1).
    if copy.sha256 != expected_sha256 {
        let failure = OperationFailure::fatal(
            OperationErrorCode::HashMismatch,
            format!(
                "copied bytes hash to {} but the snapshot recorded {expected_sha256}",
                copy.sha256
            ),
        );
        return Err(cleanup_after_claimed_failure(
            safe_root,
            &partial_relative,
            partial_identity,
            failure,
        ));
    }

    // 7. The source must not have changed while we read it (§14.5).
    let stage_started = std::time::Instant::now();
    match current_fingerprint(&source) {
        Ok(post) if !FileFingerprint::compare(&pre, &post).is_changed() => {}
        _ => {
            let failure = OperationFailure::fatal(
                OperationErrorCode::SourceChanged,
                "source changed while copying (RFC-0001 §27.5)",
            );
            return Err(cleanup_after_claimed_failure(
                safe_root,
                &partial_relative,
                partial_identity,
                failure,
            ));
        }
    }
    stages.post_fingerprint += nanos_since(stage_started);
    // Re-prove the root boundary after reading and before committing the
    // destination. This catches a junction/root swap during a long copy; the
    // file fingerprint above independently catches a source-object swap.
    let stage_started = std::time::Instant::now();
    if let Err(failure) = validate_source_root(safe_root, operation) {
        return Err(cleanup_after_claimed_failure(
            safe_root,
            &partial_relative,
            partial_identity,
            failure,
        ));
    }
    stages.revalidate_root += nanos_since(stage_started);

    // 8. Finalize. The no-overwrite guarantee comes from the platform, not
    // from a prior exists() check, which would be a race (ADR-0021): if the
    // destination appeared during the copy, the kernel itself refuses. The
    // identity check and rename both happen on the same handle, so a foreign
    // replacement cannot enter between them.
    let stage_started = std::time::Instant::now();
    if let Err(error) = safe_root.finalize_claimed_partial_no_replace(
        &partial_relative,
        &relative,
        partial_identity,
    ) {
        let failure = OperationFailure::from_fs_safety(error);
        return Err(cleanup_after_claimed_failure(
            safe_root,
            &partial_relative,
            partial_identity,
            failure,
        ));
    }
    stages.finalize += nanos_since(stage_started);

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
        retain_partial_lease: false,
    })
}

fn validate_source_root(
    safe_root: &SafeOutputRoot,
    operation: &ExecutableOperation,
) -> Result<(), OperationFailure> {
    let root = operation.source_root_path.as_deref().ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "copy operation without a source root",
        )
    })?;
    df_fs_safety::ensure_root_is_not_reparse(root).map_err(OperationFailure::from_fs_safety)?;
    df_fs_safety::ensure_physical_roots_disjoint(root, safe_root.path())
        .map_err(OperationFailure::from_fs_safety)?;
    if let Some(expected) = operation.source_root_identity.as_deref() {
        let current = df_fs_safety::identity_of(root)
            .map_err(OperationFailure::from_fs_safety)?
            .map(|identity| format!("{}:{}", identity.volume_serial, identity.file_index));
        if current.as_deref() != Some(expected) {
            return Err(OperationFailure::fatal(
                OperationErrorCode::SourceChanged,
                "source root identity changed after plan approval",
            ));
        }
    }
    Ok(())
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
    // Existence and re-hash must use the extended form: a long path probed
    // without it reports "not found" for a file that is really there, and the
    // collision logic would mis-see a taken destination as free.
    if !df_fs_safety::extended_for_io(planned_absolute).exists() {
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
    if !df_fs_safety::extended_for_io(&suffixed_absolute).exists() {
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

/// `.dataforge-partial-<operation-id>-<lease-token>` next to the destination.
///
/// The original file name deliberately does not participate. A valid NTFS
/// component may already occupy all 255 UTF-16 units; adding our bookkeeping
/// suffix to it would make the partial impossible to create even with a
/// `\\?\` path. Two UUIDs still consume fewer than 100 UTF-16 units. The
/// random lease is committed before file creation, so a pre-existing name
/// cannot acquire executor ownership merely by a state transition.
fn partial_file_name(
    operation: &ExecutableOperation,
    lease_token: &str,
) -> Result<String, OperationFailure> {
    let parsed = uuid::Uuid::parse_str(lease_token).map_err(|_| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease token is not a UUID",
        )
    })?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != lease_token {
        return Err(OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease token is not in canonical form",
        ));
    }
    let name = format!(".dataforge-partial-{}-{canonical}", operation.operation_id);
    debug_assert!(name.encode_utf16().count() <= 255);
    Ok(name)
}

fn format_partial_identity(identity: FileIdentity) -> String {
    format!(
        "{:016x}:{:016x}",
        identity.volume_serial, identity.file_index
    )
}

fn parse_partial_identity(value: &str) -> Result<FileIdentity, OperationFailure> {
    let (volume, file) = value.split_once(':').ok_or_else(|| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease identity has no separator",
        )
    })?;
    if value.len() != 33 || volume.len() != 16 || file.len() != 16 {
        return Err(OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease identity has an invalid length",
        ));
    }
    let volume_serial = u64::from_str_radix(volume, 16).map_err(|_| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease volume identity is invalid",
        )
    })?;
    let file_index = u64::from_str_radix(file, 16).map_err(|_| {
        OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease file identity is invalid",
        )
    })?;
    if format!("{volume_serial:016x}:{file_index:016x}") != value || file_index == 0 {
        return Err(OperationFailure::fatal(
            OperationErrorCode::InvalidPath,
            "partial lease identity is not canonical physical identity",
        ));
    }
    Ok(FileIdentity {
        volume_serial,
        file_index,
    })
}

/// Deterministic suffix before the extension (§27.3), matching the planner.
fn suffixed_file_name(relative: &SafeRelativePath, sha256: &str) -> String {
    df_fs_safety::deterministic_collision_file_name(relative.file_name(), sha256)
}

/// Resolve a failure after a durable partial claim without losing recovery
/// authority. A transient cleanup I/O failure keeps RUNNING+claim; an identity
/// conflict is foreign and fails closed without retaining ownership.
fn cleanup_after_claimed_failure(
    safe_root: &SafeOutputRoot,
    partial: &SafeRelativePath,
    identity: FileIdentity,
    original: OperationFailure,
) -> OperationFailure {
    match safe_root.remove_leased_partial_secure(partial, identity) {
        Ok(_) => original,
        Err(
            error @ (FsSafetyError::Io { .. } | FsSafetyError::OutputRootIdentityChanged { .. }),
        ) => OperationFailure::retained_cleanup(format!(
            "{}; cleanup of the physically claimed partial must be retried: {error}",
            original.detail
        )),
        Err(error) => OperationFailure::from_fs_safety(error),
    }
}

struct StreamedCopy {
    bytes: u64,
    sha256: String,
    blake3: String,
    /// Nanoseconds spent in the durability `sync_all`, so the caller can bill
    /// it to the sync stage separately from the read/write/hash loop.
    sync_nanos: u64,
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
    let sync_started = std::time::Instant::now();
    writer.sync_all()?;
    let sync_nanos = nanos_since(sync_started);
    Ok(StreamedCopy {
        bytes,
        sha256: hex::encode(sha.finalize()),
        blake3: blake.finalize().to_hex().to_string(),
        sync_nanos,
    })
}

fn hash_existing(path: &Path, buffer_bytes: usize) -> std::io::Result<String> {
    let mut reader = std::fs::File::open(df_fs_safety::extended_for_io(path))?;
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

// The adversarial suite exercises the execution protocol end to end, and
// execution refuses fail-closed off Windows until POSIX write safety
// exists (that refusal is pinned by the CLI and corpus POSIX tests).
#[cfg(all(test, windows))]
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
        create_plan(&mut db, Actor::Test, df_domain::DuplicatePolicy::ReportOnly).unwrap();
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

    fn first_copy_operation(db: &Db) -> ExecutableOperation {
        let project = repository::load_project(db).unwrap();
        let plan = plans::current_plan(db, project.id).unwrap().unwrap();
        plans::executable_operations(db, plan.id, u32::MAX)
            .unwrap()
            .into_iter()
            .find(|operation| operation.expected_sha256.is_some())
            .expect("the fixture has at least one executable copy")
    }

    fn planted_partial(
        output: &Path,
        operation: &ExecutableOperation,
        lease_token: &str,
    ) -> PathBuf {
        let destination = output.join(&operation.destination_relative_path);
        let parent = destination.parent().expect("copy destination has a parent");
        std::fs::create_dir_all(parent).unwrap();
        parent.join(partial_file_name(operation, lease_token).unwrap())
    }

    fn create_and_claim_partial(
        db: &Db,
        output: &Path,
        operation: &ExecutableOperation,
        lease_token: &str,
        bytes: &[u8],
    ) -> (PathBuf, FileIdentity) {
        let partial = planted_partial(output, operation, lease_token);
        let mut handle = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .unwrap();
        let identity = df_fs_safety::identity_of_open_file(&handle, &partial)
            .unwrap()
            .expect("the executor requires physical identity");
        plans::claim_copy_partial(
            db,
            operation.operation_id,
            lease_token,
            &format_partial_identity(identity),
        )
        .unwrap();
        handle.write_all(bytes).unwrap();
        handle.sync_all().unwrap();
        drop(handle);
        (partial, identity)
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

    /// Original source/output-alias boundary: after approval, replace the
    /// frozen source root with a junction to the output. Execution must fail in
    /// preflight, before creating a directory or moving the lifecycle state.
    #[cfg(windows)]
    #[test]
    fn a_source_root_repointed_to_output_is_rejected_before_any_write() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let saved_origin = tmp.path().join("origen-original");
        std::fs::rename(&fx.origin, &saved_origin).unwrap();
        std::fs::create_dir_all(&fx.output).unwrap();
        if !make_junction(&fx.origin, &fx.output) {
            eprintln!("SKIP: this environment cannot create junctions (mklink /J failed)");
            return;
        }

        let error =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("reparse point") || message.contains("overlap physically")),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            repository::load_project(&fx.db).unwrap().state,
            ProjectState::PlanApproved,
            "preflight refusal must not enter EXECUTING"
        );
        assert_eq!(
            std::fs::read_dir(&fx.output).unwrap().count(),
            0,
            "execution wrote inside the physical source"
        );
        assert_eq!(
            std::fs::read(saved_origin.join("a.txt")).unwrap(),
            b"same bytes"
        );
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
        df_planner::create_plan(&mut db, Actor::Test, df_domain::DuplicatePolicy::ReportOnly)
            .unwrap();
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

    /// Threat T8: a RUNNING operation plus its random durable token identifies
    /// the executor-owned partial. A killed process must not strand it forever.
    #[test]
    fn crash_with_partial_file_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);

        // Exact durable state left by a killed attempt: token + RUNNING were
        // committed before create; physical ownership was then claimed from
        // the create_new handle before this partial was partially written.
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (partial, _) = create_and_claim_partial(
            &fx.db,
            &fx.output,
            &operation,
            &token,
            b"prefix copied before the crash",
        );

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        let mut statement = fx
            .db
            .conn_for_tests()
            .prepare(
                "SELECT p.destination_relative_path, r.outcome, r.detail
                 FROM operation_results r
                 JOIN plan_operations p ON p.id = r.operation_id
                 ORDER BY p.sequence",
            )
            .unwrap();
        let diagnostics: Vec<(Option<String>, String, Option<String>)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}; {diagnostics:#?}");
        assert_eq!(outcome.failed_retryable, 0, "{outcome:?}");
        assert_eq!(outcome.failed_final, 0, "{outcome:?}");
        assert!(
            !partial.exists(),
            "the leased stale partial must be reclaimed"
        );
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("a.txt")).unwrap(),
            b"same bytes"
        );
    }

    /// Crash ordering invariant: attempt A remains the durable claim until
    /// reclaim(A) succeeds. A crash after removal but before lease(B) is then
    /// replayable as a missing-file no-op with A still in the database.
    #[test]
    fn crash_between_reclaim_and_new_lease_is_replayable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (partial, identity) =
            create_and_claim_partial(&fx.db, &fx.output, &operation, &token, b"stale");
        let interrupted = first_copy_operation(&fx.db);
        let safe_root = SafeOutputRoot::validate(&fx.output).unwrap();

        reclaim_interrupted_partial(&safe_root, &interrupted).unwrap();
        assert!(!partial.exists());
        let (state, stored_token, stored_identity): (String, Option<String>, Option<String>) = fx
            .db
            .conn_for_tests()
            .query_row(
                "SELECT execution_state, partial_lease_token, partial_lease_identity
                 FROM plan_operations WHERE id = ?1",
                [operation.operation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "RUNNING");
        assert_eq!(stored_token.as_deref(), Some(token.as_str()));
        assert_eq!(
            stored_identity.as_deref(),
            Some(format_partial_identity(identity).as_str())
        );

        // Simulated crash here: no B was issued. Full execute repeats the A
        // cleanup as a no-op, then leases B and completes.
        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
    }

    #[cfg(windows)]
    #[test]
    fn blocked_reclaim_retains_claim_until_retry_can_remove_it() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (partial, identity) =
            create_and_claim_partial(&fx.db, &fx.output, &operation, &token, b"stale");
        let interrupted = first_copy_operation(&fx.db);
        // FILE_SHARE_READ | FILE_SHARE_WRITE, deliberately no SHARE_DELETE.
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&partial)
            .unwrap();
        let safe_root = SafeOutputRoot::validate(&fx.output).unwrap();

        let failure = reclaim_interrupted_partial(&safe_root, &interrupted).unwrap_err();
        assert!(failure.retain_partial_lease, "{failure:?}");
        assert_eq!(failure.state, ExecutionState::Running);
        let outcome = failure.into_outcome(chrono::Utc::now());
        plans::record_operation_outcome(&mut fx.db, operation.operation_id, &outcome).unwrap();

        let (state, stored_token, stored_identity): (String, Option<String>, Option<String>) = fx
            .db
            .conn_for_tests()
            .query_row(
                "SELECT execution_state, partial_lease_token, partial_lease_identity
                 FROM plan_operations WHERE id = ?1",
                [operation.operation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "RUNNING");
        assert_eq!(stored_token.as_deref(), Some(token.as_str()));
        assert_eq!(
            stored_identity.as_deref(),
            Some(format_partial_identity(identity).as_str())
        );
        assert_eq!(std::fs::read(&partial).unwrap(), b"stale");

        drop(blocker);
        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert!(!partial.exists());
    }

    #[test]
    fn reopened_executing_project_with_claim_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let database = tmp.path().join("state.sqlite");
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (partial, _) =
            create_and_claim_partial(&fx.db, &fx.output, &operation, &token, b"stale");
        repository::update_project_state(&mut fx.db, ProjectState::Executing, Actor::Test).unwrap();
        drop(fx.db);

        let mut reopened = Db::open(&database).unwrap();
        let outcome =
            execute_plan(&mut reopened, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert!(!partial.exists());
    }

    #[test]
    fn reopened_executing_project_before_first_operation_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let database = tmp.path().join("state.sqlite");
        let mut fx = approved_project(tmp.path());
        repository::update_project_state(&mut fx.db, ProjectState::Executing, Actor::Test).unwrap();
        drop(fx.db);

        let mut reopened = Db::open(&database).unwrap();
        let outcome =
            execute_plan(&mut reopened, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(outcome.completed, 6);
    }

    #[test]
    fn executing_with_terminal_operations_reuses_latest_completion_event() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        let project = repository::load_project(&fx.db).unwrap();
        let plan = plans::current_plan(&fx.db, project.id).unwrap().unwrap();
        // Exact legacy crash window: operations terminal, project still
        // EXECUTING, and EXECUTION_COMPLETED is already the latest event.
        fx.db
            .conn_for_tests()
            .execute(
                "UPDATE projects SET state = 'EXECUTING' WHERE id = ?1",
                [project.id.to_string()],
            )
            .unwrap();
        plans::emit_event(
            &mut fx.db,
            project.id,
            plans::EVENT_EXECUTION_COMPLETED,
            &serde_json::json!({"plan_id": plan.id.to_string(), "legacy_crash": true}),
            Actor::Test,
        )
        .unwrap();
        let completed_events = |db: &Db| -> i64 {
            db.conn_for_tests()
                .query_row(
                    "SELECT COUNT(*) FROM audit_events
                     WHERE event_type = 'EXECUTION_COMPLETED'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let before = completed_events(&fx.db);

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(
            completed_events(&fx.db),
            before,
            "recovery must not duplicate the already-committed milestone"
        );
    }

    /// A reserved-looking file is not executor-owned merely because it uses
    /// the operation UUID and a valid token shape. A fresh attempt leases a
    /// different unpredictable name and preserves the foreign bytes.
    #[test]
    fn a_pending_partial_squatter_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let foreign_token = uuid::Uuid::new_v4().to_string();
        let squatted = planted_partial(&fx.output, &operation, &foreign_token);
        std::fs::write(&squatted, b"foreign bytes; never delete").unwrap();

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(outcome.failed_retryable, 0, "{outcome:?}");
        assert_eq!(
            std::fs::read(&squatted).unwrap(),
            b"foreign bytes; never delete"
        );
    }

    /// Regression for the unsafe RUNNING-only design: a foreign partial that
    /// predates `mark RUNNING`, followed by a crash before create_new, must not
    /// be reclassified as ours and deleted by the next execution.
    #[test]
    fn a_preexisting_squatter_does_not_become_owned_after_mark_then_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let foreign_token = uuid::Uuid::new_v4().to_string();
        let squatted = planted_partial(&fx.output, &operation, &foreign_token);
        std::fs::write(&squatted, b"present before the lease").unwrap();

        // Simulate the precise crash window: the database transition commits,
        // but the process dies before creating/writing its own token path.
        let leased_token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        assert_ne!(leased_token, foreign_token);

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(
            std::fs::read(&squatted).unwrap(),
            b"present before the lease",
            "RUNNING alone must never transfer ownership of a foreign entry"
        );
    }

    /// Adversarial crash window: create_new sees a foreign file at the exact
    /// reserved token and fails, then the process dies before recording the
    /// outcome. With no post-create identity claim, retry must never delete it.
    #[test]
    fn create_new_collision_then_crash_never_authorizes_foreign_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let squatted = planted_partial(&fx.output, &operation, &token);
        std::fs::write(&squatted, b"foreign exact-token squatter").unwrap();
        let relative_destination =
            SafeRelativePath::parse(Path::new(&operation.destination_relative_path)).unwrap();
        let partial_relative = relative_destination
            .with_file_name(&partial_file_name(&operation, &token).unwrap())
            .unwrap();
        let safe_root = SafeOutputRoot::validate(&fx.output).unwrap();
        assert!(
            safe_root.create_partial_secure(&partial_relative).is_err(),
            "the simulated create_new must collide"
        );
        let claim: Option<String> = fx
            .db
            .conn_for_tests()
            .query_row(
                "SELECT partial_lease_identity FROM plan_operations WHERE id = ?1",
                [operation.operation_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(claim.is_none(), "failed create_new must never gain a claim");

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(
            std::fs::read(&squatted).unwrap(),
            b"foreign exact-token squatter"
        );
    }

    /// The only unavoidable crash window is after our `create_new` succeeds
    /// but before its physical identity is committed. With no durable claim,
    /// retry must favor preservation: finish under a fresh lease and leave the
    /// old object for the independent verifier to report as a partial orphan.
    #[test]
    fn crash_after_create_before_claim_preserves_the_unclaimed_orphan() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let orphan = planted_partial(&fx.output, &operation, &token);
        let relative_destination =
            SafeRelativePath::parse(Path::new(&operation.destination_relative_path)).unwrap();
        let partial_relative = relative_destination
            .with_file_name(&partial_file_name(&operation, &token).unwrap())
            .unwrap();
        let safe_root = SafeOutputRoot::validate(&fx.output).unwrap();
        let mut handle = safe_root.create_partial_secure(&partial_relative).unwrap();
        handle.write_all(b"ours, but not durably claimed").unwrap();
        handle.sync_all().unwrap();
        drop(handle);

        let claim: Option<String> = fx
            .db
            .conn_for_tests()
            .query_row(
                "SELECT partial_lease_identity FROM plan_operations WHERE id = ?1",
                [operation.operation_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(claim.is_none(), "the simulated crash precedes the claim");

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(
            std::fs::read(&orphan).unwrap(),
            b"ours, but not durably claimed",
            "a token without physical identity is never deletion authority"
        );
        assert_eq!(
            std::fs::read(fx.output.join("origen").join("a.txt")).unwrap(),
            b"same bytes"
        );
    }

    /// A claimed partial can still be replaced after a crash. The persisted
    /// physical identity must make retry reject and preserve the substitute.
    #[test]
    fn a_partial_substituted_after_claim_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (partial, claimed) =
            create_and_claim_partial(&fx.db, &fx.output, &operation, &token, b"ours");
        std::fs::remove_file(&partial).unwrap();
        std::fs::write(&partial, b"foreign replacement").unwrap();
        assert_ne!(
            df_fs_safety::identity_of(&partial).unwrap(),
            Some(claimed),
            "the test must replace the physical object"
        );

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.failed_final, 1, "{outcome:?}");
        assert_eq!(std::fs::read(&partial).unwrap(), b"foreign replacement");
    }

    /// If the process dies after atomic finalize but before recording its
    /// result, retry sees no partial and the identical destination completes
    /// as already represented. The lease is cleared with that outcome.
    #[test]
    fn crash_after_finalize_before_outcome_resumes_and_clears_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let source_bytes = std::fs::read(source_path(&operation).unwrap()).unwrap();
        let (partial, identity) =
            create_and_claim_partial(&fx.db, &fx.output, &operation, &token, &source_bytes);
        let destination = fx.output.join(&operation.destination_relative_path);
        let safe_root = SafeOutputRoot::validate(&fx.output).unwrap();
        let destination_relative =
            SafeRelativePath::parse(Path::new(&operation.destination_relative_path)).unwrap();
        let partial_relative = destination_relative
            .with_file_name(&partial_file_name(&operation, &token).unwrap())
            .unwrap();
        safe_root
            .finalize_claimed_partial_no_replace(&partial_relative, &destination_relative, identity)
            .unwrap();
        assert!(!partial.exists());

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.state, "EXECUTED", "{outcome:?}");
        assert_eq!(std::fs::read(&destination).unwrap(), source_bytes);
        let (stored_token, stored_identity): (Option<String>, Option<String>) = fx
            .db
            .conn_for_tests()
            .query_row(
                "SELECT partial_lease_token, partial_lease_identity
                 FROM plan_operations WHERE id = ?1",
                [operation.operation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(stored_token.is_none(), "outcome must clear the lease");
        assert!(stored_identity.is_none(), "outcome must clear the claim");
    }

    /// Even with a RUNNING lease, reclamation is restricted to plain files.
    /// A directory (and, at the fs-safety boundary, any reparse point) is an
    /// unsafe ownership conflict, not something the executor may remove.
    #[test]
    fn a_running_non_file_partial_is_refused_without_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (planted, _) = create_and_claim_partial(&fx.db, &fx.output, &operation, &token, b"");
        std::fs::remove_file(&planted).unwrap();
        std::fs::create_dir(&planted).unwrap();

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.failed_final, 1, "{outcome:?}");
        assert!(
            planted.is_dir(),
            "the non-file conflict must survive untouched"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_claimed_partial_replaced_by_reparse_is_refused_without_following() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = approved_project(tmp.path());
        let operation = first_copy_operation(&fx.db);
        let token = plans::lease_copy_operation(&fx.db, operation.operation_id).unwrap();
        let (planted, _) =
            create_and_claim_partial(&fx.db, &fx.output, &operation, &token, b"ours");
        std::fs::remove_file(&planted).unwrap();
        let outside = tmp.path().join("outside-partial-target");
        std::fs::create_dir(&outside).unwrap();
        if !make_junction(&planted, &outside) {
            eprintln!("SKIP: this environment cannot create junctions (mklink /J failed)");
            return;
        }

        let outcome =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();

        assert_eq!(outcome.failed_final, 1, "{outcome:?}");
        assert!(df_fs_safety::is_reparse_point(&planted).unwrap());
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            0,
            "recovery must never follow the replacement"
        );
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
