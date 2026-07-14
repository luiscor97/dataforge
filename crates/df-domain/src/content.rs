//! Content identity entities (RFC-0001 §9.4, §14, §15).

use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};

use crate::ids::{ContentId, SnapshotId};

/// Hashing lifecycle of a content object / hash job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HashState {
    Pending,
    Hashed,
    /// The fingerprint changed between snapshot and hashing
    /// (`SOURCE_CHANGED_DURING_HASH`, RFC-0001 §14.5).
    SourceChanged,
    Failed,
    /// Not a duplicate candidate in fast mode (RFC-0001 §14.4).
    Skipped,
}

impl HashState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Hashed => "HASHED",
            Self::SourceChanged => "SOURCE_CHANGED",
            Self::Failed => "FAILED",
            Self::Skipped => "SKIPPED",
        }
    }

    pub fn parse(v: &str) -> DfResult<Self> {
        match v {
            "PENDING" => Ok(Self::Pending),
            "HASHED" => Ok(Self::Hashed),
            "SOURCE_CHANGED" => Ok(Self::SourceChanged),
            "FAILED" => Ok(Self::Failed),
            "SKIPPED" => Ok(Self::Skipped),
            other => Err(DfError::Validation(format!("unknown hash state `{other}`"))),
        }
    }
}

/// Hashing strategy (RFC-0001 §14.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashMode {
    /// BLAKE3 + SHA-256 of every file. Recommended for the legal profile.
    Full,
    /// Hash only files whose size collides with another file.
    Fast,
}

impl HashMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Fast => "fast",
        }
    }

    pub fn parse(v: &str) -> DfResult<Self> {
        match v {
            "full" => Ok(Self::Full),
            "fast" => Ok(Self::Fast),
            other => Err(DfError::Validation(format!("unknown hash mode `{other}`"))),
        }
    }
}

/// Unique binary content, keyed by SHA-256 (canonical) with BLAKE3 as the
/// operational hash (RFC-0001 §6 ADR-0007).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentObject {
    pub id: ContentId,
    pub size_bytes: u64,
    pub sha256: String,
    pub blake3: String,
    pub first_seen_snapshot: SnapshotId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip() {
        for s in [
            HashState::Pending,
            HashState::Hashed,
            HashState::SourceChanged,
            HashState::Failed,
            HashState::Skipped,
        ] {
            assert_eq!(HashState::parse(s.as_str()).unwrap(), s);
        }
        for m in [HashMode::Full, HashMode::Fast] {
            assert_eq!(HashMode::parse(m.as_str()).unwrap(), m);
        }
    }
}
