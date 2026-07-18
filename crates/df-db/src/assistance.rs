//! Persistence for the assisted-intelligence audit trail (Milestone 0.7).
//!
//! One immutable row per invocation, storing the engine's audit contract
//! verbatim plus indexable columns. Keys, consent tokens and raw provider
//! payloads never reach this module.

use df_domain::{Actor, ProjectId};
use df_error::DfResult;
use rusqlite::params;
use uuid::Uuid;

use crate::repository::{append_event, to_stored_timestamp};
use crate::{db_err, Db};

pub const EVENT_AI_ASSISTANCE: &str = "AI_ASSISTANCE_RECORDED";

/// Indexable projection of one audit row; `audit_json` is the full contract.
#[derive(Debug, Clone)]
pub struct AssistanceAuditInput {
    pub request_id_sha256: String,
    pub purpose: String,
    pub provider_kind: String,
    pub provider: String,
    pub model: String,
    pub endpoint: String,
    pub status: String,
    pub failure: Option<String>,
    pub disclosure_sha256: String,
    pub prompt_sha256: String,
    pub audit_json: String,
}

/// A stored audit row for reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssistanceAuditView {
    pub purpose: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub failure: Option<String>,
    pub disclosure_sha256: String,
    pub created_at: String,
}

/// Persist one audit row and its ledger event in the same transaction.
pub fn insert_audit(
    db: &mut Db,
    project_id: ProjectId,
    input: &AssistanceAuditInput,
    actor: Actor,
) -> DfResult<()> {
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO assistance_audits
            (id, project_id, request_id_sha256, purpose, provider_kind,
             provider, model, endpoint, status, failure, disclosure_sha256,
             prompt_sha256, audit_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            Uuid::new_v4().to_string(),
            project_id.to_string(),
            input.request_id_sha256,
            input.purpose,
            input.provider_kind,
            input.provider,
            input.model,
            input.endpoint,
            input.status,
            input.failure,
            input.disclosure_sha256,
            input.prompt_sha256,
            input.audit_json,
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        project_id,
        EVENT_AI_ASSISTANCE,
        &serde_json::json!({
            "request_id_sha256": input.request_id_sha256,
            "purpose": input.purpose,
            "provider": input.provider,
            "model": input.model,
            "status": input.status,
            "disclosure_sha256": input.disclosure_sha256,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Latest audits of the project, newest first.
pub fn list_audits(
    db: &Db,
    project_id: ProjectId,
    limit: u32,
) -> DfResult<Vec<AssistanceAuditView>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT purpose, provider, model, status, failure,
                    disclosure_sha256, created_at
             FROM assistance_audits
             WHERE project_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id.to_string(), limit as i64], |row| {
            Ok(AssistanceAuditView {
                purpose: row.get(0)?,
                provider: row.get(1)?,
                model: row.get(2)?,
                status: row.get(3)?,
                failure: row.get(4)?,
                disclosure_sha256: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}
