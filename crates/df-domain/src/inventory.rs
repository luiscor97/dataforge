//! Inventory entities (RFC-0001 §9.3, §9.4, §13, §14).
//!
//! A scan walks the source roots and records what exists — folders and file
//! occurrences — without following reparse points and without touching the
//! origin. Hashing later binds occurrences to unique [`ContentObject`]s.
//!
//! This module is pure data: the walker lives in `df-scan`, the hashing in
//! `df-hash`, the persistence in `df-db`.

use serde::{Deserialize, Serialize};

use crate::{
    ids::{ContentId, FolderId, OccurrenceId, ProjectId, ScanRunId, SnapshotId, SourceRootId},
    Timestamp,
};

/// Outcome of scanning one directory entry (RFC-0001 §13.2, §13.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScanEntryStatus {
    /// Metadata captured normally.
    Ok,
    /// The entry exists but reading it (or its metadata) failed; the error
    /// text travels with the record. Partial errors never abort a scan.
    Error,
    /// The entry is a reparse point (symlink, junction, mount point). It is
    /// recorded but never followed (RFC-0001 §13.6 `SEEN_NOT_FOLLOWED`).
    ReparseNotFollowed,
}

impl ScanEntryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Error => "ERROR",
            Self::ReparseNotFollowed => "REPARSE_NOT_FOLLOWED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "OK" => Ok(Self::Ok),
            "ERROR" => Ok(Self::Error),
            "REPARSE_NOT_FOLLOWED" => Ok(Self::ReparseNotFollowed),
            other => Err(df_error::DfError::Validation(format!(
                "unknown scan entry status `{other}`"
            ))),
        }
    }
}

/// Physical fingerprint of a file (RFC-0001 §14.1).
///
/// Cheap identity used to detect that a file changed between scan and hash
/// (`SOURCE_CHANGED_DURING_HASH`) and to skip re-hashing unchanged files.
/// It is *not* a content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub size_bytes: u64,
    pub modified_at_fs: Option<Timestamp>,
}

impl FileFingerprint {
    /// Canonical token stored in SQLite and compared verbatim.
    ///
    /// Versioned (`v1:`) so a future fingerprint that includes volume/file
    /// identity (RFC-0001 §13.5) never compares equal to an old one.
    pub fn token(&self) -> String {
        match self.modified_at_fs {
            Some(ts) => format!("v1:{}:{}", self.size_bytes, ts.timestamp_millis()),
            None => format!("v1:{}:none", self.size_bytes),
        }
    }
}

/// A directory seen during a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRecord {
    pub id: FolderId,
    pub snapshot_id: SnapshotId,
    pub source_root_id: SourceRootId,
    /// Path relative to the source root; empty string for the root itself.
    pub relative_path: String,
    pub parent_relative_path: Option<String>,
    pub name: String,
    pub normalized_name: String,
    /// 0 for the source root, 1 for its children, and so on.
    pub depth: u32,
    pub status: ScanEntryStatus,
    pub error: Option<String>,
}

/// A file occurrence: one physical appearance of content at a path
/// (RFC-0001 §9.3). Path and content are distinct entities (rule 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathOccurrence {
    pub id: OccurrenceId,
    pub snapshot_id: SnapshotId,
    pub source_root_id: SourceRootId,
    /// Path relative to the source root. The absolute path is reconstructed
    /// as `source_root.absolute_path ∪ relative_path`; it is not duplicated
    /// in storage so root relocation cannot desynchronise the inventory.
    pub relative_path: String,
    pub parent_relative_path: String,
    pub file_name: String,
    /// Lowercased name used for grouping and comparison (RFC-0001 §13.4);
    /// the raw name above is never modified.
    pub normalized_name: String,
    pub extension: Option<String>,
    pub size_bytes: u64,
    pub created_at_fs: Option<Timestamp>,
    pub modified_at_fs: Option<Timestamp>,
    /// Raw Windows file attribute bits; 0 when unknown or not Windows.
    pub attributes: u32,
    /// Length in UTF-16 code units of the absolute path (Windows limit unit).
    pub path_length: u32,
    pub depth: u32,
    /// [`FileFingerprint::token`] captured at scan time.
    pub fingerprint: String,
    pub scan_status: ScanEntryStatus,
    pub error: Option<String>,
    /// True when the on-disk name was not valid Unicode and `file_name`
    /// holds a lossy rendering (RFC-0001 §13.4: no destructive conversion —
    /// the flag preserves the fact that the raw name differs).
    pub name_is_lossy: bool,
}

/// Hash lifecycle of a [`ContentObject`] (RFC-0001 §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HashState {
    Pending,
    Hashed,
    Failed,
    /// The file changed between the pre- and post-hash fingerprint checks
    /// (RFC-0001 §14.5 `SOURCE_CHANGED_DURING_HASH`).
    SourceChanged,
}

impl HashState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Hashed => "HASHED",
            Self::Failed => "FAILED",
            Self::SourceChanged => "SOURCE_CHANGED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "HASHED" => Ok(Self::Hashed),
            "FAILED" => Ok(Self::Failed),
            "SOURCE_CHANGED" => Ok(Self::SourceChanged),
            other => Err(df_error::DfError::Validation(format!(
                "unknown hash state `{other}`"
            ))),
        }
    }
}

/// Unique binary content, identified by its hashes (RFC-0001 §9.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentObject {
    pub id: ContentId,
    pub size_bytes: u64,
    /// Canonical audit identity (RFC-0001 ADR-0007). Hex, lowercase.
    pub sha256: Option<String>,
    /// Operational identity for caches and future chunking. Hex, lowercase.
    pub blake3: Option<String>,
    pub mime_type: Option<String>,
    pub first_seen_snapshot: SnapshotId,
    pub hash_state: HashState,
}

/// Progress state of one scan execution (`scan_runs` table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScanRunStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl ScanRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "RUNNING" => Ok(Self::Running),
            "COMPLETED" => Ok(Self::Completed),
            "CANCELLED" => Ok(Self::Cancelled),
            "FAILED" => Ok(Self::Failed),
            other => Err(df_error::DfError::Validation(format!(
                "unknown scan run status `{other}`"
            ))),
        }
    }
}

/// Counters accumulated while scanning; persisted on the scan run and
/// reported in the `SCAN_COMPLETED` audit event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCounters {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub errors: u64,
    pub reparse_points: u64,
}

/// One execution of the scanner over the source roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRun {
    pub id: ScanRunId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub status: ScanRunStatus,
    pub counters: ScanCounters,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

impl ScanRun {
    pub fn new(project_id: ProjectId, snapshot_id: SnapshotId) -> Self {
        Self {
            id: ScanRunId::new(),
            project_id,
            snapshot_id,
            status: ScanRunStatus::Running,
            counters: ScanCounters::default(),
            started_at: chrono::Utc::now(),
            finished_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_token_is_versioned_and_stable() {
        let ts: Timestamp = "2026-07-14T10:00:00.123Z".parse().unwrap();
        let fp = FileFingerprint {
            size_bytes: 42,
            modified_at_fs: Some(ts),
        };
        assert_eq!(fp.token(), format!("v1:42:{}", ts.timestamp_millis()));
        let no_mtime = FileFingerprint {
            size_bytes: 42,
            modified_at_fs: None,
        };
        assert_eq!(no_mtime.token(), "v1:42:none");
        assert_ne!(fp.token(), no_mtime.token());
    }

    #[test]
    fn fingerprint_changes_when_size_or_mtime_change() {
        let ts: Timestamp = "2026-07-14T10:00:00.000Z".parse().unwrap();
        let base = FileFingerprint {
            size_bytes: 10,
            modified_at_fs: Some(ts),
        };
        let bigger = FileFingerprint {
            size_bytes: 11,
            ..base
        };
        let later = FileFingerprint {
            modified_at_fs: Some(ts + chrono::Duration::milliseconds(1)),
            ..base
        };
        assert_ne!(base.token(), bigger.token());
        assert_ne!(base.token(), later.token());
    }

    #[test]
    fn status_enums_round_trip() {
        for status in [
            ScanEntryStatus::Ok,
            ScanEntryStatus::Error,
            ScanEntryStatus::ReparseNotFollowed,
        ] {
            assert_eq!(ScanEntryStatus::parse(status.as_str()).unwrap(), status);
        }
        for state in [
            HashState::Pending,
            HashState::Hashed,
            HashState::Failed,
            HashState::SourceChanged,
        ] {
            assert_eq!(HashState::parse(state.as_str()).unwrap(), state);
        }
        for status in [
            ScanRunStatus::Running,
            ScanRunStatus::Completed,
            ScanRunStatus::Cancelled,
            ScanRunStatus::Failed,
        ] {
            assert_eq!(ScanRunStatus::parse(status.as_str()).unwrap(), status);
        }
        assert!(ScanEntryStatus::parse("SKIPPED").is_err());
        assert!(HashState::parse("DONE").is_err());
        assert!(ScanRunStatus::parse("PAUSED").is_err());
    }

    #[test]
    fn new_scan_runs_start_running_with_zero_counters() {
        let run = ScanRun::new(ProjectId::new(), SnapshotId::new());
        assert_eq!(run.status, ScanRunStatus::Running);
        assert_eq!(run.counters, ScanCounters::default());
        assert!(run.finished_at.is_none());
    }
}
