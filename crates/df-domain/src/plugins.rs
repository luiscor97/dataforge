//! Plugin ecosystem evidence (RFC-0001 §22, Milestone 0.6).
//!
//! A registered plugin is a signed, content-addressed WebAssembly component
//! whose ABI can only report findings and suggestions. Runs and findings are
//! evidence with the same doctrine as similarity and media: sealed,
//! append-only, and structurally incapable of authorising an operation.

use serde::{Deserialize, Serialize};

use crate::{PluginRegistrationId, PluginRunId, ProjectId, SnapshotId, Timestamp};

/// Lifecycle of one configuration-addressed plugin run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PluginRunStatus {
    Running,
    Completed,
    Failed,
}

impl PluginRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "RUNNING" => Ok(Self::Running),
            "COMPLETED" => Ok(Self::Completed),
            "FAILED" => Ok(Self::Failed),
            other => Err(df_error::DfError::Validation(format!(
                "unknown plugin run status `{other}`"
            ))),
        }
    }
}

/// Monotonic counters persisted with a plugin run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRunCounters {
    pub subjects_total: u64,
    pub subjects_analyzed: u64,
    pub subjects_failed: u64,
    pub findings_total: u64,
}

/// One immutable, configuration-addressed plugin run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRun {
    pub id: PluginRunId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub registration_id: PluginRegistrationId,
    pub status: PluginRunStatus,
    /// SHA-256 of `config` in canonical JSON; the run's reuse identity.
    pub config_digest: String,
    pub config: serde_json::Value,
    pub counters: PluginRunCounters,
    pub subject_cap_reached: bool,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}
