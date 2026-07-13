//! Snapshot entity.
//!
//! A snapshot is the immutable record of one scan of the source roots.
//! Real capture logic arrives with the scanner in Milestone 0.1; the entity
//! and its table exist now so every later result can reference one
//! (RFC-0001 rule 4).

use serde::{Deserialize, Serialize};

use crate::{
    ids::{ProjectId, SnapshotId},
    Timestamp,
};

/// Lifecycle of a snapshot capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SnapshotStatus {
    Pending,
    Capturing,
    Complete,
    Failed,
}

impl SnapshotStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Capturing => "CAPTURING",
            Self::Complete => "COMPLETE",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "CAPTURING" => Ok(Self::Capturing),
            "COMPLETE" => Ok(Self::Complete),
            "FAILED" => Ok(Self::Failed),
            other => Err(df_error::DfError::Validation(format!(
                "unknown snapshot status `{other}`"
            ))),
        }
    }
}

/// Immutable record of one inventory pass over the source roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub project_id: ProjectId,
    pub status: SnapshotStatus,
    pub created_at: Timestamp,
}

impl Snapshot {
    pub fn new(project_id: ProjectId) -> Self {
        Self {
            id: SnapshotId::new(),
            project_id,
            status: SnapshotStatus::Pending,
            created_at: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_snapshots_start_pending() {
        let s = Snapshot::new(ProjectId::new());
        assert_eq!(s.status, SnapshotStatus::Pending);
    }

    #[test]
    fn snapshot_status_round_trips() {
        for status in [
            SnapshotStatus::Pending,
            SnapshotStatus::Capturing,
            SnapshotStatus::Complete,
            SnapshotStatus::Failed,
        ] {
            assert_eq!(SnapshotStatus::parse(status.as_str()).unwrap(), status);
        }
    }
}
