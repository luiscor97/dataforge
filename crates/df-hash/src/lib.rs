//! Content hashing (RFC-0001 §12.3, §14).
//!
//! Full mode only for now: every scanned-OK occurrence gets BLAKE3
//! (operational identity) and SHA-256 (canonical audit identity) in a single
//! streaming read. The two-step fast mode of §14.4 is a later optimisation.
//!
//! Safety properties:
//! - files are opened read-only; the origin is never written (rule 1);
//! - the fingerprint captured at scan time is re-checked before *and* after
//!   reading; any change marks the job `SOURCE_CHANGED` (§14.5) instead of
//!   recording a hash that may not describe the scanned file;
//! - work is a persistent queue (`hash_jobs`): a killed or paused run
//!   resumes where it stopped (rule 13);
//! - per-file failures never abort the run.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use df_db::inventory::{self, PendingHashJob};
use df_db::{repository, Db};
use df_domain::{Actor, FileFingerprint, HashState, ProjectState};
use df_error::{DfError, DfResult};
use serde::Serialize;
use sha2::Digest;

/// Tuning knobs of one hash run.
#[derive(Debug, Clone)]
pub struct HashOptions {
    /// Bytes read per I/O call while streaming a file.
    pub read_buffer_bytes: usize,
    /// Pending jobs fetched from the queue per round trip.
    pub job_batch: u32,
    /// Filesystem/CPU workers hashing a batch in parallel. `0` = auto
    /// (a conservative cap on the machine's parallelism). The database
    /// coordinator stays single-threaded: workers only run the pure
    /// `hash_one`, never touch SQLite, and the persisted result is identical
    /// for any worker count (M1.0.1).
    pub workers: usize,
    /// Carry content bindings forward from the previous complete snapshot
    /// when the v2 fingerprint is byte-identical with every field present
    /// (RFC-0001 §14.4 fast path; ADR-0035). Off by default: full mode is
    /// the recommendation for evidential profiles, so reuse is an explicit
    /// per-run decision, and every reused binding records its provenance.
    pub incremental: bool,
    /// Accept a project left in `HASHING` by a run that died without
    /// reaching its cancellation path (a kill, a power cut, a closed
    /// window). `HASH_PAUSED` is only ever written by the cooperative
    /// `cancel` check below, so an abrupt death strands the project in a
    /// state no stage accepts, with the queue intact but unreachable.
    ///
    /// Off by default and never inferred: DataForge cannot tell a dead run
    /// from a live one holding the same database, and silently joining a
    /// live run would have two processes hashing one queue. Setting this is
    /// the operator asserting no other run is active. The interrupted run is
    /// closed with its own `HASH_PAUSED` event before the new one starts, so
    /// the ledger shows why a `HASHING` project was allowed to restart.
    pub resume_interrupted: bool,
    /// Stop after this many files and pause, leaving the rest queued.
    ///
    /// For a caller that cannot afford to block. An agent driving the engine
    /// over stdio has one session, and a call that takes hours to return is a
    /// call that never comes back. A budget turns hashing into something it
    /// can supervise — do this much, report, decide, call again — and needs no
    /// detached process to do it, because the queue was already persistent and
    /// resumable.
    ///
    /// Exhausting it is neither a failure nor a user cancellation: it takes
    /// the same cooperative pause path, so the project lands in `HASH_PAUSED`
    /// with the remainder queued, and `pending` says how much is left. `None`
    /// runs to completion, which is what a person at a terminal wants.
    pub max_files: Option<u64>,
}

impl Default for HashOptions {
    fn default() -> Self {
        Self {
            read_buffer_bytes: 1024 * 1024,
            job_batch: 256,
            workers: 0,
            incremental: false,
            resume_interrupted: false,
            max_files: None,
        }
    }
}

/// Resolve the effective worker count. `auto` (0) caps at the machine's
/// parallelism but no higher than [`AUTO_WORKER_CAP`], because using every
/// logical thread is rarely the fastest on mixed I/O and must be measured,
/// not assumed (M1.0.1). An explicit request is honoured (still ≥ 1).
fn resolve_workers(requested: usize) -> usize {
    const AUTO_WORKER_CAP: usize = 8;
    if requested > 0 {
        return requested;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, AUTO_WORKER_CAP)
}

/// Result of a finished (or paused) hash run.
#[derive(Debug, Clone, Serialize)]
pub struct HashOutcome {
    pub snapshot_id: String,
    pub hashed: u64,
    /// Bindings carried forward from the previous snapshot (subset of
    /// `hashed`); zero unless the run was explicitly incremental.
    pub reused: u64,
    pub failed: u64,
    pub source_changed: u64,
    pub pending: u64,
    pub cancelled: bool,
    /// Project state after the run (`HASHED`, or `HASH_PAUSED` when
    /// cancelled).
    pub state: String,
}

/// Hash every pending occurrence of the latest complete snapshot and move
/// the project to `HASHED` (or `HASH_PAUSED` when `cancel` fires).
pub fn hash_project(
    db: &mut Db,
    actor: Actor,
    options: &HashOptions,
    cancel: Option<&AtomicBool>,
) -> DfResult<HashOutcome> {
    if options.read_buffer_bytes == 0 || options.job_batch == 0 {
        return Err(DfError::Validation(
            "read_buffer_bytes and job_batch must be at least 1".to_string(),
        ));
    }

    let project = repository::load_project(db)?;
    let resuming_interrupted =
        matches!(project.state, ProjectState::Hashing) && options.resume_interrupted;
    match project.state {
        ProjectState::Scanned | ProjectState::HashPaused => {}
        ProjectState::Hashing if options.resume_interrupted => {}
        ProjectState::Hashing => {
            return Err(DfError::Validation(
                "the project is in state HASHING: either a hash run is active, or one \
                 died without pausing. If no run is active, re-run with \
                 `resume_interrupted` (CLI: `--resume-interrupted`) to close the \
                 interrupted run and continue the queue"
                    .to_string(),
            ));
        }
        other => {
            return Err(DfError::Validation(format!(
                "cannot hash a project in state {other} (expected SCANNED or HASH_PAUSED)"
            )));
        }
    }
    let snapshot = inventory::latest_complete_snapshot(db, project.id)?.ok_or_else(|| {
        DfError::Validation("the project has no complete snapshot to hash".to_string())
    })?;

    // Close the dead run on the record before opening a new one. `HASHING ->
    // HASHING` is not a legal transition (§11) and skipping the pause would
    // leave the interrupted run with a `HASH_STARTED` that no event ever
    // answers.
    if resuming_interrupted {
        let interrupted = inventory::inventory_summary(db, snapshot.id)?;
        inventory::record_hash_outcome(
            db,
            project.id,
            snapshot.id,
            inventory::EVENT_HASH_PAUSED,
            &interrupted,
            0,
            actor,
        )?;
        repository::update_project_state(db, ProjectState::HashPaused, actor)?;
    }

    repository::update_project_state(db, ProjectState::Hashing, actor)?;
    // Say who is doing this, so a second process — or the same person
    // tomorrow — can see whether a run is holding the project rather than
    // guess from a state that outlives its process.
    df_db::liveness::claim(db, project.id, df_db::liveness::RunStage::Hash, actor)?;
    inventory::enqueue_hash_jobs(db, snapshot.id, actor)?;
    let reused = if options.incremental {
        inventory::reuse_previous_hash_bindings(db, project.id, snapshot.id)?
    } else {
        0
    };

    // The coordinator owns SQLite: it fetches a batch, hands the immutable
    // jobs to a bounded worker pool, and persists the batch in one
    // transaction. Workers never touch the database, so per-file commits (the
    // thing that made large runs crawl) stay gone and the persistent queue
    // keeps a crash safe — uncommitted results stay PENDING and are recomputed
    // on resume. Results are keyed by job, so the persisted state is identical
    // for any worker count.
    let workers = resolve_workers(options.workers);
    let mut cancelled = false;
    let mut budget_spent: u64 = 0;
    let mut budget_exhausted = false;
    loop {
        let jobs = inventory::pending_hash_jobs(db, snapshot.id, options.job_batch)?;
        if jobs.is_empty() {
            break;
        }
        // The budget trims the batch *before* it is dispatched. With a worker
        // pool the ceiling can only be on work handed out: a worker already
        // reading a file cannot be told to un-hash it, and stopping mid-file
        // would leave the job PENDING after paying for it.
        let mut jobs = jobs;
        if let Some(max) = options.max_files {
            let remaining = max.saturating_sub(budget_spent) as usize;
            if jobs.len() > remaining {
                jobs.truncate(remaining);
                budget_exhausted = true;
            }
        }

        let (outcomes, batch_cancelled) =
            hash_batch(&jobs, workers, options.read_buffer_bytes, cancel);
        cancelled |= batch_cancelled;
        // Only jobs that actually produced an outcome are recorded; a job
        // skipped because cancellation fired stays PENDING for the next run.
        let results: Vec<inventory::HashJobResult> = jobs
            .iter()
            .zip(outcomes)
            .filter_map(|(job, outcome)| {
                outcome.map(|outcome| inventory::HashJobResult { job, outcome })
            })
            .collect();
        budget_spent += results.len() as u64;
        inventory::record_hash_results(db, &results)?;
        // One beat per committed batch: the heartbeat tracks progress that
        // actually reached the database, never merely that the loop spun.
        df_db::liveness::beat(db, project.id)?;
        if cancelled || budget_exhausted {
            break;
        }
    }

    let summary = inventory::inventory_summary(db, snapshot.id)?;
    // A spent budget pauses exactly as a cancellation does. What matters to a
    // caller is not why it stopped but what is left, and `pending` says that
    // either way.
    let (event_type, next_state) = if cancelled || budget_exhausted {
        (inventory::EVENT_HASH_PAUSED, ProjectState::HashPaused)
    } else {
        (inventory::EVENT_HASH_COMPLETED, ProjectState::Hashed)
    };
    inventory::record_hash_outcome(
        db,
        project.id,
        snapshot.id,
        event_type,
        &summary,
        reused,
        actor,
    )?;
    let project = repository::update_project_state(db, next_state, actor)?;
    df_db::liveness::release(db, project.id)?;

    Ok(HashOutcome {
        snapshot_id: snapshot.id.to_string(),
        hashed: summary.hash_done,
        reused,
        failed: summary.hash_failed,
        source_changed: summary.hash_source_changed,
        pending: summary.hash_pending,
        cancelled,
        state: project.state.as_str().to_string(),
    })
}

/// Hash one batch across up to `workers` scoped threads, each with its own
/// reusable buffer, pulling jobs by an atomic index (self-balancing across
/// uneven file sizes). Returns one slot per input job — `None` when
/// cancellation stopped it before it started — plus whether cancellation
/// fired. No database access happens here.
fn hash_batch(
    jobs: &[PendingHashJob],
    workers: usize,
    read_buffer_bytes: usize,
    cancel: Option<&AtomicBool>,
) -> (Vec<Option<inventory::HashJobOutcome>>, bool) {
    let effective = workers.clamp(1, jobs.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);

    // Each worker returns (index, outcome) pairs; the coordinator merges them
    // back into job order. Positions never collide (each index is claimed by
    // exactly one worker via fetch_add), so the merge is unambiguous.
    let collected: Vec<Vec<(usize, inventory::HashJobOutcome)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..effective)
            .map(|_| {
                let next = &next;
                let cancelled = &cancelled;
                scope.spawn(move || {
                    let mut buffer = vec![0u8; read_buffer_bytes];
                    let mut local = Vec::new();
                    loop {
                        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                            cancelled.store(true, Ordering::Relaxed);
                            break;
                        }
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        if idx >= jobs.len() {
                            break;
                        }
                        local.push((idx, hash_one(&jobs[idx], &mut buffer)));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("hash worker panicked"))
            .collect()
    });

    let mut outcomes: Vec<Option<inventory::HashJobOutcome>> =
        (0..jobs.len()).map(|_| None).collect();
    for worker in collected {
        for (idx, outcome) in worker {
            outcomes[idx] = Some(outcome);
        }
    }
    (outcomes, cancelled.load(Ordering::Relaxed))
}

/// Hash one file without touching the database; every per-file problem
/// becomes a job outcome, never an error.
fn hash_one(job: &PendingHashJob, buffer: &mut [u8]) -> inventory::HashJobOutcome {
    use inventory::HashJobOutcome::{Closed, Hashed};

    let relative = job
        .raw_relative_path
        .as_ref()
        .map(|raw| PathBuf::from(raw.to_os_string()))
        .unwrap_or_else(|| PathBuf::from(&job.relative_path));
    let path = compose_path(&job.root_path, &relative);

    let pre = match current_fingerprint(&path) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            return Closed {
                state: HashState::Failed,
                error: format!("cannot stat `{}`: {error}", path.display()),
            };
        }
    };
    // Parsed comparison, never a string compare: a v1 token stored by an
    // older version must not be mistaken for a v2 match, and the verdict
    // distinguishes "same file, proven" from "nothing visible changed"
    // (ADR-0019).
    let stored = match FileFingerprint::parse(&job.fingerprint) {
        Ok(stored) => stored,
        Err(error) => {
            return Closed {
                state: HashState::Failed,
                error: format!("unreadable stored fingerprint: {error}"),
            };
        }
    };
    if FileFingerprint::compare(&stored, &pre).is_changed() {
        return Closed {
            state: HashState::SourceChanged,
            error: "file changed between scan and hash (RFC-0001 §14.5)".to_string(),
        };
    }

    let digests = match stream_digests(&path, buffer) {
        Ok(digests) => digests,
        Err(error) => {
            return Closed {
                state: HashState::Failed,
                error: format!("cannot read `{}`: {error}", path.display()),
            };
        }
    };

    // Post-check (§14.5): the fingerprint must not have moved while reading.
    match current_fingerprint(&path) {
        Ok(post) if !FileFingerprint::compare(&pre, &post).is_changed() => {}
        Ok(_) | Err(_) => {
            return Closed {
                state: HashState::SourceChanged,
                error: "file changed while hashing (RFC-0001 §14.5)".to_string(),
            };
        }
    }

    Hashed {
        sha256: digests.sha256,
        blake3: digests.blake3,
    }
}

struct Digests {
    sha256: String,
    blake3: String,
}

/// One streaming pass computing both digests (RFC-0001 ADR-0007).
fn stream_digests(path: &Path, buffer: &mut [u8]) -> std::io::Result<Digests> {
    let mut file = std::fs::File::open(path)?;
    let mut sha = sha2::Sha256::new();
    let mut blake = blake3::Hasher::new();
    loop {
        let read = file.read(buffer)?;
        if read == 0 {
            break;
        }
        sha.update(&buffer[..read]);
        blake.update(&buffer[..read]);
    }
    Ok(Digests {
        sha256: hex::encode(sha.finalize()),
        blake3: blake.finalize().to_hex().to_string(),
    })
}

/// Capture the current fingerprint (v2, ADR-0019).
fn current_fingerprint(path: &Path) -> DfResult<FileFingerprint> {
    Ok(df_fs_safety::capture_fingerprint(path)?)
}

/// Mirror of the scanner's path composition, including the Windows
/// extended-length prefix for long paths.
fn compose_path(root: &Path, relative: &Path) -> PathBuf {
    let joined = if relative.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    #[cfg(windows)]
    {
        const LEGACY_MAX_PATH: usize = 260;
        let text = joined.as_os_str().to_string_lossy();
        if text.len() >= LEGACY_MAX_PATH && !text.starts_with(r"\\") {
            return PathBuf::from(format!(r"\\?\{text}"));
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use df_db::inventory::{exact_duplicates, list_occurrences, name_collisions};
    use df_domain::{ProfileRef, Project, SourceRoot};
    use df_scan::{scan_project, ScanOptions};

    use super::*;

    fn scanned_project(tmp: &Path) -> (Db, PathBuf) {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(origin.join("sub")).unwrap();
        std::fs::write(origin.join("a.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("sub").join("b.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("c.txt"), b"different").unwrap();

        let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Prueba hash",
            ProfileRef::default(),
            tmp.join("salida"),
            tmp.join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin.clone())];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        (db, origin)
    }

    #[test]
    fn hashing_binds_occurrences_to_content_and_finds_exact_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());

        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.source_changed, 0);
        assert_eq!(outcome.pending, 0);
        assert_eq!(outcome.state, "HASHED");

        let snapshot_id = outcome.snapshot_id.parse().unwrap();
        let sets = exact_duplicates(&db, snapshot_id).unwrap();
        assert_eq!(sets.len(), 1, "one duplicate set expected");
        let set = &sets[0];
        assert_eq!(set.size_bytes, 10);
        assert_eq!(set.occurrences.len(), 2);
        // The SHA-256 primitive itself is validated against the official
        // "abc" vector in `known_test_vectors_for_both_algorithms`.
        assert_eq!(set.sha256, hex::encode(sha2::Sha256::digest(b"same bytes")));
        assert!(set.occurrences.iter().any(|p| p.ends_with("a.txt")));
        assert!(set.occurrences.iter().any(|p| p.ends_with("b.txt")));
    }

    /// The case from the real archive: exhibits are numbered per matter, so
    /// the same file name legitimately holds a different photograph in each.
    #[test]
    fn a_name_reused_across_matters_with_different_content_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        for (matter, bytes) in [
            ("pericial-1", b"foto uno".as_slice()),
            ("pericial-2", b"foto dos".as_slice()),
            ("pericial-3", b"foto tres".as_slice()),
        ] {
            std::fs::create_dir_all(origin.join(matter)).unwrap();
            std::fs::write(origin.join(matter).join("00000001.JPG"), bytes).unwrap();
        }
        // Same name *and* same bytes in two places: a duplicate, not a
        // collision. It must not inflate the finding.
        std::fs::create_dir_all(origin.join("copia")).unwrap();
        std::fs::write(origin.join("copia").join("informe.pdf"), b"identico").unwrap();
        std::fs::write(origin.join("informe.pdf"), b"identico").unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Colisiones",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        let snapshot_id = outcome.snapshot_id.parse().unwrap();
        let report = name_collisions(&db, snapshot_id, 4).unwrap();

        assert_eq!(
            report.colliding_names, 1,
            "only the exhibit name means more than one thing"
        );
        assert_eq!(report.worst_name_contents, 3);
        assert_eq!(report.occurrences_involved, 3);

        let collision = &report.collisions[0];
        assert_eq!(collision.normalized_name, "00000001.jpg");
        assert_eq!(collision.contents, 3);
        assert_eq!(collision.folders, 3);
        // One sample per distinct content, so the sample shows the
        // disagreement instead of repeating one side of it.
        assert_eq!(collision.sample_paths.len(), 3);
    }

    /// The shape an agent drives: bounded calls over a persistent queue.
    #[test]
    fn a_budget_stops_early_and_the_rest_stays_queued() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());

        let first = hash_project(
            &mut db,
            Actor::Test,
            &HashOptions {
                max_files: Some(2),
                ..HashOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(first.hashed, 2, "the budget is a ceiling, not a suggestion");
        assert_eq!(first.pending, 1, "and it says what is left");
        assert_eq!(
            first.state, "HASH_PAUSED",
            "a spent budget pauses on the same path a cancellation does"
        );
        // Not a cancellation: nobody asked it to stop.
        assert!(!first.cancelled);

        // Calling again finishes the job. Nothing was lost or re-read.
        let second = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert_eq!(second.hashed, 3, "counts are per snapshot, not per call");
        assert_eq!(second.pending, 0);
        assert_eq!(second.state, "HASHED");
    }

    /// The shape the real archive asked for: a folder named by path, and
    /// heavy media named by size.
    #[test]
    fn a_profile_can_decline_to_read_material_without_losing_it() {
        use df_domain::{ExclusionMatch, HashExclusion};

        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(origin.join("TomTom").join("HOME")).unwrap();
        std::fs::create_dir_all(origin.join("asunto")).unwrap();
        std::fs::write(origin.join("TomTom").join("HOME").join("mapa.dat"), b"gps").unwrap();
        std::fs::write(origin.join("asunto").join("escrito.pdf"), b"documento").unwrap();
        // Heavy enough to match the size rule below.
        std::fs::write(origin.join("asunto").join("vista.mp4"), vec![0u8; 4096]).unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Exclusiones",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();

        let exclusions = vec![
            HashExclusion {
                id: "gps.tomtom".to_string(),
                reason: "navigation data, not case material".to_string(),
                r#match: ExclusionMatch {
                    path_glob: Some("TomTom*".to_string()),
                    ..ExclusionMatch::default()
                },
            },
            HashExclusion {
                id: "media.heavy".to_string(),
                reason: "large video; identity would cost hours and settle nothing".to_string(),
                r#match: ExclusionMatch {
                    file_name_glob: Some("*.mp4".to_string()),
                    min_size_bytes: Some(1024),
                    ..ExclusionMatch::default()
                },
            },
        ];
        let snapshot = df_db::inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .unwrap();
        let outcome = df_db::inventory::enqueue_hash_jobs_with(
            &mut db,
            snapshot.id,
            Actor::Test,
            &exclusions,
        )
        .unwrap();

        assert_eq!(outcome.enqueued, 1, "only the legal document is read");
        assert_eq!(outcome.excluded, 2);

        // The whole point: excluded is not discarded. All three are still
        // inventoried, so the snapshot keeps describing the origin.
        let summary = df_db::inventory::inventory_summary(&db, snapshot.id).unwrap();
        assert_eq!(summary.files, 3, "nothing left the inventory");

        // And each exclusion carries the wording that caused it, so "why was
        // this never read?" has an answer a year from now.
        let recorded = df_db::inventory::hash_exclusions(&db, snapshot.id).unwrap();
        assert_eq!(recorded.len(), 2);
        assert!(recorded.iter().any(|e| e.rule_id == "gps.tomtom"
            && e.reason.contains("navigation")
            && e.relative_path.contains("mapa.dat")));
        assert!(recorded.iter().any(|e| e.rule_id == "media.heavy"));
    }

    #[test]
    fn parallel_hashing_is_deterministic_regardless_of_worker_count() {
        // Build 40 duplicate pairs of varied sizes: every file lands in a
        // duplicate set, so exact_duplicates covers every content identity.
        fn build(root: &Path) {
            std::fs::create_dir_all(root).unwrap();
            for i in 0..40u32 {
                let len = 1 + (i as usize * 997) % 5000;
                let content: Vec<u8> = (0..len).map(|b| ((b as u32) ^ i) as u8).collect();
                std::fs::write(root.join(format!("a-{i:03}.bin")), &content).unwrap();
                std::fs::write(root.join(format!("b-{i:03}.bin")), &content).unwrap();
            }
        }
        // Small buffer + small batch force multiple reads per file and several
        // batches, exercising the parallel path across batch boundaries.
        fn run(dir: &Path, workers: usize) -> (u64, u64, Vec<(String, u64, usize)>) {
            let origin = dir.join("origen");
            build(&origin);
            let mut db = Db::open(&dir.join("state.sqlite")).unwrap();
            let project = Project::new(
                "det",
                ProfileRef::default(),
                dir.join("out"),
                dir.join("audit"),
                "test",
            );
            let roots = vec![SourceRoot::new(project.id, origin)];
            repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
            scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
            let options = HashOptions {
                read_buffer_bytes: 512,
                job_batch: 7,
                workers,
                ..HashOptions::default()
            };
            let outcome = hash_project(&mut db, Actor::Test, &options, None).unwrap();
            let snapshot = outcome.snapshot_id.parse().unwrap();
            let mut sets: Vec<(String, u64, usize)> = exact_duplicates(&db, snapshot)
                .unwrap()
                .into_iter()
                .map(|s| (s.sha256, s.size_bytes, s.occurrences.len()))
                .collect();
            sets.sort();
            (outcome.hashed, outcome.failed, sets)
        }

        let sequential = run(&tempfile::tempdir().unwrap().path().join("seq"), 1);
        let parallel = run(&tempfile::tempdir().unwrap().path().join("par"), 8);
        assert_eq!(sequential.0, 80, "all 80 files hashed");
        assert_eq!(sequential.1, 0, "no failures");
        assert_eq!(
            sequential, parallel,
            "worker count must not change hashing results"
        );
    }

    #[test]
    fn known_test_vectors_for_both_algorithms() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vector.bin");
        std::fs::write(&path, b"abc").unwrap();
        let digests = stream_digests(&path, &mut [0u8; 4]).unwrap();
        // FIPS 180-2 appendix B.1 test vector.
        assert_eq!(
            digests.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Official BLAKE3 test vector for input "abc".
        assert_eq!(digests.blake3, blake3::hash(b"abc").to_hex().to_string());
    }

    #[test]
    fn a_file_that_changes_after_scan_is_marked_source_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, origin) = scanned_project(tmp.path());
        // Grow the file so size (hence fingerprint) differs from scan time.
        std::fs::write(origin.join("c.txt"), b"different and longer now").unwrap();

        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert_eq!(outcome.hashed, 2);
        assert_eq!(outcome.source_changed, 1);
        assert_eq!(outcome.state, "HASHED");
    }

    #[test]
    fn a_deleted_file_fails_its_job_without_aborting_the_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, origin) = scanned_project(tmp.path());
        std::fs::remove_file(origin.join("c.txt")).unwrap();

        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert_eq!(outcome.hashed, 2);
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.state, "HASHED");
    }

    #[test]
    fn cancellation_pauses_and_a_second_run_resumes_the_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());

        let cancel = AtomicBool::new(true); // cancel before the first job
        let outcome =
            hash_project(&mut db, Actor::Test, &HashOptions::default(), Some(&cancel)).unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.state, "HASH_PAUSED");
        assert_eq!(outcome.pending, 3);
        assert_eq!(outcome.hashed, 0);

        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert!(!outcome.cancelled);
        assert_eq!(outcome.state, "HASHED");
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.pending, 0);
    }

    /// A run that dies without reaching its cancellation path leaves the
    /// project in `HASHING`. The queue survives, but no stage accepts that
    /// state, so without an explicit opt-in the work is stranded.
    #[test]
    fn an_interrupted_run_is_refused_by_default_and_resumed_on_request() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());

        // Simulate the abrupt death: the state the engine writes when a run
        // starts, with nothing that ever answers it.
        repository::update_project_state(&mut db, ProjectState::Hashing, Actor::Test).unwrap();

        let refused = hash_project(&mut db, Actor::Test, &HashOptions::default(), None);
        let message = refused.unwrap_err().to_string();
        assert!(message.contains("HASHING"), "{message}");
        assert!(message.contains("--resume-interrupted"), "{message}");

        let outcome = hash_project(
            &mut db,
            Actor::Test,
            &HashOptions {
                resume_interrupted: true,
                ..HashOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(outcome.state, "HASHED");
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.pending, 0);
    }

    /// Resuming must not throw away hashes the dead run already committed:
    /// the queue is the record, and it is picked up where it stopped. Over a
    /// million files interrupted near the end, re-queueing the done ones would
    /// mean redoing hours of work that is already on disk and correct.
    ///
    /// The partial state is built directly rather than through cancellation:
    /// `hash_project` runs to completion on the calling thread, so a flag
    /// flipped from that same thread is either already true (nothing done) or
    /// never true (everything done) — never the half-done case this is about.
    /// Committing one job by hand reproduces exactly what a kill leaves.
    #[test]
    fn resuming_an_interrupted_run_keeps_the_work_already_committed() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());
        let snapshot = df_db::inventory::latest_complete_snapshot(
            &db,
            repository::load_project(&db).unwrap().id,
        )
        .unwrap()
        .unwrap()
        .id;

        // A run that started, hashed exactly one file, and died.
        repository::update_project_state(&mut db, ProjectState::Hashing, Actor::Test).unwrap();
        inventory::enqueue_hash_jobs(&mut db, snapshot, Actor::Test).unwrap();
        let one = inventory::pending_hash_jobs(&db, snapshot, 1).unwrap();
        assert_eq!(one.len(), 1);
        let mut buffer = vec![0u8; 1024];
        let done = vec![inventory::HashJobResult {
            job: &one[0],
            outcome: hash_one(&one[0], &mut buffer),
        }];
        inventory::record_hash_results(&mut db, &done).unwrap();

        // Two left, and that is what the resume must find: the finished one
        // keeps its job row, so `enqueue_hash_jobs` cannot resurrect it.
        assert_eq!(
            inventory::pending_hash_jobs(&db, snapshot, u32::MAX)
                .unwrap()
                .len(),
            2,
            "the committed hash must not return to the queue"
        );

        let outcome = hash_project(
            &mut db,
            Actor::Test,
            &HashOptions {
                resume_interrupted: true,
                ..HashOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(outcome.state, "HASHED");
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.pending, 0);
    }

    // Windows: NTFS provides every v2 field, so identity carries.
    #[cfg(windows)]
    #[test]
    fn incremental_rescan_reuses_unchanged_bindings() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        // Nothing changed on disk: a rescan plus incremental hash must
        // carry every binding forward without reading a byte of content.
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let outcome = hash_project(
            &mut db,
            Actor::Test,
            &HashOptions {
                incremental: true,
                ..HashOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(outcome.hashed, 3);
        assert_eq!(
            outcome.reused, 3,
            "identical v2 fingerprints carry identity"
        );
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.state, "HASHED");

        // Duplicate resolution works identically through reused bindings.
        let snapshot_id = outcome.snapshot_id.parse().unwrap();
        let sets = exact_duplicates(&db, snapshot_id).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].occurrences.len(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn incremental_rescan_rehashes_a_changed_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, origin) = scanned_project(tmp.path());
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        // Change one file's bytes (and size, hence fingerprint).
        std::fs::write(origin.join("c.txt"), b"contenido distinto y mas largo").unwrap();

        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let outcome = hash_project(
            &mut db,
            Actor::Test,
            &HashOptions {
                incremental: true,
                ..HashOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.reused, 2, "only unchanged files reuse identity");
        assert_eq!(outcome.failed, 0);

        // The changed file's new content hash is real, not carried over.
        let snapshot_id = outcome.snapshot_id.parse().unwrap();
        let sets = exact_duplicates(&db, snapshot_id).unwrap();
        assert_eq!(sets.len(), 1, "the a/b duplicate pair persists");
    }

    /// POSIX: the captured fingerprint lacks full physical identity
    /// (no NTFS attributes), so incremental reuse must refuse and take
    /// the full-hash path — the conservative rule of ADR-0035, pinned.
    #[cfg(not(windows))]
    #[test]
    fn incremental_reuse_refuses_without_full_physical_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let outcome = hash_project(
            &mut db,
            Actor::Test,
            &HashOptions {
                incremental: true,
                ..HashOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.reused, 0, "weaker identity must never carry");
    }

    #[test]
    fn full_mode_is_the_default_and_never_reuses() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();

        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.reused, 0, "reuse is an explicit per-run decision");
    }

    #[test]
    fn duplicate_content_across_separate_batches_maps_to_one_content_object() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());

        // One job per batch (and per transaction): the second copy of
        // "same bytes" must find the content object committed by an
        // earlier batch, not a row from its own transaction.
        let options = HashOptions {
            job_batch: 1,
            ..HashOptions::default()
        };
        let outcome = hash_project(&mut db, Actor::Test, &options, None).unwrap();
        assert_eq!(outcome.hashed, 3);
        assert_eq!(outcome.failed, 0);

        let snapshot_id = outcome.snapshot_id.parse().unwrap();
        let sets = exact_duplicates(&db, snapshot_id).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].occurrences.len(), 2);
    }

    #[test]
    fn hashing_requires_a_scanned_project() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Sin escanear",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        repository::create_project(&mut db, &project, &[], Actor::Test).unwrap();
        let err = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[test]
    fn identical_content_maps_to_a_single_content_object() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = scanned_project(tmp.path());
        let outcome = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        let snapshot_id = outcome.snapshot_id.parse().unwrap();
        // 3 occurrences, but only 2 unique contents.
        assert_eq!(list_occurrences(&db, snapshot_id).unwrap().len(), 3);
        let summary = inventory::inventory_summary(&db, snapshot_id).unwrap();
        assert_eq!(summary.hash_done, 3);

        let project = repository::load_project(&db).unwrap();
        let events = repository::list_events(&db, project.id).unwrap();
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"HASH_STARTED"));
        assert!(types.contains(&"HASH_COMPLETED"));
        df_ledger::verify_chain(&events).expect("ledger stays valid");
    }

    #[cfg(windows)]
    #[test]
    fn invalid_utf16_source_path_is_reopened_from_raw_identity() {
        use std::os::windows::ffi::OsStringExt;

        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        let name = std::ffi::OsString::from_wide(&[
            b'i' as u16,
            b'n' as u16,
            b'v' as u16,
            0xD800,
            b'.' as u16,
            b'b' as u16,
            b'i' as u16,
            b'n' as u16,
        ]);
        let source = origin.join(&name);
        std::fs::write(&source, b"raw utf16 identity").unwrap();

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "UTF-16 no representable",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        repository::create_project(
            &mut db,
            &project,
            &[SourceRoot::new(project.id, origin)],
            Actor::Test,
        )
        .unwrap();
        let scanned = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        assert_eq!(scanned.files, 1);
        assert_eq!(scanned.errors, 0);
        let snapshot_id = scanned.snapshot_id.parse().unwrap();
        let occurrences = list_occurrences(&db, snapshot_id).unwrap();
        assert_eq!(occurrences.len(), 1);
        assert!(occurrences[0]
            .raw_relative_path
            .as_ref()
            .is_some_and(df_domain::RawPath::is_lossy));

        let hashed = hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        assert_eq!(hashed.hashed, 1);
        assert_eq!(hashed.failed, 0);
        assert_eq!(hashed.source_changed, 0);
        let summary = inventory::inventory_summary(&db, snapshot_id).unwrap();
        assert_eq!(summary.hash_done, 1);
    }
}
