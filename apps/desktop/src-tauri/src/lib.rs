//! DataForge Desktop — thin Tauri shell.
//!
//! The UI holds no critical logic (RFC-0001 rule 16): every command below
//! delegates to `df-facade`, exactly like the CLI does.

use df_domain::{Actor, DuplicatePolicy};
use df_error::DfError;
use df_facade::{
    AnalyzeOutcome, ApproveOutcome, ContentArtifactBuildOutcome, ContentExtractionOptions,
    ContentExtractionOutcome, ContentQueryOutcome, ContentSearchOutcome, CreateProjectRequest,
    ExecuteOutcome, HashOutcome, MediaOutcome, PlanOutcome, ProjectStatus, QueryOptions,
    ScanOutcome, SearchRequest, SimilarityOutcome, SnapshotBuildOptions, VerifyOutcome,
};
use serde::Serialize;

/// Error shape delivered to the frontend.
#[derive(Debug, Serialize)]
pub struct ErrorDto {
    pub code: String,
    pub message: String,
}

impl From<DfError> for ErrorDto {
    fn from(error: DfError) -> Self {
        let code = match &error {
            DfError::InvalidTransition { .. } => "invalid_transition",
            DfError::Validation(_) => "validation",
            DfError::NotFound(_) => "not_found",
            DfError::Conflict(_) => "conflict",
            DfError::Database(_) => "database",
            DfError::LedgerIntegrity(_) => "ledger_integrity",
            DfError::Serialization(_) => "serialization",
            DfError::Io { .. } => "io",
        };
        Self {
            code: code.to_string(),
            message: error.to_string(),
        }
    }
}

async fn run_blocking_command<T, F>(operation: F) -> Result<T, ErrorDto>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DfError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| ErrorDto {
            code: "internal".to_string(),
            message: format!("desktop command worker failed: {error}"),
        })?
        .map_err(ErrorDto::from)
}

#[tauri::command]
fn create_project(request: CreateProjectRequest) -> Result<ProjectStatus, ErrorDto> {
    df_facade::create_project(&request, Actor::Desktop).map_err(ErrorDto::from)
}

#[tauri::command]
fn open_project(project_dir: String) -> Result<ProjectStatus, ErrorDto> {
    df_facade::open_project(std::path::Path::new(&project_dir)).map_err(ErrorDto::from)
}

#[tauri::command]
fn project_status(project_dir: String) -> Result<ProjectStatus, ErrorDto> {
    df_facade::project_status(std::path::Path::new(&project_dir)).map_err(ErrorDto::from)
}

// --- The reconstruction pipeline -----------------------------------------
//
// Until now the shell could create a project and run the optional analyses,
// but not the work the product exists for: a non-technical user had to drop
// to the CLI to actually reconstruct anything. Each command below is the same
// facade call the CLI makes, run off the UI thread because these are long.

/// Inventory the sources into an immutable snapshot. Reads only.
#[tauri::command]
async fn scan_project(project_dir: String) -> Result<ScanOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::scan_project(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

/// Give every scanned file its content identity (BLAKE3 + SHA-256).
#[tauri::command]
async fn hash_project(project_dir: String) -> Result<HashOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::hash_project(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

/// Materialise the exact-duplicate evidence of the hashed snapshot.
#[tauri::command]
async fn analyze_project(project_dir: String) -> Result<AnalyzeOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::analyze_project(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

/// Propose the reconstruction. `REPORT_ONLY` is the only policy the guided
/// flow offers: duplicates are reported, never consolidated, so nothing the
/// user owns can be dropped by a decision they did not make.
#[tauri::command]
async fn create_plan(project_dir: String) -> Result<PlanOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::create_plan(
            std::path::Path::new(&project_dir),
            Actor::Desktop,
            DuplicatePolicy::ReportOnly,
        )
    })
    .await
}

/// Freeze the plan into the immutable execution manifest.
#[tauri::command]
async fn approve_plan(project_dir: String) -> Result<ApproveOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::approve_plan(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

/// Execute the approved manifest: verified copy, resumable, never replacing.
#[tauri::command]
async fn execute_plan(project_dir: String) -> Result<ExecuteOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::execute_plan(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

/// Re-check the output from primary evidence, independently of the executor.
#[tauri::command]
async fn verify_project(project_dir: String) -> Result<VerifyOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::verify_project_output(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

#[tauri::command]
fn analyze_similarity(project_dir: String) -> Result<SimilarityOutcome, ErrorDto> {
    df_facade::analyze_similarity(std::path::Path::new(&project_dir), Actor::Desktop)
        .map_err(ErrorDto::from)
}

#[tauri::command]
async fn analyze_media(project_dir: String) -> Result<MediaOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::analyze_media(std::path::Path::new(&project_dir), Actor::Desktop)
    })
    .await
}

#[tauri::command]
async fn extract_content(project_dir: String) -> Result<ContentExtractionOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::extract_project_content(
            std::path::Path::new(&project_dir),
            Actor::Desktop,
            &ContentExtractionOptions::default(),
        )
    })
    .await
}

#[tauri::command]
async fn fail_content_extraction(
    project_dir: String,
    run_id: String,
    reason: String,
) -> Result<ContentExtractionOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::fail_content_extraction(
            std::path::Path::new(&project_dir),
            &run_id,
            &reason,
            Actor::Desktop,
        )
    })
    .await
}

#[tauri::command]
async fn build_content_artifacts(
    project_dir: String,
    run_id: Option<String>,
) -> Result<ContentArtifactBuildOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::build_content_artifacts(
            std::path::Path::new(&project_dir),
            run_id.as_deref(),
            Default::default(),
            SnapshotBuildOptions::default(),
            Actor::Desktop,
        )
    })
    .await
}

#[tauri::command]
async fn search_content(
    project_dir: String,
    run_id: Option<String>,
    request: SearchRequest,
) -> Result<ContentSearchOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::search_project_content(
            std::path::Path::new(&project_dir),
            run_id.as_deref(),
            &request,
        )
    })
    .await
}

#[tauri::command]
async fn query_content(
    project_dir: String,
    run_id: Option<String>,
    sql: String,
) -> Result<ContentQueryOutcome, ErrorDto> {
    run_blocking_command(move || {
        df_facade::query_project_content(
            std::path::Path::new(&project_dir),
            run_id.as_deref(),
            &sql,
            QueryOptions::default(),
        )
    })
    .await
}

#[tauri::command]
fn engine_version() -> String {
    df_facade::APP_VERSION.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            create_project,
            open_project,
            project_status,
            scan_project,
            hash_project,
            analyze_project,
            create_plan,
            approve_plan,
            execute_plan,
            verify_project,
            analyze_similarity,
            analyze_media,
            extract_content,
            fail_content_extraction,
            build_content_artifacts,
            search_content,
            query_content,
            engine_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running DataForge Desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_errors_keep_a_stable_frontend_shape() {
        let error = ErrorDto::from(DfError::Validation("consulta vacía".to_string()));

        assert_eq!(error.code, "validation");
        assert!(error.message.contains("consulta vacía"));
        assert_eq!(
            serde_json::to_value(&error).expect("error DTO must serialize"),
            serde_json::json!({
                "code": "validation",
                "message": "validation failed: consulta vacía"
            })
        );
    }
}
