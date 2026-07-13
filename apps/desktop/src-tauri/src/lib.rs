//! DataForge Desktop — thin Tauri shell.
//!
//! The UI holds no critical logic (RFC-0001 rule 16): every command below
//! delegates to `df-facade`, exactly like the CLI does.

use df_domain::Actor;
use df_error::DfError;
use df_facade::{CreateProjectRequest, ProjectStatus};
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
            engine_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running DataForge Desktop");
}
