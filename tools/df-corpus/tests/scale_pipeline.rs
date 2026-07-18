//! Full-pipeline runs over a generated corpus (M0.1 acceptance criteria
//! 1 "100.000 archivos" and 10 "corpus base"; RFC-0001 §40).
//!
//! Two flavours of the same drive:
//! - [`a_small_corpus_survives_the_full_pipeline`] runs in CI on every push
//!   with a few hundred files;
//! - [`scale_full_pipeline`] is `#[ignore]`d and sized by `DF_CORPUS_FILES`
//!   (default 100 000). Run the real acceptance check with:
//!
//!   ```powershell
//!   cargo test -p df-corpus --release -- --ignored scale --nocapture
//!   ```

use std::io::Read;
use std::path::Path;
use std::time::Instant;

use df_corpus::{generate, CorpusSpec};
use df_domain::Actor;
use df_facade::{CreateProjectRequest, DuplicatePolicy};
use sha2::{Digest, Sha256};

struct PipelineReport {
    generated_files: u64,
    scanned_files: u64,
    hashed: u64,
    verify_verdict: String,
    ledger_events: u64,
}

/// Generate a corpus of `files` files and drive it through the whole engine:
/// create → scan → hash → analyze → plan → approve → execute → verify.
///
/// Asserts the invariants that make the run meaningful (counts match, origin
/// untouched, verification COMPLETED, ledger valid) and returns the numbers
/// for reporting.
fn run_pipeline(base: &Path, files: u64) -> PipelineReport {
    let origin = base.join("origen");
    let spec = CorpusSpec {
        files,
        ..CorpusSpec::default()
    };

    let started = Instant::now();
    let corpus = generate(&origin, &spec).expect("corpus generation");
    eprintln!(
        "[corpus] {} files, {} folders, {} bytes in {:.1?}",
        corpus.files_written,
        corpus.folders_created,
        corpus.bytes_written,
        started.elapsed()
    );
    assert_eq!(corpus.files_written, files);

    let stage = Instant::now();
    let origin_before = tree_fingerprint(&origin);
    eprintln!(
        "[origin fingerprint] {} entries in {:.1?}",
        origin_before.len(),
        stage.elapsed()
    );

    let project_dir = base.join("proyecto");
    let request = CreateProjectRequest {
        name: format!("Corpus {files}"),
        project_dir: project_dir.clone(),
        output_root: base.join("salida"),
        audit_root: None,
        source_roots: vec![origin.clone()],
        profile: Some("generic".to_string()),
    };
    df_facade::create_project(&request, Actor::Test).expect("create");

    let stage = Instant::now();
    let scan = df_facade::scan_project(&project_dir, Actor::Test).expect("scan");
    eprintln!(
        "[scan] {} files, {} folders, {} errors in {:.1?}",
        scan.files,
        scan.folders,
        scan.errors,
        stage.elapsed()
    );
    assert_eq!(scan.files, corpus.files_written, "scan must see every file");
    assert_eq!(scan.errors, 0);

    let stage = Instant::now();
    let hash = df_facade::hash_project(&project_dir, Actor::Test).expect("hash");
    eprintln!(
        "[hash] {} hashed, {} failed in {:.1?}",
        hash.hashed,
        hash.failed,
        stage.elapsed()
    );
    assert_eq!(hash.hashed, corpus.files_written);
    assert_eq!(hash.failed + hash.source_changed + hash.pending, 0);

    let stage = Instant::now();
    let analysis = df_facade::analyze_project(&project_dir, Actor::Test).expect("analyze");
    eprintln!(
        "[analyze] {} duplicate sets, {} clone sets, {} partial, {} anomalies in {:.1?}",
        analysis.duplicate_sets,
        analysis.tree_clone_sets,
        analysis.partial_tree_clones,
        analysis.anomalies,
        stage.elapsed()
    );
    assert_eq!(analysis.state, "ANALYZED");
    assert!(
        analysis.duplicate_sets > 0,
        "a 20% duplicate corpus must produce duplicate sets"
    );

    let stage = Instant::now();
    let plan = df_facade::create_plan(&project_dir, Actor::Test, DuplicatePolicy::ReportOnly)
        .expect("plan");
    eprintln!(
        "[plan] {} operations ({} copies) in {:.1?}",
        plan.operations,
        plan.copies,
        stage.elapsed()
    );
    df_facade::approve_plan(&project_dir, Actor::Test).expect("approve");

    // Execution is resumable by design (RFC-0001 §27.4): on a real OS a
    // fraction of operations can fail retryably under IO contention (an
    // antivirus or indexer briefly holding a freshly created file). Retrying
    // is exactly what a user does, so the test does it too — but every retry
    // must make progress, and nothing may fail *final*.
    let stage = Instant::now();
    let mut execution = df_facade::execute_plan(&project_dir, Actor::Test).expect("execute");
    let mut attempts = 1;
    eprintln!(
        "[execute] attempt {attempts}: {} completed, {} retryable, {} final in {:.1?}",
        execution.completed,
        execution.failed_retryable,
        execution.failed_final,
        stage.elapsed()
    );
    while execution.failed_retryable > 0 && attempts < 5 {
        let before = execution.completed;
        let stage = Instant::now();
        execution = df_facade::execute_plan(&project_dir, Actor::Test).expect("execute retry");
        attempts += 1;
        eprintln!(
            "[execute] attempt {attempts}: {} completed, {} retryable, {} final in {:.1?}",
            execution.completed,
            execution.failed_retryable,
            execution.failed_final,
            stage.elapsed()
        );
        assert!(
            execution.completed > before,
            "a retry must make progress ({} -> {})",
            before,
            execution.completed
        );
    }
    assert_eq!(
        execution.failed_final, 0,
        "no operation may fail final on a healthy corpus"
    );
    assert_eq!(
        execution.failed_retryable, 0,
        "retryable failures must drain within {attempts} attempts"
    );
    assert_eq!(execution.state, "EXECUTED");
    assert_eq!(execution.pending, 0);

    let stage = Instant::now();
    let verification = df_facade::verify_project_output(&project_dir, Actor::Test).expect("verify");
    eprintln!(
        "[verify] {} checked, verdict {} in {:.1?}",
        verification.checked,
        verification.verdict,
        stage.elapsed()
    );
    assert_eq!(
        verification.verdict, "COMPLETED",
        "verification findings: {:?}",
        verification.findings
    );

    // The origin is byte-for-byte untouched after the whole pipeline.
    assert_eq!(
        origin_before,
        tree_fingerprint(&origin),
        "the origin must not change (RFC-0001 rule 1)"
    );

    let audit = df_facade::verify_audit(&project_dir).expect("audit");
    assert!(audit.ledger_ok, "ledger must verify after the full run");

    PipelineReport {
        generated_files: corpus.files_written,
        scanned_files: scan.files,
        hashed: hash.hashed,
        verify_verdict: verification.verdict,
        ledger_events: audit.event_count,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EntryIdentity {
    Directory,
    File { size: u64, sha256: [u8; 32] },
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct TreeEntryIdentity {
    relative_path: String,
    identity: EntryIdentity,
}

fn portable_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256_file(path: &Path) -> [u8; 32] {
    let mut reader = std::fs::File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hasher.finalize().into()
}

/// Exact origin identity: normalized relative path, entry type and SHA-256.
fn tree_fingerprint(root: &Path) -> Vec<TreeEntryIdentity> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let identity = if metadata.is_dir() {
                queue.push(path.clone());
                EntryIdentity::Directory
            } else if metadata.is_file() {
                EntryIdentity::File {
                    size: metadata.len(),
                    sha256: sha256_file(&path),
                }
            } else {
                EntryIdentity::Other
            };
            out.push(TreeEntryIdentity {
                relative_path: portable_relative_path(root, &path),
                identity,
            });
        }
    }
    out.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    out
}

#[test]
fn origin_fingerprint_covers_paths_and_same_length_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("origin");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("a.txt");
    std::fs::write(&file, b"first").unwrap();
    let original = tree_fingerprint(&root);

    std::fs::write(&file, b"other").unwrap();
    assert_ne!(original, tree_fingerprint(&root));

    std::fs::write(&file, b"first").unwrap();
    std::fs::rename(&file, root.join("b.txt")).unwrap();
    assert_ne!(original, tree_fingerprint(&root));
}

/// CI-sized end-to-end drive: fast, but through every phase.
#[test]
fn a_small_corpus_survives_the_full_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let report = run_pipeline(tmp.path(), 300);
    assert_eq!(report.generated_files, 300);
    assert_eq!(report.scanned_files, 300);
    assert_eq!(report.hashed, 300);
    assert_eq!(report.verify_verdict, "COMPLETED");
    assert!(report.ledger_events > 10);
}

/// The M0.1 acceptance run. Ignored by default; size with DF_CORPUS_FILES.
///
/// ```powershell
/// cargo test -p df-corpus --release -- --ignored scale --nocapture
/// ```
#[test]
#[ignore = "scale run: minutes of IO; execute explicitly with --ignored"]
fn scale_full_pipeline() {
    let files: u64 = std::env::var("DF_CORPUS_FILES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);
    let overall = Instant::now();
    // DF_CORPUS_BASE keeps everything (corpus, project DB, output) on disk
    // for post-mortem inspection; the default temp dir cleans up after itself.
    let report = match std::env::var("DF_CORPUS_BASE").ok() {
        Some(base) => {
            let base = std::path::PathBuf::from(base);
            std::fs::create_dir_all(&base).unwrap();
            run_pipeline(&base, files)
        }
        None => {
            let tmp = tempfile::tempdir().unwrap();
            run_pipeline(tmp.path(), files)
        }
    };
    eprintln!(
        "[scale] {} files end-to-end in {:.1?} — verdict {}, {} ledger events",
        report.generated_files,
        overall.elapsed(),
        report.verify_verdict,
        report.ledger_events
    );
}
