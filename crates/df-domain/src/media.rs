//! Media intelligence evidence (RFC-0001 §21, Milestone 0.5).
//!
//! Perceptual fingerprints relate *renditions* of the same visual or
//! acoustic material — a recompressed photo, a transcoded track — while
//! exact SHA-256 identity remains the only definition of equality. Every
//! entity here is review evidence: it cannot authorise a plan, an execution
//! or any destructive operation.

use serde::{Deserialize, Serialize};

use crate::{ContentId, MediaRelationId, MediaRunId, ProjectId, SnapshotId, Timestamp};

/// Lifecycle of one configuration-addressed media run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaRunStatus {
    Running,
    Completed,
    Failed,
}

impl MediaRunStatus {
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
                "unknown media run status `{other}`"
            ))),
        }
    }
}

/// The three review relations perceptual comparison can propose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaRelationKind {
    ImagePerceptualMatch,
    AudioAcousticMatch,
    VideoPerceptualMatch,
}

impl MediaRelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImagePerceptualMatch => "IMAGE_PERCEPTUAL_MATCH",
            Self::AudioAcousticMatch => "AUDIO_ACOUSTIC_MATCH",
            Self::VideoPerceptualMatch => "VIDEO_PERCEPTUAL_MATCH",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "IMAGE_PERCEPTUAL_MATCH" => Ok(Self::ImagePerceptualMatch),
            "AUDIO_ACOUSTIC_MATCH" => Ok(Self::AudioAcousticMatch),
            "VIDEO_PERCEPTUAL_MATCH" => Ok(Self::VideoPerceptualMatch),
            other => Err(df_error::DfError::Validation(format!(
                "unknown media relation `{other}`"
            ))),
        }
    }
}

/// Monotonic counters persisted with a media run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRunCounters {
    pub contents_total: u64,
    pub contents_analyzed: u64,
    pub contents_limited: u64,
    pub contents_failed: u64,
    pub pairs_compared: u64,
    pub relations_total: u64,
}

/// One immutable, configuration-addressed media analysis run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRun {
    pub id: MediaRunId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub status: MediaRunStatus,
    pub contract_version: String,
    /// SHA-256 of `config` in canonical JSON; the run's reuse identity.
    pub config_digest: String,
    pub config: serde_json::Value,
    pub counters: MediaRunCounters,
    pub pair_cap_reached: bool,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// A sealed, non-destructive review relation between two contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRelationRecord {
    pub id: MediaRelationId,
    pub run_id: MediaRunId,
    pub snapshot_id: SnapshotId,
    /// Ordered pair: `content_a < content_b` textually, stored once.
    pub content_a: ContentId,
    pub content_b: ContentId,
    pub kind: MediaRelationKind,
    /// Similarity score scaled to millionths (1_000_000 = identical
    /// fingerprints), copied verbatim from the comparison evidence.
    pub score_millionths: u32,
    /// The serialized `ComparisonEvidence` of the engine contract.
    pub evidence: serde_json::Value,
    pub created_at: Timestamp,
}
