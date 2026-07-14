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
    let recomputed = df_planner::plan_operations_sha256(&operations);
    match plan.serialized_sha256.as_deref() {
        Some(stored) if stored == recomputed => {}
        stored => findings.push(VerificationFinding {
            kind: "PLAN_TAMPERED".to_string(),
            severity: SEVERITY_PROBLEM.to_string(),
            subject: plan.id.to_string(),
            detail: format!(
                "plan re-serializes to {recomputed} but approval recorded {}",
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
        let destination = project.output_root.join(&artefact.final_relative_path);
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
        let current = std::fs::symlink_metadata(&source).ok().map(|metadata| {
            FileFingerprint {
                size_bytes: metadata.len(),
                modified_at_fs: metadata.modified().ok().map(Into::into),
            }
            .token()
        });
        if current.as_deref() != Some(occurrence.fingerprint.as_str()) {
            findings.push(VerificationFinding {
                kind: "ORIGIN_CHANGED".to_string(),
                severity: SEVERITY_WARNING.to_string(),
                subject: occurrence.relative_path.clone(),
                detail: match current {
                    Some(_) => "source file changed since the snapshot".to_string(),
                    None => "source file is no longer readable".to_string(),
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
fn walk_output(
    output_root: &Path,
    subtree: &Path,
    expected_files: &HashSet<String>,
    expected_dirs: &HashSet<String>,
    findings: &mut Vec<VerificationFinding>,
) {
    let mut queue: Vec<PathBuf> = vec![subtree.to_path_buf()];
    while let Some(dir_rel) = queue.pop() {
        let dir_abs = output_root.join(&dir_rel);
        let Ok(entries) = std::fs::read_dir(&dir_abs) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = dir_rel.join(&name);
            let rel_text = rel.to_string_lossy().into_owned();
            let is_dir = entry.metadata().map(|m| m.is_dir()).unwrap_or(false);
            if is_dir {
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
            if name.contains(".dataforge-partial-") {
                findings.push(VerificationFinding {
                    kind: "PARTIAL_LEFTOVER".to_string(),
                    severity: SEVERITY_PROBLEM.to_string(),
                    subject: rel_text,
                    detail: "a partial file survived execution (RFC-0001 §28.2)".to_string(),
                });
                continue;
            }
            if !expected_files.contains(&rel_text.to_lowercase()) {
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

#[cfg(test)]
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
        create_plan(&mut db, Actor::Test).unwrap();
        approve_plan(&mut db, Actor::Test).unwrap();
        execute_plan(&mut db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        Fixture { db, origin, output }
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
            fx.output
                .join("origen")
                .join(".x.txt.dataforge-partial-deadbeef"),
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
