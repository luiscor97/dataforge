//! Physical inventory entities (RFC-0001 §9.3, §13).

use std::path::PathBuf;

use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};

use crate::{
    ids::{FolderId, OccurrenceId, ScanRunId, SnapshotId, SourceRootId},
    Timestamp,
};

/// Physical fingerprint used to detect source changes without hashing
/// (RFC-0001 §14.1). Volume serial / file index are added when viable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size_bytes: u64,
    /// Modification time as nanoseconds since the Unix epoch.
    pub modified_unix_ns: i64,
}

impl Fingerprint {
    pub fn encode(&self) -> String {
        format!("v1:{}:{}", self.size_bytes, self.modified_unix_ns)
    }

    pub fn decode(text: &str) -> DfResult<Self> {
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() != 3 || parts[0] != "v1" {
            return Err(DfError::Validation(format!("bad fingerprint `{text}`")));
        }
        Ok(Self {
            size_bytes: parts[1]
                .parse()
                .map_err(|_| DfError::Validation(format!("bad fingerprint `{text}`")))?,
            modified_unix_ns: parts[2]
                .parse()
                .map_err(|_| DfError::Validation(format!("bad fingerprint `{text}`")))?,
        })
    }
}

/// Kind of a non-directory entry found during a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntryKind {
    File,
    /// Reparse point (symlink, junction, mount point). Never followed by
    /// default (RFC-0001 §13.6).
    Reparse,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "FILE",
            Self::Reparse => "REPARSE",
        }
    }

    pub fn parse(v: &str) -> DfResult<Self> {
        match v {
            "FILE" => Ok(Self::File),
            "REPARSE" => Ok(Self::Reparse),
            other => Err(DfError::Validation(format!("unknown entry kind `{other}`"))),
        }
    }
}

/// Scan status of one occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScanStatus {
    Ok,
    /// Reparse point recorded but not traversed.
    SeenNotFollowed,
    /// Metadata could not be read; see `error`.
    Error,
}

impl ScanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::SeenNotFollowed => "SEEN_NOT_FOLLOWED",
            Self::Error => "ERROR",
        }
    }

    pub fn parse(v: &str) -> DfResult<Self> {
        match v {
            "OK" => Ok(Self::Ok),
            "SEEN_NOT_FOLLOWED" => Ok(Self::SeenNotFollowed),
            "ERROR" => Ok(Self::Error),
            other => Err(DfError::Validation(format!("unknown scan status `{other}`"))),
        }
    }
}

/// One physical appearance of a file (or reparse point) inside a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathOccurrence {
    pub id: OccurrenceId,
    pub snapshot_id: SnapshotId,
    pub source_root_id: SourceRootId,
    /// Path relative to the source root (display form).
    pub relative_path: PathBuf,
    /// Raw UTF-16 code units of the relative path, only stored when the
    /// display form was lossy (RFC-0001 §13.4: no destructive conversion).
    pub raw_path_utf16: Option<Vec<u8>>,
    pub file_name: String,
    pub normalized_name: String,
    pub extension: Option<String>,
    pub kind: EntryKind,
    pub size_bytes: u64,
    pub created_at_fs: Option<Timestamp>,
    pub modified_at_fs: Option<Timestamp>,
    /// Windows file attribute bits (0 when unavailable).
    pub attributes: u32,
    pub depth: u32,
    pub path_length: u32,
    pub fingerprint: Option<Fingerprint>,
    pub scan_status: ScanStatus,
    pub error: Option<String>,
}

/// A directory recorded during a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntry {
    pub id: FolderId,
    pub snapshot_id: SnapshotId,
    pub source_root_id: SourceRootId,
    pub relative_path: PathBuf,
    pub depth: u32,
    /// Direct children seen (files + dirs + reparse). None if unreadable.
    pub entry_count: Option<u32>,
    pub error: Option<String>,
}

/// Lifecycle of a scan run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScanRunStatus {
    Running,
    Complete,
    Cancelled,
    Failed,
    /// Found RUNNING at startup after a crash; superseded by a new run.
    Abandoned,
}

impl ScanRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Complete => "COMPLETE",
            Self::Cancelled => "CANCELLED",
            Self::Failed => "FAILED",
            Self::Abandoned => "ABANDONED",
        }
    }

    pub fn parse(v: &str) -> DfResult<Self> {
        match v {
            "RUNNING" => Ok(Self::Running),
            "COMPLETE" => Ok(Self::Complete),
            "CANCELLED" => Ok(Self::Cancelled),
            "FAILED" => Ok(Self::Failed),
            "ABANDONED" => Ok(Self::Abandoned),
            other => Err(DfError::Validation(format!(
                "unknown scan run status `{other}`"
            ))),
        }
    }
}

/// Aggregated counters of one scan run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCounters {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub reparse_points: u64,
    pub errors: u64,
}

/// A scan run over a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRun {
    pub id: ScanRunId,
    pub snapshot_id: SnapshotId,
    pub status: ScanRunStatus,
    pub counters: ScanCounters,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_round_trips() {
        let fp = Fingerprint {
            size_bytes: 123,
            modified_unix_ns: 1_700_000_000_123_456_789,
        };
        assert_eq!(Fingerprint::decode(&fp.encode()).unwrap(), fp);
        assert!(Fingerprint::decode("v2:1:2").is_err());
        assert!(Fingerprint::decode("garbage").is_err());
    }

    #[test]
    fn enums_round_trip() {
        for k in [EntryKind::File, EntryKind::Reparse] {
            assert_eq!(EntryKind::parse(k.as_str()).unwrap(), k);
        }
        for s in [ScanStatus::Ok, ScanStatus::SeenNotFollowed, ScanStatus::Error] {
            assert_eq!(ScanStatus::parse(s.as_str()).unwrap(), s);
        }
        for r in [
            ScanRunStatus::Running,
            ScanRunStatus::Complete,
            ScanRunStatus::Cancelled,
            ScanRunStatus::Failed,
            ScanRunStatus::Abandoned,
        ] {
            assert_eq!(ScanRunStatus::parse(r.as_str()).unwrap(), r);
        }
    }
}
