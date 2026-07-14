//! Versioned exports of DataForge state (RFC-0001 §35, §36).
//!
//! Reports are *exports*, not the source of truth (rule 6): SQLite holds the
//! authoritative state and these functions render a read-only, versioned
//! view of it into the project directory. Regenerating an export overwrites
//! the previous file of the same name — that is intended and does not breach
//! the no-overwrite rule, which protects the origin and the document output,
//! never the regenerable `plans/` and `reports/` exports.
//!
//! Every JSON export carries the common header of §36.1 so a reader can tell
//! what it is looking at and which generator produced it.

use std::path::Path;

use df_db::{inventory, plans, repository, Db};
use df_error::{DfError, DfResult};
use serde::Serialize;

/// Version of DataForge that stamps every export.
pub const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Schema version of the export formats (independent SemVer, §36.2).
pub const SCHEMA_VERSION: &str = "1.0.0";

const SCHEMA_PLAN: &str = "dataforge.plan";
const SCHEMA_REPORT: &str = "dataforge.report";
const SCHEMA_DUPLICATES: &str = "dataforge.duplicates";

/// Result of writing one export file.
#[derive(Debug, Clone, Serialize)]
pub struct ReportFile {
    /// Absolute path of the file written.
    pub path: String,
    /// Schema identifier of the content (`dataforge.plan`, …).
    pub schema: String,
    pub bytes: u64,
}

/// Common versioned header of every JSON export (§36.1).
#[derive(Debug, Serialize)]
struct Header {
    schema: String,
    schema_version: String,
    project_id: String,
    snapshot_id: String,
    created_at: String,
    generator_version: String,
}

fn header(schema: &str, project_id: &str, snapshot_id: &str) -> Header {
    Header {
        schema: schema.to_string(),
        schema_version: SCHEMA_VERSION.to_string(),
        project_id: project_id.to_string(),
        snapshot_id: snapshot_id.to_string(),
        created_at: df_ledger::canonical_timestamp(chrono::Utc::now()),
        generator_version: GENERATOR_VERSION.to_string(),
    }
}

/// Write bytes to `path` atomically: a sibling temp file is flushed and then
/// renamed over the destination, so a crash never leaves a half-written
/// export. The destination is a regenerable export, so replacing it is safe.
fn write_atomic(path: &Path, contents: &[u8]) -> DfResult<u64> {
    let parent = path
        .parent()
        .ok_or_else(|| DfError::Validation(format!("`{}` has no parent", path.display())))?;
    std::fs::create_dir_all(parent).map_err(|e| DfError::io(parent, e))?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".to_string());
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));

    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp).map_err(|e| DfError::io(&tmp, e))?;
        file.write_all(contents).map_err(|e| DfError::io(&tmp, e))?;
        file.sync_all().map_err(|e| DfError::io(&tmp, e))?;
    }
    // Windows rename fails if the destination exists; replacing an export is
    // legitimate (it is regenerable, not user data).
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| DfError::io(path, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| DfError::io(path, e))?;
    Ok(contents.len() as u64)
}

fn write_json<T: Serialize>(path: &Path, schema: &str, value: &T) -> DfResult<ReportFile> {
    let text =
        serde_json::to_string_pretty(value).map_err(|e| DfError::Serialization(e.to_string()))?;
    let bytes = write_atomic(path, text.as_bytes())?;
    Ok(ReportFile {
        path: path.display().to_string(),
        schema: schema.to_string(),
        bytes,
    })
}

/// Export the current plan of a project to `plans/plan-NNNN.json` (§35).
///
/// `NNNN` is the plan version, so re-exporting the same plan is idempotent
/// and re-planning produces a new file.
pub fn export_plan(db: &Db, project_dir: &Path) -> DfResult<ReportFile> {
    let project = repository::load_project(db)?;
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan to export".to_string()))?;
    let operations = plans::list_operations(db, plan.id)?;

    let mut by_type = std::collections::BTreeMap::new();
    for op in &operations {
        *by_type.entry(op.operation_type.as_str()).or_insert(0u64) += 1;
    }

    let document = serde_json::json!({
        "header": header(SCHEMA_PLAN, &project.id.to_string(), &plan.snapshot_id.to_string()),
        "plan": {
            "id": plan.id.to_string(),
            "version": plan.version,
            "status": plan.status.as_str(),
            "serialized_sha256": plan.serialized_sha256,
            "created_at": df_ledger::canonical_timestamp(plan.created_at),
            "approved_at": plan.approved_at.map(df_ledger::canonical_timestamp),
        },
        "summary": {
            "operations": operations.len(),
            "by_type": by_type,
        },
        "operations": operations.iter().map(|op| serde_json::json!({
            "sequence": op.sequence,
            "operation_type": op.operation_type.as_str(),
            "source_occurrence": op.source_occurrence.map(|id| id.to_string()),
            "content_id": op.content_id.map(|id| id.to_string()),
            "destination_relative_path": op.destination_relative_path,
            "risk": op.risk.as_str(),
            "approval": op.approval.as_str(),
            "execution_state": op.execution_state.as_str(),
            "idempotency_key": op.idempotency_key,
            "reason": op.reason,
        })).collect::<Vec<_>>(),
    });

    let path = project_dir
        .join("plans")
        .join(format!("plan-{:04}.json", plan.version));
    write_json(&path, SCHEMA_PLAN, &document)
}

/// Files written by [`export_verification`].
#[derive(Debug, Clone, Serialize)]
pub struct VerificationExport {
    pub json: ReportFile,
    pub markdown: ReportFile,
}

/// Export the latest verification run of a project to
/// `reports/verification-NNNN.json` plus a human-readable
/// `reports/verification-NNNN.md` (§35). Includes the copy manifest
/// (artefact → SHA-256), which is the auditable proof of the migration.
pub fn export_verification(db: &Db, project_dir: &Path) -> DfResult<VerificationExport> {
    let project = repository::load_project(db)?;
    let plan = plans::current_plan(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no plan".to_string()))?;
    let run = plans::latest_verification_run(db, plan.id)?
        .ok_or_else(|| DfError::Validation("the project has not been verified yet".to_string()))?;
    let artefacts = plans::verifiable_artefacts(db, plan.id)?;

    let manifest: Vec<serde_json::Value> = artefacts
        .iter()
        .map(|a| {
            serde_json::json!({
                "operation_type": a.operation_type.as_str(),
                "destination_relative_path": a.final_relative_path,
                "sha256": a.expected_sha256,
                "size_bytes": a.size_bytes,
            })
        })
        .collect();

    let document = serde_json::json!({
        "header": header(SCHEMA_REPORT, &project.id.to_string(), &plan.snapshot_id.to_string()),
        "plan": {
            "id": plan.id.to_string(),
            "version": plan.version,
            "serialized_sha256": plan.serialized_sha256,
        },
        "verification": {
            "id": run.id.to_string(),
            "verdict": run.verdict,
            "checked": run.checked,
            "problems": run.problems,
            "warnings": run.warnings,
            "started_at": df_ledger::canonical_timestamp(run.started_at),
            "finished_at": df_ledger::canonical_timestamp(run.finished_at),
            "findings": run.findings.iter().map(|f| serde_json::json!({
                "kind": f.kind,
                "severity": f.severity,
                "subject": f.subject,
                "detail": f.detail,
            })).collect::<Vec<_>>(),
        },
        "manifest": manifest,
    });

    let json_path = project_dir
        .join("reports")
        .join(format!("verification-{:04}.json", plan.version));
    let json = write_json(&json_path, SCHEMA_REPORT, &document)?;

    let markdown = render_verification_markdown(&project, &plan, &run, &artefacts);
    let md_path = project_dir
        .join("reports")
        .join(format!("verification-{:04}.md", plan.version));
    let markdown_bytes = write_atomic(&md_path, markdown.as_bytes())?;

    Ok(VerificationExport {
        json,
        markdown: ReportFile {
            path: md_path.display().to_string(),
            schema: SCHEMA_REPORT.to_string(),
            bytes: markdown_bytes,
        },
    })
}

fn render_verification_markdown(
    project: &df_domain::Project,
    plan: &df_domain::Plan,
    run: &plans::VerificationRunRecord,
    artefacts: &[plans::VerifiableArtefact],
) -> String {
    let mut out = String::new();
    out.push_str("# DataForge \u{2014} informe de verificaci\u{f3}n\n\n");
    out.push_str(&format!(
        "- **Proyecto:** {} (`{}`)\n",
        project.name, project.id
    ));
    out.push_str(&format!("- **Plan:** v{} (`{}`)\n", plan.version, plan.id));
    if let Some(sha) = &plan.serialized_sha256 {
        out.push_str(&format!("- **Hash del plan:** `{sha}`\n"));
    }
    out.push_str(&format!("- **Veredicto:** {}\n", run.verdict));
    out.push_str(&format!(
        "- **Comprobados:** {} · **Problemas:** {} · **Avisos:** {}\n",
        run.checked, run.problems, run.warnings
    ));
    out.push_str(&format!(
        "- **Verificado:** {}\n\n",
        df_ledger::canonical_timestamp(run.finished_at)
    ));

    out.push_str("## Hallazgos\n\n");
    if run.findings.is_empty() {
        out.push_str("Ninguno. La copia reproduce el origen sin incidencias.\n\n");
    } else {
        out.push_str("| Severidad | Tipo | Sujeto | Detalle |\n");
        out.push_str("| --- | --- | --- | --- |\n");
        for f in &run.findings {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                f.severity,
                f.kind,
                md_escape(&f.subject),
                md_escape(&f.detail)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Manifiesto de copia\n\n");
    out.push_str("| Destino | SHA-256 | Bytes |\n");
    out.push_str("| --- | --- | --- |\n");
    for a in artefacts {
        if a.operation_type == df_domain::OperationType::CreateDirectory {
            continue;
        }
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            md_escape(&a.final_relative_path),
            a.expected_sha256.as_deref().unwrap_or("-"),
            a.size_bytes
        ));
    }
    out.push_str(&format!(
        "\n_Generado por DataForge {GENERATOR_VERSION} (esquema {SCHEMA_REPORT} {SCHEMA_VERSION}). Exportación regenerable; la fuente de verdad es SQLite._\n"
    ));
    out
}

/// Escape the Markdown table cell separators.
fn md_escape(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

/// Export the exact-duplicate evidence of the latest complete snapshot to
/// `reports/duplicates-<snapshot>.csv` (§15, §35). Evidence only: no action
/// is proposed (§15.2).
pub fn export_duplicates(db: &Db, project_dir: &Path) -> DfResult<ReportFile> {
    let project = repository::load_project(db)?;
    let snapshot = inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let sets = inventory::exact_duplicates(db, snapshot.id)?;

    let mut csv = String::from("set_index,sha256,size_bytes,occurrence_path\n");
    for (index, set) in sets.iter().enumerate() {
        for path in &set.occurrences {
            csv.push_str(&format!(
                "{},{},{},{}\n",
                index + 1,
                set.sha256,
                set.size_bytes,
                csv_escape(path)
            ));
        }
    }

    let snapshot_tag = snapshot.id.to_string();
    let short = snapshot_tag.split('-').next().unwrap_or("snapshot");
    let path = project_dir
        .join("reports")
        .join(format!("duplicates-{short}.csv"));
    let bytes = write_atomic(&path, csv.as_bytes())?;
    Ok(ReportFile {
        path: path.display().to_string(),
        schema: SCHEMA_DUPLICATES.to_string(),
        bytes,
    })
}

/// Quote a CSV field if it contains a comma, quote or newline (RFC 4180).
fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use df_domain::{Actor, ProfileRef, Project, SourceRoot};
    use df_executor::{execute_plan, ExecuteOptions};
    use df_hash::{hash_project, HashOptions};
    use df_planner::{analyze_project, approve_plan, create_plan};
    use df_scan::{scan_project, ScanOptions};

    use super::*;

    /// Drive the full pipeline and return (db, project_dir).
    fn completed_project(tmp: &Path) -> (Db, PathBuf) {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(origin.join("sub")).unwrap();
        std::fs::write(origin.join("a.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("sub").join("b.txt"), b"same bytes").unwrap();
        std::fs::write(origin.join("c.txt"), b"different").unwrap();

        let project_dir = tmp.join("proyecto");
        std::fs::create_dir_all(&project_dir).unwrap();
        let mut db = Db::open(&project_dir.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Informe demo",
            ProfileRef::default(),
            tmp.join("salida"),
            tmp.join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
        analyze_project(&mut db, Actor::Test).unwrap();
        create_plan(&mut db, Actor::Test).unwrap();
        approve_plan(&mut db, Actor::Test).unwrap();
        execute_plan(&mut db, Actor::Test, &ExecuteOptions::default(), None).unwrap();
        df_verifier::verify_project(&mut db, Actor::Test, &df_verifier::VerifyOptions::default())
            .unwrap();
        (db, project_dir)
    }

    #[test]
    fn plan_export_has_versioned_header_and_covers_every_operation() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, project_dir) = completed_project(tmp.path());

        let file = export_plan(&db, &project_dir).unwrap();
        assert!(file.path.ends_with("plan-0001.json"));
        assert_eq!(file.schema, "dataforge.plan");

        let text =
            std::fs::read_to_string(project_dir.join("plans").join("plan-0001.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["header"]["schema"], "dataforge.plan");
        assert_eq!(json["header"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(json["plan"]["status"], "APPROVED");
        // 3 copies + 2 directories.
        assert_eq!(json["operations"].as_array().unwrap().len(), 5);
        assert!(json["operations"][0]["reason"].as_str().is_some());
    }

    #[test]
    fn verification_export_writes_json_and_markdown_with_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, project_dir) = completed_project(tmp.path());

        let export = export_verification(&db, &project_dir).unwrap();
        assert!(export.json.path.ends_with("verification-0001.json"));
        assert!(export.markdown.path.ends_with("verification-0001.md"));

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(project_dir.join("reports").join("verification-0001.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(json["verification"]["verdict"], "COMPLETED");
        // Manifest lists the 3 copied files with their hashes.
        let manifest = json["manifest"].as_array().unwrap();
        let files = manifest
            .iter()
            .filter(|m| m["operation_type"] != "CREATE_DIRECTORY")
            .count();
        assert_eq!(files, 3);
        assert!(manifest
            .iter()
            .all(|m| m["sha256"].is_string() || m["operation_type"] == "CREATE_DIRECTORY"));

        let md = std::fs::read_to_string(project_dir.join("reports").join("verification-0001.md"))
            .unwrap();
        assert!(md.contains("Veredicto:** COMPLETED"));
        assert!(md.contains("Manifiesto de copia"));
    }

    #[test]
    fn duplicates_export_is_csv_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, project_dir) = completed_project(tmp.path());

        let file = export_duplicates(&db, &project_dir).unwrap();
        assert!(file.path.ends_with(".csv"));
        let csv = std::fs::read_to_string(&file.path).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "set_index,sha256,size_bytes,occurrence_path");
        // One duplicate set with two members → header + 2 rows.
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("1,"));
    }

    #[test]
    fn exports_are_regenerable_and_atomic() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, project_dir) = completed_project(tmp.path());
        let first = export_plan(&db, &project_dir).unwrap();
        // Re-exporting the same plan overwrites in place, leaving no temp file.
        let second = export_plan(&db, &project_dir).unwrap();
        assert_eq!(first.path, second.path);
        let temp_files = std::fs::read_dir(project_dir.join("plans"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(temp_files, 0, "no leftover temp files");
    }

    #[test]
    fn exporting_a_plan_before_planning_is_a_validation_error() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        std::fs::create_dir_all(&project_dir).unwrap();
        let mut db = Db::open(&project_dir.join("state.sqlite")).unwrap();
        let project = Project::new(
            "Vacío",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        repository::create_project(&mut db, &project, &[], Actor::Test).unwrap();
        assert!(matches!(
            export_plan(&db, &project_dir),
            Err(DfError::Validation(_))
        ));
    }
}
