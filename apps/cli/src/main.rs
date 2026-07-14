//! `dataforge` — command line client of the DataForge engine.
//!
//! Milestone 0.1 scope: `project create/status`, `scan`, `hash`,
//! `report duplicates` and `audit verify`. The CLI contains no engine
//! logic; everything goes through `df-facade` (RFC-0001 rules 16/17).
//! Exit codes follow RFC-0001 §33.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use df_domain::Actor;
use df_error::DfResult;
use df_facade::{
    AnalyzeOutcome, ApproveOutcome, AuditReport, CreateProjectRequest, DuplicateReport,
    ExecuteOutcome, HashOutcome, PlanOutcome, PlanValidationReport, ProjectStatus, ScanOutcome,
    TreeCloneReport, VerifyOutcome,
};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "dataforge",
    version,
    about = "Open-source document reconstruction engine (local-first)",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Emit machine-readable JSON instead of human text.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Manage reconstruction projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Inventory the source roots into a new snapshot (read-only).
    Scan {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Compute BLAKE3 + SHA-256 for every scanned file. Resumable.
    Hash {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Analyse the hashed snapshot (exact duplicate sets).
    Analyze {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Manage reconstruction plans.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// Execute the approved plan (verified copy). Resumable.
    Execute {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Verify the executed plan from primary evidence.
    Verify {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Evidence reports derived from the inventory.
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Audit ledger operations.
    Audit {
        #[command(subcommand)]
        command: AuditCommand,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Create a new project directory with its SQLite state.
    Create {
        /// Human name of the project.
        #[arg(long)]
        name: String,
        /// Directory that will hold the project (must be new or empty).
        #[arg(long)]
        path: PathBuf,
        /// Where verified copies will be produced in later milestones.
        /// Must not overlap the project directory or any source root.
        #[arg(long)]
        output_root: PathBuf,
        /// Where audit material will be written (default: <path>/audit).
        #[arg(long)]
        audit_root: Option<PathBuf>,
        /// Origin directory to register (repeatable). Origins are read-only.
        #[arg(long = "source")]
        source: Vec<PathBuf>,
        /// Profile name.
        #[arg(long, default_value = "generic")]
        profile: String,
    },
    /// Show the state, roots, ledger summary and integrity of a project.
    Status {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Generate a full-coverage plan for the analysed snapshot.
    Create {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Re-check the plan invariants (destinations, collisions, coverage).
    Validate {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Approve and freeze the plan (canonical SHA-256).
    Approve {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ReportCommand {
    /// Exact duplicates (same size, same SHA-256) of the latest snapshot.
    Duplicates {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Exact tree clones (folders with byte-for-byte identical subtrees).
    TreeClones {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum AuditCommand {
    /// Cryptographically verify the audit ledger chain.
    Verify {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

/// Everything the CLI can print, one variant per command family.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Output {
    Status(Box<ProjectStatus>),
    Scan(ScanOutcome),
    Hash(HashOutcome),
    Analyze(AnalyzeOutcome),
    Plan(PlanOutcome),
    PlanValidation(PlanValidationReport),
    Approve(ApproveOutcome),
    Execute(ExecuteOutcome),
    Verify(VerifyOutcome),
    Duplicates(DuplicateReport),
    TreeClones(TreeCloneReport),
    Audit(AuditReport),
}

fn run(cli: &Cli) -> DfResult<Output> {
    match &cli.command {
        Command::Project { command } => match command {
            ProjectCommand::Create {
                name,
                path,
                output_root,
                audit_root,
                source,
                profile,
            } => {
                let request = CreateProjectRequest {
                    name: name.clone(),
                    project_dir: path.clone(),
                    output_root: output_root.clone(),
                    audit_root: audit_root.clone(),
                    source_roots: source.clone(),
                    profile: Some(profile.clone()),
                };
                df_facade::create_project(&request, Actor::Cli)
                    .map(Box::new)
                    .map(Output::Status)
            }
            ProjectCommand::Status { path } => df_facade::project_status(path)
                .map(Box::new)
                .map(Output::Status),
        },
        Command::Scan { path } => df_facade::scan_project(path, Actor::Cli).map(Output::Scan),
        Command::Hash { path } => df_facade::hash_project(path, Actor::Cli).map(Output::Hash),
        Command::Analyze { path } => {
            df_facade::analyze_project(path, Actor::Cli).map(Output::Analyze)
        }
        Command::Plan { command } => match command {
            PlanCommand::Create { path } => {
                df_facade::create_plan(path, Actor::Cli).map(Output::Plan)
            }
            PlanCommand::Validate { path } => {
                df_facade::validate_plan(path).map(Output::PlanValidation)
            }
            PlanCommand::Approve { path } => {
                df_facade::approve_plan(path, Actor::Cli).map(Output::Approve)
            }
        },
        Command::Execute { path } => df_facade::execute_plan(path, Actor::Cli).map(Output::Execute),
        Command::Verify { path } => {
            df_facade::verify_project_output(path, Actor::Cli).map(Output::Verify)
        }
        Command::Report { command } => match command {
            ReportCommand::Duplicates { path } => {
                df_facade::duplicate_report(path).map(Output::Duplicates)
            }
            ReportCommand::TreeClones { path } => {
                df_facade::tree_clone_report(path).map(Output::TreeClones)
            }
        },
        Command::Audit { command } => match command {
            AuditCommand::Verify { path } => df_facade::verify_audit(path).map(Output::Audit),
        },
    }
}

fn print_status(status: &ProjectStatus) {
    println!("Project   : {} ({})", status.name, status.project_id);
    println!("State     : {}", status.state);
    println!("Profile   : {}", status.profile);
    println!("Created   : {}", status.created_at);
    println!("Updated   : {}", status.updated_at);
    println!("Directory : {}", status.project_dir);
    println!("Output    : {}", status.output_root);
    println!("Audit     : {}", status.audit_root);
    if status.source_roots.is_empty() {
        println!("Sources   : (none registered)");
    } else {
        println!("Sources   :");
        for root in &status.source_roots {
            println!("  - {} [{}] read-only", root.absolute_path, root.filesystem);
        }
    }
    if let (Some(snapshot), Some(inv)) = (&status.latest_snapshot_id, &status.inventory) {
        println!("Snapshot  : {snapshot}");
        println!(
            "Inventory : {} file(s), {} folder(s), {} byte(s), {} error(s), {} reparse point(s)",
            inv.files, inv.folders, inv.bytes, inv.scan_errors, inv.reparse_points
        );
        println!(
            "Hashing   : {} done, {} pending, {} failed, {} source-changed",
            inv.hash_done, inv.hash_pending, inv.hash_failed, inv.hash_source_changed
        );
    }
    match &status.last_event {
        Some(event) => println!(
            "Ledger    : {} event(s), last {} at {} by {}",
            status.event_count, event.event_type, event.timestamp, event.actor
        ),
        None => println!("Ledger    : empty"),
    }
    if let Some(integrity) = &status.integrity {
        if integrity.is_ok() {
            println!("Integrity : OK (database, foreign keys, migrations, ledger)");
        } else {
            println!("Integrity : FAILED");
            for problem in &integrity.problems {
                println!("  ! {problem}");
            }
        }
    }
}

fn print_scan(outcome: &ScanOutcome) {
    println!("Snapshot  : {}", outcome.snapshot_id);
    println!("Scan run  : {}", outcome.scan_run_id);
    println!("Files     : {}", outcome.files);
    println!("Folders   : {}", outcome.folders);
    println!("Bytes     : {}", outcome.bytes);
    println!("Errors    : {}", outcome.errors);
    println!(
        "Reparse   : {} (recorded, not followed)",
        outcome.reparse_points
    );
    println!("State     : {}", outcome.state);
    if outcome.cancelled {
        println!("Cancelled : scan stopped early; run `dataforge scan` again to restart");
    }
}

fn print_hash(outcome: &HashOutcome) {
    println!("Snapshot        : {}", outcome.snapshot_id);
    println!("Hashed          : {}", outcome.hashed);
    println!("Failed          : {}", outcome.failed);
    println!("Source changed  : {}", outcome.source_changed);
    println!("Pending         : {}", outcome.pending);
    println!("State           : {}", outcome.state);
    if outcome.cancelled {
        println!("Cancelled       : run `dataforge hash` again to resume");
    }
}

fn print_analyze(outcome: &AnalyzeOutcome) {
    println!("Snapshot         : {}", outcome.snapshot_id);
    println!("Duplicate sets   : {}", outcome.duplicate_sets);
    println!("Folder signatures: {}", outcome.folder_signatures);
    println!("Tree clone sets  : {}", outcome.tree_clone_sets);
    println!("State            : {}", outcome.state);
}

fn print_plan(outcome: &PlanOutcome) {
    println!("Plan        : {} (v{})", outcome.plan_id, outcome.version);
    println!("Snapshot    : {}", outcome.snapshot_id);
    println!("Operations  : {}", outcome.operations);
    println!("  copies      : {}", outcome.copies);
    println!("  directories : {}", outcome.directories);
    println!("  no action   : {}", outcome.no_action);
    println!("  blocked     : {}", outcome.blocked);
    println!("State       : {}", outcome.state);
    println!("Next        : review it, then `dataforge plan approve`");
}

fn print_plan_validation(report: &PlanValidationReport) {
    println!("Plan       : {} (v{})", report.plan_id, report.version);
    println!("Status     : {}", report.status);
    println!("Operations : {}", report.operations);
    if report.ok {
        println!("Validation : OK (destinations, collisions, coverage)");
    } else {
        println!("Validation : FAILED");
        for problem in &report.problems {
            println!("  ! {problem}");
        }
    }
}

fn print_approve(outcome: &ApproveOutcome) {
    println!("Plan       : {} (v{})", outcome.plan_id, outcome.version);
    println!("Approved   : {} operation(s)", outcome.operations_approved);
    println!("Plan hash  : {}", outcome.serialized_sha256);
    println!("State      : {}", outcome.state);
}

fn print_execute(outcome: &ExecuteOutcome) {
    println!("Plan            : {}", outcome.plan_id);
    println!("Completed       : {}", outcome.completed);
    println!("Failed final    : {}", outcome.failed_final);
    println!("Failed retryable: {}", outcome.failed_retryable);
    println!("Pending         : {}", outcome.pending);
    println!("Bytes copied    : {}", outcome.bytes_copied);
    println!("State           : {}", outcome.state);
    if outcome.cancelled || outcome.pending > 0 || outcome.failed_retryable > 0 {
        println!("Next            : run `dataforge execute` again to resume");
    }
}

fn print_verify(outcome: &VerifyOutcome) {
    println!("Verification : {}", outcome.verification_run_id);
    println!("Plan         : {}", outcome.plan_id);
    println!("Checked      : {} artefact(s)", outcome.checked);
    println!("Verdict      : {}", outcome.verdict);
    println!("State        : {}", outcome.state);
    if !outcome.findings.is_empty() {
        println!("Findings     :");
        for finding in &outcome.findings {
            println!(
                "  [{}] {} — {}: {}",
                finding.severity, finding.kind, finding.subject, finding.detail
            );
        }
    }
}

fn print_duplicates(report: &DuplicateReport) {
    println!("Snapshot        : {}", report.snapshot_id);
    println!("Duplicate sets  : {}", report.sets.len());
    println!("Redundant files : {}", report.redundant_files);
    println!("Redundant bytes : {}", report.redundant_bytes);
    for set in &report.sets {
        println!();
        println!("  sha256 {} ({} bytes)", set.sha256, set.size_bytes);
        for path in &set.occurrences {
            println!("    - {path}");
        }
    }
    if report.sets.is_empty() {
        println!("No exact duplicates found.");
    }
}

fn print_tree_clones(report: &TreeCloneReport) {
    println!("Snapshot        : {}", report.snapshot_id);
    println!("Clone sets      : {}", report.sets.len());
    println!("Cloned folders  : {}", report.cloned_folders);
    println!("Redundant bytes : {}", report.redundant_bytes);
    for set in &report.sets {
        println!();
        println!(
            "  {} — {} file(s), {} bytes",
            set.relationship.as_str(),
            set.subtree_files,
            set.subtree_bytes
        );
        for folder in &set.folders {
            println!("    - {folder}");
        }
    }
    if report.sets.is_empty() {
        println!("No exact tree clones found.");
    }
}

fn print_audit(report: &AuditReport) {
    println!("Project : {}", report.project_id);
    println!("Events  : {}", report.event_count);
    if report.ledger_ok {
        println!("Ledger  : OK (hash chain verified)");
    } else {
        println!("Ledger  : FAILED");
        if let Some(problem) = &report.problem {
            println!("  ! {problem}");
        }
    }
}

fn print_human(output: &Output) {
    match output {
        Output::Status(status) => print_status(status),
        Output::Scan(outcome) => print_scan(outcome),
        Output::Hash(outcome) => print_hash(outcome),
        Output::Analyze(outcome) => print_analyze(outcome),
        Output::Plan(outcome) => print_plan(outcome),
        Output::PlanValidation(report) => print_plan_validation(report),
        Output::Approve(outcome) => print_approve(outcome),
        Output::Execute(outcome) => print_execute(outcome),
        Output::Verify(outcome) => print_verify(outcome),
        Output::Duplicates(report) => print_duplicates(report),
        Output::TreeClones(report) => print_tree_clones(report),
        Output::Audit(report) => print_audit(report),
    }
}

/// RFC-0001 §33 exit code for a *successful* command whose result still
/// signals a problem (failed integrity, broken ledger, partial hash).
fn verdict_exit_code(output: &Output) -> i32 {
    match output {
        Output::Status(status) => {
            if status.integrity.as_ref().is_some_and(|r| !r.is_ok()) {
                4
            } else {
                0
            }
        }
        Output::Audit(report) => {
            if report.ledger_ok {
                0
            } else {
                4
            }
        }
        Output::Scan(outcome) => {
            if outcome.cancelled || outcome.errors > 0 {
                3
            } else {
                0
            }
        }
        Output::Hash(outcome) => {
            if outcome.cancelled
                || outcome.pending > 0
                || outcome.failed > 0
                || outcome.source_changed > 0
            {
                3
            } else {
                0
            }
        }
        Output::Analyze(_) => 0,
        Output::Plan(outcome) => {
            if outcome.blocked > 0 {
                3
            } else {
                0
            }
        }
        Output::PlanValidation(report) => {
            if report.ok {
                0
            } else {
                2
            }
        }
        Output::Approve(_) => 0,
        Output::Execute(outcome) => {
            if outcome.cancelled
                || outcome.pending > 0
                || outcome.failed_retryable > 0
                || outcome.failed_final > 0
            {
                3
            } else {
                0
            }
        }
        Output::Verify(outcome) => {
            if outcome.verdict == "FAILED" {
                4
            } else {
                0
            }
        }
        Output::Duplicates(_) => 0,
        Output::TreeClones(_) => 0,
    }
}

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(output) => {
            if cli.json {
                match serde_json::to_string_pretty(&output) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("error: failed to serialize output: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                print_human(&output);
            }
            let code = verdict_exit_code(&output);
            if code != 0 {
                std::process::exit(code);
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(error.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn create_then_status_through_the_facade() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let output = tmp.path().join("salida");

        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "Prueba CLI",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            output.to_str().unwrap(),
        ]);
        let created = match run(&create).expect("create succeeds") {
            Output::Status(status) => status,
            _ => panic!("create returns a status"),
        };
        assert_eq!(created.state, "CREATED");
        assert_eq!(created.event_count, 1);

        let status = Cli::parse_from([
            "dataforge",
            "project",
            "status",
            "--path",
            project_dir.to_str().unwrap(),
        ]);
        let report = match run(&status).expect("status succeeds") {
            Output::Status(status) => status,
            _ => panic!("status returns a status"),
        };
        assert_eq!(report.project_id, created.project_id);
        assert!(report.integrity.as_ref().expect("integrity ran").is_ok());
    }

    #[test]
    fn full_pipeline_scan_hash_report_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("x.txt"), b"dup").unwrap();
        std::fs::write(origin.join("y.txt"), b"dup").unwrap();

        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "Pipeline",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            tmp.path().join("salida").to_str().unwrap(),
            "--source",
            origin.to_str().unwrap(),
        ]);
        run(&create).expect("create succeeds");

        let path = project_dir.to_str().unwrap();
        let scan =
            run(&Cli::parse_from(["dataforge", "scan", "--path", path])).expect("scan succeeds");
        match &scan {
            Output::Scan(outcome) => {
                assert_eq!(outcome.files, 2);
                assert_eq!(outcome.state, "SCANNED");
                assert_eq!(verdict_exit_code(&scan), 0);
            }
            _ => panic!("scan returns a scan outcome"),
        }

        let hash =
            run(&Cli::parse_from(["dataforge", "hash", "--path", path])).expect("hash succeeds");
        match &hash {
            Output::Hash(outcome) => {
                assert_eq!(outcome.hashed, 2);
                assert_eq!(outcome.state, "HASHED");
                assert_eq!(verdict_exit_code(&hash), 0);
            }
            _ => panic!("hash returns a hash outcome"),
        }

        let dupes = run(&Cli::parse_from([
            "dataforge",
            "report",
            "duplicates",
            "--path",
            path,
        ]))
        .expect("report succeeds");
        match &dupes {
            Output::Duplicates(report) => {
                assert_eq!(report.sets.len(), 1);
                assert_eq!(report.redundant_files, 1);
            }
            _ => panic!("report returns duplicates"),
        }

        let analyze = run(&Cli::parse_from(["dataforge", "analyze", "--path", path]))
            .expect("analyze succeeds");
        match &analyze {
            Output::Analyze(outcome) => assert_eq!(outcome.state, "ANALYZED"),
            _ => panic!("analyze returns an analyze outcome"),
        }

        let plan = run(&Cli::parse_from([
            "dataforge",
            "plan",
            "create",
            "--path",
            path,
        ]))
        .expect("plan create succeeds");
        match &plan {
            Output::Plan(outcome) => {
                assert_eq!(outcome.copies, 2);
                assert_eq!(outcome.state, "PLAN_READY");
                assert_eq!(verdict_exit_code(&plan), 0);
            }
            _ => panic!("plan create returns a plan outcome"),
        }

        let validation = run(&Cli::parse_from([
            "dataforge",
            "plan",
            "validate",
            "--path",
            path,
        ]))
        .expect("plan validate succeeds");
        match &validation {
            Output::PlanValidation(report) => assert!(report.ok, "{:?}", report.problems),
            _ => panic!("plan validate returns a validation report"),
        }

        let approve = run(&Cli::parse_from([
            "dataforge",
            "plan",
            "approve",
            "--path",
            path,
        ]))
        .expect("plan approve succeeds");
        match &approve {
            Output::Approve(outcome) => assert_eq!(outcome.state, "PLAN_APPROVED"),
            _ => panic!("plan approve returns an approve outcome"),
        }

        let execute = run(&Cli::parse_from(["dataforge", "execute", "--path", path]))
            .expect("execute succeeds");
        match &execute {
            Output::Execute(outcome) => {
                assert_eq!(outcome.state, "EXECUTED");
                assert_eq!(verdict_exit_code(&execute), 0);
            }
            _ => panic!("execute returns an execute outcome"),
        }

        let verify = run(&Cli::parse_from(["dataforge", "verify", "--path", path]))
            .expect("verify succeeds");
        match &verify {
            Output::Verify(outcome) => {
                assert_eq!(outcome.verdict, "COMPLETED", "{:?}", outcome.findings);
                assert_eq!(verdict_exit_code(&verify), 0);
            }
            _ => panic!("verify returns a verify outcome"),
        }

        // The verified copy landed in the output root.
        assert_eq!(
            std::fs::read(tmp.path().join("salida").join("origen").join("x.txt")).unwrap(),
            b"dup"
        );

        let audit = run(&Cli::parse_from([
            "dataforge",
            "audit",
            "verify",
            "--path",
            path,
        ]))
        .expect("audit succeeds");
        match &audit {
            Output::Audit(report) => {
                assert!(report.ledger_ok);
                assert_eq!(verdict_exit_code(&audit), 0);
            }
            _ => panic!("audit returns an audit report"),
        }
    }

    #[test]
    fn status_of_missing_project_maps_to_generic_failure_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let status = Cli::parse_from([
            "dataforge",
            "project",
            "status",
            "--path",
            tmp.path().join("nada").to_str().unwrap(),
        ]);
        let err = run(&status).unwrap_err();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn invalid_creation_maps_to_validation_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "  ",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            tmp.path().join("salida").to_str().unwrap(),
        ]);
        let err = run(&create).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn scanning_before_creating_sources_is_a_validation_error() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "Sin fuentes",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            tmp.path().join("salida").to_str().unwrap(),
        ]);
        run(&create).expect("create succeeds");
        let scan = Cli::parse_from(["dataforge", "scan", "--path", project_dir.to_str().unwrap()]);
        let err = run(&scan).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }
}
