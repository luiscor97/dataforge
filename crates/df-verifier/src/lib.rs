//! Independent verification of an executed plan (RFC-0001 §12.10, §28).
//!
//! Re-checks everything from primary evidence, without trusting the
//! executor's in-memory state:
//! - the approved plan re-serializes to its recorded SHA-256 (§26.4);
//! - every executable operation reached `COMPLETED` (coverage, §28.1);
//! - every artefact exists and every copy re-hashes to the content identity
//!   recorded at hash time (§28.2);
//! - no partial file was left behind;
//! - no untracked file sits inside the plan's output subtrees;
//! - the origin still matches the snapshot fingerprints.
//!
//! Problems fail the verification; warnings degrade it to
//! `COMPLETED_WITH_WARNINGS`.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use df_db::plans::{self, VerificationFinding};
use df_db::{repository, Db};
use df_domain::{Actor, FileFingerprint, OperationType, ProjectState};
use df_error::{DfError, DfResult};
use serde::Serialize;
use sha2::Digest;

const SEVERITY_PROBLEM: &str = "PROBLEM";
const SEVERITY_WARNING: &str = "WARNING";
const PARTIAL_FILE_PREFIX: &str = ".dataforge-partial-";

/// Tuning knobs of one verification run.
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Bytes per I/O call while re-hashing artefacts.
    pub read_buffer_bytes: usize,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            read_buffer_bytes: 1024 * 1024,
        }
    }
}

/// Result of a verification run.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyOutcome {
    pub verification_run_id: String,
    pub plan_id: String,
    /// `COMPLETED`, `COMPLETED_WITH_WARNINGS` or `FAILED`.
    pub verdict: String,
    /// Artefacts re-checked on disk.
    pub checked: u64,
    pub problems: u64,
    pub warnings: u64,
    pub findings: Vec<VerificationFinding>,
    /// Project state after the run.
    pub state: String,
}

/// Verify the executed plan and close the pipeline:
/// `EXECUTED → VERIFYING → COMPLETED | COMPLETED_WITH_WARNINGS | FAILED`.
pub fn verify_project(
    db: &mut Db,
    actor: Actor,
    options: &VerifyOptions,
) -> DfResult<VerifyOutcome> {
    if options.read_buffer_bytes == 0 {
        return Err(DfError::Validation(
            "read_buffer_bytes must be at least 1".to_string(),
        ));
    }

    let project = repository::load_project(db)?;
    if project.state != ProjectState::Executed {
        return Err(DfError::Validation(format!(
            "cannot verify a project in state {} (expected EXECUTED)",
            project.state
        )));
    }
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;

    repository::update_project_state(db, ProjectState::Verifying, actor)?;
    let started_at = chrono::Utc::now();

    let mut findings: Vec<VerificationFinding> = Vec::new();
    let mut checked: u64 = 0;

    // 1. Plan immutability (§26.4): stored operations must re-serialize to
    //    the hash frozen at approval.
    let operations = plans::list_operations(db, plan.id)?;
    // Re-hash the frozen manifest, not the operation rows: the manifest is
    // what approval signed (ADR-0018), so this is what detects a post-approval
    // edit to the executed material.
    let manifest = plans::manifest(db, plan.id)?;
    let recomputed = df_planner::manifest_sha256(&manifest);
    match plan.serialized_sha256.as_deref() {
        Some(stored) if stored == recomputed => {}
        stored => findings.push(VerificationFinding {
            kind: "PLAN_TAMPERED".to_string(),
            severity: SEVERITY_PROBLEM.to_string(),
            subject: plan.id.to_string(),
            detail: format!(
                "the execution manifest re-serializes to {recomputed} but approval recorded {}",
                stored.unwrap_or("nothing")
            ),
        }),
    }

    // 2. Execution coverage (§28.1): every executable operation completed.
    for op in &operations {
        if op.operation_type.is_executable()
            && op.execution_state != df_domain::ExecutionState::Completed
        {
            findings.push(VerificationFinding {
                kind: "INCOMPLETE_OPERATION".to_string(),
                severity: SEVERITY_PROBLEM.to_string(),
                subject: op
                    .destination_relative_path
                    .clone()
                    .unwrap_or_else(|| format!("operation #{}", op.sequence)),
                detail: format!(
                    "operation #{} ({}) is {} — {}",
                    op.sequence,
                    op.operation_type.as_str(),
                    op.execution_state.as_str(),
                    op.reason
                ),
            });
        }
    }

    // 3. Artefacts (§28.2): existence and content identity, re-read from disk.
    let artefacts = plans::verifiable_artefacts(db, plan.id)?;
    let mut expected_files: HashSet<String> = HashSet::new();
    let mut expected_dirs: HashSet<String> = HashSet::new();
    for artefact in &artefacts {
        // Long outputs are legitimate (the executor writes them via the
        // extended-length prefix), so the verifier must re-read them the same
        // way or it would report every deep artefact as missing.
        let destination =
            df_fs_safety::extended_for_io(&project.output_root.join(&artefact.final_relative_path))
                .into_owned();
        checked += 1;
        if artefact.operation_type == OperationType::CreateDirectory {
            expected_dirs.insert(artefact.final_relative_path.to_lowercase());
            if !destination.is_dir() {
                findings.push(VerificationFinding {
                    kind: "MISSING_DESTINATION".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: artefact.final_relative_path.clone(),
                    detail: "planned directory is missing from the output".to_string(),
                });
            }
            continue;
        }
        expected_files.insert(artefact.final_relative_path.to_lowercase());
        if !destination.is_file() {
            findings.push(VerificationFinding {
                kind: "MISSING_DESTINATION".to_string(),
                severity: SEVERITY_PROBLEM.to_string(),
                subject: artefact.final_relative_path.clone(),
                detail: "copied file is missing from the output".to_string(),
            });
            continue;
        }
        match (
            &artefact.expected_sha256,
            hash_file(&destination, options.read_buffer_bytes),
        ) {
            (Some(expected), Ok(actual)) if &actual == expected => {}
            (Some(expected), Ok(actual)) => findings.push(VerificationFinding {
                kind: "HASH_MISMATCH".to_string(),
                severity: SEVERITY_PROBLEM.to_string(),
                subject: artefact.final_relative_path.clone(),
                detail: format!("output hashes to {actual}, snapshot recorded {expected}"),
            }),
            (None, _) => findings.push(VerificationFinding {
                kind: "HASH_MISMATCH".to_string(),
                severity: SEVERITY_PROBLEM.to_string(),
                subject: artefact.final_relative_path.clone(),
                detail: "no content identity recorded for a completed copy".to_string(),
            }),
            (_, Err(error)) => findings.push(VerificationFinding {
                kind: "HASH_MISMATCH".to_string(),
                severity: SEVERITY_PROBLEM.to_string(),
                subject: artefact.final_relative_path.clone(),
                detail: format!("output could not be re-read: {error}"),
            }),
        }
    }

    // 4–5. Walk the plan's output subtrees: no partials, no untracked files.
    let top_dirs: Vec<String> = artefacts
        .iter()
        .filter(|a| {
            a.operation_type == OperationType::CreateDirectory
                && !a.final_relative_path.contains(std::path::MAIN_SEPARATOR)
        })
        .map(|a| a.final_relative_path.clone())
        .collect();
    for top in &top_dirs {
        walk_output(
            &project.output_root,
            Path::new(top),
            &expected_files,
            &expected_dirs,
            &mut findings,
        );
    }

    // 6. Origin integrity: the source must still match the snapshot.
    let roots = repository::load_source_roots(db, project.id)?;
    let root_paths: std::collections::HashMap<_, _> = roots
        .iter()
        .map(|r| (r.id, r.absolute_path.clone()))
        .collect();
    for occurrence in df_db::inventory::list_occurrences(db, plan.snapshot_id)? {
        if occurrence.scan_status != df_domain::ScanEntryStatus::Ok {
            continue;
        }
        let Some(root) = root_paths.get(&occurrence.source_root_id) else {
            continue;
        };
        let source = root.join(&occurrence.relative_path);
        // Parsed comparison against the snapshot's fingerprint (ADR-0019); a
        // v1 token from an older snapshot is compared on what it carries and
        // never mistaken for a proven match.
        let stored = FileFingerprint::parse(&occurrence.fingerprint).ok();
        let current = df_fs_safety::capture_fingerprint(&source).ok();
        let changed = match (&stored, &current) {
            (Some(stored), Some(current)) => FileFingerprint::compare(stored, current).is_changed(),
            // Unreadable now, or an unparsable stored token: both are worth
            // reporting rather than silently treating as unchanged.
            _ => true,
        };
        if changed {
            findings.push(VerificationFinding {
                kind: "ORIGIN_CHANGED".to_string(),
                severity: SEVERITY_WARNING.to_string(),
                subject: occurrence.relative_path.clone(),
                detail: match (stored, current) {
                    (Some(_), Some(_)) => "source file changed since the snapshot".to_string(),
                    (None, _) => "the snapshot's fingerprint is unreadable".to_string(),
                    (_, None) => "source file is no longer readable".to_string(),
                },
            });
        }
    }

    let problems = findings
        .iter()
        .filter(|f| f.severity == SEVERITY_PROBLEM)
        .count() as u64;
    let warnings = findings
        .iter()
        .filter(|f| f.severity == SEVERITY_WARNING)
        .count() as u64;
    let (verdict, next_state) = if problems > 0 {
        ("FAILED", ProjectState::Failed)
    } else if warnings > 0 {
        (
            "COMPLETED_WITH_WARNINGS",
            ProjectState::CompletedWithWarnings,
        )
    } else {
        ("COMPLETED", ProjectState::Completed)
    };

    let run_id = plans::record_verification_run(
        db, project.id, plan.id, verdict, checked, &findings, started_at, actor,
    )?;
    let project = repository::update_project_state(db, next_state, actor)?;

    Ok(VerifyOutcome {
        verification_run_id: run_id.to_string(),
        plan_id: plan.id.to_string(),
        verdict: verdict.to_string(),
        checked,
        problems,
        warnings,
        findings,
        state: project.state.as_str().to_string(),
    })
}

/// Recursively inspect one plan output subtree for §28.2 invariants.
///
/// This walk is **adversarial by design** (ADR-0017, threat T7). The previous
/// version used `entry.metadata()`, which follows links: a junction planted in
/// the output was reported as an ordinary directory, pushed onto the queue and
/// walked into — so the verifier happily read files *outside* the output root
/// and called the result verified. It also swallowed `read_dir` errors with a
/// silent `continue`, so an unreadable subtree looked like an empty one.
///
/// Now:
/// - every entry is stat'ed with `symlink_metadata` (never follows);
/// - any reparse point is a `PROBLEM` and is never descended into;
/// - read failures become findings instead of silence;
/// - cycles are cut by physical identity, not by a depth limit.
fn walk_output(
    output_root: &Path,
    subtree: &Path,
    expected_files: &HashSet<String>,
    expected_dirs: &HashSet<String>,
    findings: &mut Vec<VerificationFinding>,
) {
    // Directories already visited, by physical identity: a junction loop would
    // otherwise spin forever with ever-longer paths.
    let mut visited: HashSet<df_fs_safety::FileIdentity> = HashSet::new();
    let mut queue: Vec<PathBuf> = vec![subtree.to_path_buf()];

    while let Some(dir_rel) = queue.pop() {
        let dir_abs = output_root.join(&dir_rel);
        let dir_text = dir_rel.to_string_lossy().into_owned();

        // A directory we are about to read must itself not be a link.
        match df_fs_safety::is_reparse_point(&dir_abs) {
            Ok(true) => {
                findings.push(VerificationFinding {
                    kind: "OUTPUT_REPARSE_POINT".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: dir_text,
                    detail: "a directory in the output is a reparse point; the output is not \
                             fully under DataForge's control (RFC-0001 §28.2)"
                        .to_string(),
                });
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                findings.push(VerificationFinding {
                    kind: "OUTPUT_SUBTREE_UNREADABLE".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: dir_text,
                    detail: format!("could not inspect the directory: {error}"),
                });
                continue;
            }
        }

        // Cycle detection by identity (§28.2): cheap and exact.
        if let Ok(Some(identity)) = df_fs_safety::identity_of(&dir_abs) {
            if !visited.insert(identity) {
                findings.push(VerificationFinding {
                    kind: "OUTPUT_CYCLE_DETECTED".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: dir_text,
                    detail: "this directory was already visited; the output contains a cycle"
                        .to_string(),
                });
                continue;
            }
        }

        let entries = match std::fs::read_dir(df_fs_safety::extended_for_io(&dir_abs)) {
            Ok(entries) => entries,
            Err(error) => {
                // Never a silent continue: an output we cannot read is an
                // output we cannot certify.
                findings.push(VerificationFinding {
                    kind: "OUTPUT_SUBTREE_UNREADABLE".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: dir_text,
                    detail: format!("could not list the directory: {error}"),
                });
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    findings.push(VerificationFinding {
                        kind: "OUTPUT_SUBTREE_UNREADABLE".to_string(),
                        severity: SEVERITY_PROBLEM.to_string(),
                        subject: dir_text.clone(),
                        detail: format!("could not read a directory entry: {error}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = dir_rel.join(&name);
            let rel_text = rel.to_string_lossy().into_owned();
            let abs = output_root.join(&rel);

            // symlink_metadata: describes the entry itself, not its target.
            let metadata = match std::fs::symlink_metadata(df_fs_safety::extended_for_io(&abs)) {
                Ok(metadata) => metadata,
                Err(error) => {
                    findings.push(VerificationFinding {
                        kind: "OUTPUT_SUBTREE_UNREADABLE".to_string(),
                        severity: SEVERITY_PROBLEM.to_string(),
                        subject: rel_text,
                        detail: format!("could not stat the entry: {error}"),
                    });
                    continue;
                }
            };

            // A reparse point is reported and never followed — this is the one
            // that used to let the walk escape.
            if df_fs_safety::metadata_is_reparse(&metadata) {
                findings.push(VerificationFinding {
                    kind: "OUTPUT_REPARSE_POINT".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: rel_text,
                    detail: "a symlink, junction or mount point appeared in the output; \
                             DataForge never creates one, so the output is not solely under \
                             its control (RFC-0001 §28.2)"
                        .to_string(),
                });
                continue;
            }

            if metadata.is_dir() {
                if !expected_dirs.contains(&rel_text.to_lowercase()) {
                    findings.push(VerificationFinding {
                        kind: "UNTRACKED_FILE".to_string(),
                        severity: SEVERITY_WARNING.to_string(),
                        subject: rel_text,
                        detail: "directory inside the plan output was not produced by the plan"
                            .to_string(),
                    });
                }
                queue.push(rel);
                continue;
            }

            if name.starts_with(PARTIAL_FILE_PREFIX) {
                findings.push(VerificationFinding {
                    kind: "PARTIAL_LEFTOVER".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: rel_text,
                    detail: "a partial file survived execution (RFC-0001 §28.2)".to_string(),
                });
                continue;
            }

            if !expected_files.contains(&rel_text.to_lowercase()) {
                // Still a warning: a legible, ordinary file inside the
                // legitimate tree is untidy, not a breach of containment.
                findings.push(VerificationFinding {
                    kind: "UNTRACKED_FILE".to_string(),
                    severity: SEVERITY_WARNING.to_string(),
                    subject: rel_text,
                    detail: "file inside the plan output was not produced by the plan".to_string(),
                });
            }
        }
    }
}

fn hash_file(path: &Path, buffer_bytes: usize) -> std::io::Result<String> {
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

// Verification exercises executed outputs, and execution refuses
// fail-closed off Windows until POSIX write safety exists.
#[cfg(all(test, windows))]
mod tests {
    use df_domain::{ProfileRef, Project, SourceRoot};
    use df_executor::{execute_plan, ExecuteOptions};
    use df_hash::{hash_project, HashOptions};
    use df_planner::{analyze_project, approve_plan, create_plan};
    use df_scan::{scan_project, ScanOptions};

    use super::*;

    struct Fixture {
        db: Db,
        origin: PathBuf,
        output: PathBuf,
    }

    /// Create a directory junction with `mklink /J`; false when the
    /// environment forbids it, so a test skips *loudly* rather than silently.
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

    fn executed_project(tmp: &Path) -> Fixture {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(origin.join("sub")).unwrap();
        std::fs::write(origin.join("a.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("sub").join("b.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("c.txt"), b"different").unwrap();

        let output = tmp.join("salida");
        let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Prueba verify",
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
        execute_plan(&mut db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        Fixture { db, origin, output }
    }

    #[cfg(windows)]
    fn approved_project_with_file_name(tmp: &Path, file_name: &str) -> Fixture {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        let source = origin.join(file_name);
        std::fs::write(df_fs_safety::extended_for_io(&source), b"long-name payload").unwrap();

        let output = tmp.join("salida");
        let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Prueba nombres largos",
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

    #[cfg(windows)]
    fn copied_file_artefact(db: &Db) -> plans::VerifiableArtefact {
        let project = repository::load_project(db).unwrap();
        let plan = plans::current_plan(db, project.id).unwrap().unwrap();
        plans::verifiable_artefacts(db, plan.id)
            .unwrap()
            .into_iter()
            .find(|artefact| artefact.expected_sha256.is_some())
            .expect("the single source file must produce a verifiable copy")
    }

    /// Regression: the old partial name prepended the complete destination
    /// name, so a perfectly valid 204-unit component became >255 and failed
    /// final before copying a byte.
    #[cfg(windows)]
    #[test]
    fn a_204_unit_file_name_executes_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let file_name = format!("{}.txt", "a".repeat(200));
        assert_eq!(file_name.encode_utf16().count(), 204);
        let mut fx = approved_project_with_file_name(tmp.path(), &file_name);

        let execution =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(execution.failed_final, 0, "{execution:?}");
        assert_eq!(execution.failed_retryable, 0, "{execution:?}");
        let artefact = copied_file_artefact(&fx.db);
        assert_eq!(
            std::fs::read(df_fs_safety::extended_for_io(
                &fx.output.join(&artefact.final_relative_path),
            ))
            .unwrap(),
            b"long-name payload"
        );

        let verification =
            verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(
            verification.verdict, "COMPLETED",
            "{:?}",
            verification.findings
        );
    }

    /// At the NTFS component boundary, a conflicting destination needs both a
    /// bounded collision name and a short partial. The squatted file must stay
    /// byte-for-byte untouched, while the verified copy lands under the
    /// deterministic suffix with its extension preserved.
    #[cfg(windows)]
    #[test]
    fn a_255_unit_collision_executes_without_overwrite_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let file_name = format!("{}.txt", "b".repeat(251));
        assert_eq!(file_name.encode_utf16().count(), 255);
        let mut fx = approved_project_with_file_name(tmp.path(), &file_name);

        let project = repository::load_project(&fx.db).unwrap();
        let plan = plans::current_plan(&fx.db, project.id).unwrap().unwrap();
        let planned_relative = plans::manifest(&fx.db, plan.id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.expected_sha256.is_some())
            .and_then(|entry| entry.destination_relative_path)
            .expect("the file copy must have a planned destination");
        let squatted = fx.output.join(&planned_relative);
        std::fs::create_dir_all(df_fs_safety::extended_for_io(squatted.parent().unwrap())).unwrap();
        std::fs::write(
            df_fs_safety::extended_for_io(&squatted),
            b"do not overwrite",
        )
        .unwrap();

        let execution =
            execute_plan(&mut fx.db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        assert_eq!(execution.failed_final, 0, "{execution:?}");
        assert_eq!(execution.failed_retryable, 0, "{execution:?}");
        assert_eq!(
            std::fs::read(df_fs_safety::extended_for_io(&squatted)).unwrap(),
            b"do not overwrite",
            "the collision path must never be replaced"
        );

        let artefact = copied_file_artefact(&fx.db);
        assert_ne!(artefact.final_relative_path, planned_relative);
        let final_name = Path::new(&artefact.final_relative_path)
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert!(final_name.contains("~df-"), "{final_name}");
        assert!(final_name.ends_with(".txt"), "{final_name}");
        assert!(final_name.encode_utf16().count() <= 255, "{final_name}");
        assert_eq!(
            std::fs::read(df_fs_safety::extended_for_io(
                &fx.output.join(&artefact.final_relative_path),
            ))
            .unwrap(),
            b"long-name payload"
        );

        // The squatter was test-owned and has already proved no-overwrite.
        // Remove it so independent verification judges only plan artefacts.
        std::fs::remove_file(df_fs_safety::extended_for_io(&squatted)).unwrap();
        let verification =
            verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(
            verification.verdict, "COMPLETED",
            "{:?}",
            verification.findings
        );
    }

    /// Threat T7 / P0-2: a junction planted after execution must make the
    /// verification FAIL, and the walk must not read a single byte outside the
    /// output root.
    #[cfg(windows)]
    #[test]
    fn a_junction_planted_after_execution_fails_verification_without_reading_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());

        // A directory full of secrets that the verifier must never touch.
        let outside = tmp.path().join("fuera");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secreto.txt"), b"no me leas").unwrap();

        // Plant salida\origen\atajo -> fuera, after a clean execution.
        let planted = fx.output.join("origen").join("atajo");
        if !make_junction(&planted, &outside) {
            eprintln!("SKIP: this environment cannot create junctions (mklink /J failed)");
            return;
        }

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();

        assert_eq!(outcome.verdict, "FAILED", "{:?}", outcome.findings);
        assert_eq!(outcome.state, "FAILED");
        assert!(
            outcome
                .findings
                .iter()
                .any(|f| f.kind == "OUTPUT_REPARSE_POINT" && f.severity == "PROBLEM"),
            "expected an OUTPUT_REPARSE_POINT problem, got {:?}",
            outcome.findings
        );
        // The proof it did not follow: the file behind the junction is never
        // mentioned in any finding.
        assert!(
            !outcome
                .findings
                .iter()
                .any(|f| f.subject.contains("secreto")),
            "the verifier walked through the junction and saw {:?}",
            outcome.findings
        );
    }

    /// Deny/restore read access on a directory via `icacls`. Returns false
    /// when the environment does not allow it, so the test skips loudly.
    #[cfg(windows)]
    fn set_read_denied(path: &Path, denied: bool) -> bool {
        let user = match std::env::var("USERNAME") {
            Ok(user) if !user.is_empty() => user,
            _ => return false,
        };
        let mut cmd = std::process::Command::new("icacls");
        cmd.arg(path);
        if denied {
            cmd.arg("/deny").arg(format!("{user}:(RX)"));
        } else {
            cmd.arg("/remove:d").arg(&user);
        }
        matches!(
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
            Ok(s) if s.success()
        )
    }

    /// P0-2: a directory we cannot read is a PROBLEM, never a silent
    /// `continue` that would let an uninspected output pass as verified.
    #[cfg(windows)]
    #[test]
    fn an_unreadable_subtree_is_reported_instead_of_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        let sub = fx.output.join("origen").join("sub");

        if !set_read_denied(&sub, true) {
            eprintln!("SKIP: icacls could not deny read access in this environment");
            return;
        }
        // Confirm the denial actually bites; if not, we would be asserting on
        // a test that proves nothing.
        if std::fs::read_dir(&sub).is_ok() {
            let _ = set_read_denied(&sub, false);
            eprintln!("SKIP: the read denial had no effect (elevated session?)");
            return;
        }

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default());
        // Always restore access before asserting, so the temp dir can be
        // cleaned up even when an assertion fails.
        let _ = set_read_denied(&sub, false);
        let outcome = outcome.unwrap();

        assert_eq!(outcome.verdict, "FAILED", "{:?}", outcome.findings);
        assert!(
            outcome
                .findings
                .iter()
                .any(|f| f.kind == "OUTPUT_SUBTREE_UNREADABLE" && f.severity == "PROBLEM"),
            "an output we cannot read must be a PROBLEM, got {:?}",
            outcome.findings
        );
    }

    /// Threat T5 / P0-3: the manifest is what approval signed, so editing it
    /// offline must fail verification. The triggers block the honest paths, so
    /// we drop them first to simulate someone with raw access to the file.
    #[test]
    fn tampering_the_execution_manifest_fails_verification() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());

        // (A clean run of this same fixture verifying as COMPLETED is covered
        // by `a_clean_execution_verifies_as_completed`; verifying here first
        // would move the project past EXECUTED and make the second pass
        // invalid for a reason unrelated to tampering.)

        // Forge the expected hash of one entry, bypassing the immutability
        // triggers the way an offline attacker with the .sqlite would.
        fx.db
            .conn_for_tests()
            .execute_batch("DROP TRIGGER execution_manifest_no_update;")
            .unwrap();
        let changed = fx
            .db
            .conn_for_tests()
            .execute(
                "UPDATE execution_manifest SET expected_sha256 = ?1
                 WHERE expected_sha256 IS NOT NULL",
                [&"f".repeat(64)],
            )
            .unwrap();
        assert!(changed > 0, "the test must actually tamper with something");

        // Verify: the manifest no longer hashes to what approval recorded.
        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "FAILED", "{:?}", outcome.findings);
        assert!(
            outcome
                .findings
                .iter()
                .any(|f| f.kind == "PLAN_TAMPERED" && f.severity == "PROBLEM"),
            "expected PLAN_TAMPERED, got {:?}",
            outcome.findings
        );
    }

    /// P0-3: the immutability triggers refuse the ordinary paths outright.
    #[test]
    fn the_execution_manifest_rejects_update_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let fx = executed_project(tmp.path());
        assert!(
            fx.db
                .conn_for_tests()
                .execute("UPDATE execution_manifest SET expected_sha256 = NULL", [])
                .is_err(),
            "the manifest must reject UPDATE"
        );
        assert!(
            fx.db
                .conn_for_tests()
                .execute("DELETE FROM execution_manifest", [])
                .is_err(),
            "the manifest must reject DELETE"
        );
    }

    #[test]
    fn a_clean_execution_verifies_as_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "COMPLETED", "{:?}", outcome.findings);
        assert_eq!(outcome.state, "COMPLETED");
        assert_eq!(outcome.problems, 0);
        assert_eq!(outcome.warnings, 0);
        // 3 copies + 2 directories (origen, origen\sub) re-checked.
        assert_eq!(outcome.checked, 5);

        let project = repository::load_project(&fx.db).unwrap();
        let events = repository::list_events(&fx.db, project.id).unwrap();
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"VERIFICATION_COMPLETED"));
        df_ledger::verify_chain(&events).expect("ledger stays valid");
    }

    #[test]
    fn a_corrupted_copy_fails_verification() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        std::fs::write(fx.output.join("origen").join("a.txt"), b"corrupted!").unwrap();

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "FAILED");
        assert_eq!(outcome.state, "FAILED");
        assert!(outcome
            .findings
            .iter()
            .any(|f| f.kind == "HASH_MISMATCH" && f.subject.ends_with("a.txt")));
    }

    #[test]
    fn a_missing_copy_fails_verification() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        std::fs::remove_file(fx.output.join("origen").join("c.txt")).unwrap();

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "FAILED");
        assert!(outcome
            .findings
            .iter()
            .any(|f| f.kind == "MISSING_DESTINATION"));
    }

    #[test]
    fn an_untracked_file_in_the_output_is_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        std::fs::write(fx.output.join("origen").join("intruso.txt"), b"?").unwrap();

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "COMPLETED_WITH_WARNINGS");
        assert_eq!(outcome.state, "COMPLETED_WITH_WARNINGS");
        assert!(outcome
            .findings
            .iter()
            .any(|f| f.kind == "UNTRACKED_FILE" && f.subject.ends_with("intruso.txt")));
    }

    #[test]
    fn an_origin_changed_after_execution_is_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        std::fs::write(fx.origin.join("c.txt"), b"changed after execution").unwrap();

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "COMPLETED_WITH_WARNINGS");
        assert!(outcome
            .findings
            .iter()
            .any(|f| f.kind == "ORIGIN_CHANGED" && f.subject == "c.txt"));
    }

    #[test]
    fn a_leftover_partial_fails_verification() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fx = executed_project(tmp.path());
        std::fs::write(
            fx.output.join("origen").join(format!(
                "{PARTIAL_FILE_PREFIX}{}-{}",
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4()
            )),
            b"leftover",
        )
        .unwrap();

        let outcome = verify_project(&mut fx.db, Actor::Test, &VerifyOptions::default()).unwrap();
        assert_eq!(outcome.verdict, "FAILED");
        assert!(outcome
            .findings
            .iter()
            .any(|f| f.kind == "PARTIAL_LEFTOVER"));
    }

    #[test]
    fn verification_requires_the_executed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = Project::new(
            "Sin ejecutar",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        repository::create_project(&mut db, &project, &[], Actor::Test).unwrap();
        let err = verify_project(&mut db, Actor::Test, &VerifyOptions::default()).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }
}
