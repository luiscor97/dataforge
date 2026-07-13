//! `dataforge` — command line client of the DataForge engine.
//!
//! Milestone 0.0 scope: `project create` and `project status`.
//! The CLI contains no engine logic; everything goes through `df-facade`
//! (RFC-0001 rules 16/17). Exit codes follow RFC-0001 §33.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use df_domain::Actor;
use df_error::DfResult;
use df_facade::{CreateProjectRequest, ProjectStatus};

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

fn run(cli: &Cli) -> DfResult<ProjectStatus> {
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
            }
            ProjectCommand::Status { path } => df_facade::project_status(path),
        },
    }
}

fn print_human(status: &ProjectStatus) {
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

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(status) => {
            let integrity_failed = status
                .integrity
                .as_ref()
                .is_some_and(|report| !report.is_ok());
            if cli.json {
                match serde_json::to_string_pretty(&status) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("error: failed to serialize status: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                print_human(&status);
            }
            // A status whose integrity pass failed is a verification failure.
            if integrity_failed {
                std::process::exit(4);
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
        let created = run(&create).expect("create succeeds");
        assert_eq!(created.state, "CREATED");
        assert_eq!(created.event_count, 1);

        let status = Cli::parse_from([
            "dataforge",
            "project",
            "status",
            "--path",
            project_dir.to_str().unwrap(),
        ]);
        let report = run(&status).expect("status succeeds");
        assert_eq!(report.project_id, created.project_id);
        assert!(report.integrity.expect("integrity ran").is_ok());
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
}
