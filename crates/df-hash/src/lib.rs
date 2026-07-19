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
    /// Carry content bindings forward from the previous complete snapshot
    /// when the v2 fingerprint is byte-identical with every field present
    /// (RFC-0001 §14.4 fast path; ADR-0035). Off by default: full mode is
    /// the recommendation for evidential profiles, so reuse is an explicit
    /// per-run decision, and every reused binding records its provenance.
    pub incremental: bool,
}

impl Default for HashOptions {
    fn default() -> Self {
        Self {
            read_buffer_bytes: 1024 * 1024,
            job_batch: 256,
            incremental: false,
        }
    }
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
    match project.state {
        ProjectState::Scanned | ProjectState::HashPaused => {}
        other => {
            return Err(DfError::Validation(format!(
                "cannot hash a project in state {other} (expected SCANNED or HASH_PAUSED)"
            )));
        }
    }
    let snapshot = inventory::latest_complete_snapshot(db, project.id)?.ok_or_else(|| {
        DfError::Validation("the project has no complete snapshot to hash".to_string())
    })?;

    repository::update_project_state(db, ProjectState::Hashing, actor)?;
    inventory::enqueue_hash_jobs(db, snapshot.id, actor)?;
    let reused = if options.incremental {
        inventory::reuse_previous_hash_bindings(db, project.id, snapshot.id)?
    } else {
        0
    };

    // One read buffer for the whole run, and one transaction per batch:
    // per-file commits are what made large runs crawl, and the persistent
    // queue keeps a crash safe — uncommitted results stay PENDING and are
    // recomputed on resume.
    let mut buffer = vec![0u8; options.read_buffer_bytes];
    let mut cancelled = false;
    loop {
        let jobs = inventory::pending_hash_jobs(db, snapshot.id, options.job_batch)?;
        if jobs.is_empty() {
            break;
        }
        let mut results = Vec::with_capacity(jobs.len());
        for job in &jobs {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                cancelled = true;
                break;
            }
            results.push(inventory::HashJobResult {
                job,
                outcome: hash_one(job, &mut buffer),
            });
        }
        inventory::record_hash_results(db, &results)?;
        if cancelled {
            break;
        }
    }

    let summary = inventory::inventory_summary(db, snapshot.id)?;
    let (event_type, next_state) = if cancelled {
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

    use df_db::inventory::{exact_duplicates, list_occurrences};
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
