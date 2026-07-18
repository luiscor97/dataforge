//! Content-defined chunk and similarity evidence (RFC-0001 §20).
//!
//! Exact SHA-256 identity remains the only definition of equality. These
//! entities describe *relationships* between different contents and are
//! deliberately incapable of authorising a plan or an execution.

use serde::{Deserialize, Serialize};

use crate::{ContentId, ProjectId, SimilarityRelationId, SimilarityRunId, SnapshotId, Timestamp};

/// Lifecycle of one configuration-addressed similarity run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SimilarityRunStatus {
    Running,
    Completed,
    Failed,
}

impl SimilarityRunStatus {
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
                "unknown similarity run status `{other}`"
            ))),
        }
    }
}

/// Conservative interpretation of shared chunk evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContentRelationshipKind {
    LikelyVersion,
    TruncatedVariant,
    RecomposedContent,
    SimilarContent,
}

impl ContentRelationshipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LikelyVersion => "LIKELY_VERSION",
            Self::TruncatedVariant => "TRUNCATED_VARIANT",
            Self::RecomposedContent => "RECOMPOSED_CONTENT",
            Self::SimilarContent => "SIMILAR_CONTENT",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "LIKELY_VERSION" => Ok(Self::LikelyVersion),
            "TRUNCATED_VARIANT" => Ok(Self::TruncatedVariant),
            "RECOMPOSED_CONTENT" => Ok(Self::RecomposedContent),
            "SIMILAR_CONTENT" => Ok(Self::SimilarContent),
            other => Err(df_error::DfError::Validation(format!(
                "unknown content relationship `{other}`"
            ))),
        }
    }
}

/// Temporal direction carried by filesystem timestamps; never guessed when
/// the evidence is absent or equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipDirection {
    AToB,
    BToA,
    Unknown,
}

impl RelationshipDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AToB => "A_TO_B",
            Self::BToA => "B_TO_A",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "A_TO_B" => Ok(Self::AToB),
            "B_TO_A" => Ok(Self::BToA),
            "UNKNOWN" => Ok(Self::Unknown),
            other => Err(df_error::DfError::Validation(format!(
                "unknown relationship direction `{other}`"
            ))),
        }
    }
}

/// Monotonic counters persisted with a similarity run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarityRunCounters {
    pub contents_total: u64,
    pub contents_chunked: u64,
    pub contents_skipped: u64,
    pub chunks_total: u64,
    pub candidates_total: u64,
    pub relations_total: u64,
}

/// One reproducible execution over a completed inventory snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarityRun {
    pub id: SimilarityRunId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub status: SimilarityRunStatus,
    pub algorithm_version: String,
    pub config_digest: String,
    /// Canonical configuration covered by `config_digest`.
    pub config: serde_json::Value,
    pub counters: SimilarityRunCounters,
    pub candidate_cap_reached: bool,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// Exact, weighted relationship between two distinct SHA-256 contents.
// No `Eq`: the score fields are finite f64 values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentRelationship {
    pub id: SimilarityRelationId,
    pub run_id: SimilarityRunId,
    pub snapshot_id: SnapshotId,
    pub content_a: ContentId,
    pub content_b: ContentId,
    pub kind: ContentRelationshipKind,
    pub direction: RelationshipDirection,
    /// Exact multiset weighted Jaccard: shared bytes / union bytes.
    pub similarity: f64,
    pub shared_chunks: u64,
    pub shared_bytes: u64,
    pub union_bytes: u64,
    /// MinHash estimate retained as candidate-generation evidence only.
    pub estimated_similarity: f64,
    pub confidence: f64,
    pub evidence: serde_json::Value,
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_enums_round_trip() {
        for status in [
            SimilarityRunStatus::Running,
            SimilarityRunStatus::Completed,
            SimilarityRunStatus::Failed,
        ] {
            assert_eq!(SimilarityRunStatus::parse(status.as_str()).unwrap(), status);
        }
        for kind in [
            ContentRelationshipKind::LikelyVersion,
            ContentRelationshipKind::TruncatedVariant,
            ContentRelationshipKind::RecomposedContent,
            ContentRelationshipKind::SimilarContent,
        ] {
            assert_eq!(ContentRelationshipKind::parse(kind.as_str()).unwrap(), kind);
        }
        for direction in [
            RelationshipDirection::AToB,
            RelationshipDirection::BToA,
            RelationshipDirection::Unknown,
        ] {
            assert_eq!(
                RelationshipDirection::parse(direction.as_str()).unwrap(),
                direction
            );
        }
    }
}
