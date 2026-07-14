//! Planning entities (RFC-0001 §9.9, §9.10, §26, §27).
//!
//! A plan is the complete, reviewable proposal of what the executor will do
//! with a snapshot. Every occurrence of the snapshot must be covered by
//! exactly one operation (§26.2) and an approved plan is immutable (§26.4).

use serde::{Deserialize, Serialize};

use crate::{
    ids::{ContentId, OccurrenceId, OperationId, PlanId, ProjectId, SnapshotId},
    Timestamp,
};

/// Confidence of an automated decision, in `[0.0, 1.0]`.
pub type Confidence = f64;

/// Lifecycle of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlanStatus {
    /// Being generated; not yet valid.
    Draft,
    /// Generated and validated; awaiting review/approval.
    Ready,
    /// Frozen (§26.4): only execution progress may change from here on.
    Approved,
    /// Replaced by a newer plan version before approval.
    Superseded,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Ready => "READY",
            Self::Approved => "APPROVED",
            Self::Superseded => "SUPERSEDED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "READY" => Ok(Self::Ready),
            "APPROVED" => Ok(Self::Approved),
            "SUPERSEDED" => Ok(Self::Superseded),
            other => Err(df_error::DfError::Validation(format!(
                "unknown plan status `{other}`"
            ))),
        }
    }
}

/// What an operation does (RFC-0001 §26.1).
///
/// Milestone 0.1 emits `COPY_ACTIVE`, `CREATE_DIRECTORY`, `NO_ACTION` and
/// `BLOCKED`; the context-aware variants arrive with profiles and rules
/// (Milestone 0.2) but are representable now so plans stay forward-readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationType {
    CopyActive,
    CopyReview,
    CopySeparated,
    CopyTemporary,
    CopyWithSuffix,
    SkipRepresented,
    PreserveAcrossContext,
    CreateDirectory,
    NoAction,
    Blocked,
}

impl OperationType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CopyActive => "COPY_ACTIVE",
            Self::CopyReview => "COPY_REVIEW",
            Self::CopySeparated => "COPY_SEPARATED",
            Self::CopyTemporary => "COPY_TEMPORARY",
            Self::CopyWithSuffix => "COPY_WITH_SUFFIX",
            Self::SkipRepresented => "SKIP_REPRESENTED",
            Self::PreserveAcrossContext => "PRESERVE_ACROSS_CONTEXT",
            Self::CreateDirectory => "CREATE_DIRECTORY",
            Self::NoAction => "NO_ACTION",
            Self::Blocked => "BLOCKED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|t| t.as_str() == value)
            .ok_or_else(|| {
                df_error::DfError::Validation(format!("unknown operation type `{value}`"))
            })
    }

    pub const ALL: [OperationType; 10] = [
        Self::CopyActive,
        Self::CopyReview,
        Self::CopySeparated,
        Self::CopyTemporary,
        Self::CopyWithSuffix,
        Self::SkipRepresented,
        Self::PreserveAcrossContext,
        Self::CreateDirectory,
        Self::NoAction,
        Self::Blocked,
    ];

    /// Whether the executor materialises something for this operation.
    pub fn is_executable(self) -> bool {
        matches!(
            self,
            Self::CopyActive
                | Self::CopyReview
                | Self::CopySeparated
                | Self::CopyTemporary
                | Self::CopyWithSuffix
                | Self::CreateDirectory
        )
    }
}

/// Human review state of one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
}

impl ApprovalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "APPROVED" => Ok(Self::Approved),
            "REJECTED" => Ok(Self::Rejected),
            other => Err(df_error::DfError::Validation(format!(
                "unknown approval state `{other}`"
            ))),
        }
    }
}

/// Execution progress of one operation (RFC-0001 §27.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionState {
    Pending,
    Running,
    CopiedPartial,
    HashVerified,
    Completed,
    FailedRetryable,
    FailedFinal,
    Blocked,
}

impl ExecutionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Running => "RUNNING",
            Self::CopiedPartial => "COPIED_PARTIAL",
            Self::HashVerified => "HASH_VERIFIED",
            Self::Completed => "COMPLETED",
            Self::FailedRetryable => "FAILED_RETRYABLE",
            Self::FailedFinal => "FAILED_FINAL",
            Self::Blocked => "BLOCKED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "RUNNING" => Ok(Self::Running),
            "COPIED_PARTIAL" => Ok(Self::CopiedPartial),
            "HASH_VERIFIED" => Ok(Self::HashVerified),
            "COMPLETED" => Ok(Self::Completed),
            "FAILED_RETRYABLE" => Ok(Self::FailedRetryable),
            "FAILED_FINAL" => Ok(Self::FailedFinal),
            "BLOCKED" => Ok(Self::Blocked),
            other => Err(df_error::DfError::Validation(format!(
                "unknown execution state `{other}`"
            ))),
        }
    }

    /// States the executor never revisits.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::FailedFinal | Self::Blocked)
    }
}

/// Typed error codes of the execution protocol (RFC-0001 §27.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationErrorCode {
    SourceChanged,
    SourceMissing,
    PermissionDenied,
    NoSpace,
    HashMismatch,
    DestinationChanged,
    InvalidPath,
    IoError,
}

impl OperationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceChanged => "SOURCE_CHANGED",
            Self::SourceMissing => "SOURCE_MISSING",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::NoSpace => "NO_SPACE",
            Self::HashMismatch => "HASH_MISMATCH",
            Self::DestinationChanged => "DESTINATION_CHANGED",
            Self::InvalidPath => "INVALID_PATH",
            Self::IoError => "IO_ERROR",
        }
    }
}

/// Risk attributed to an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "LOW" => Ok(Self::Low),
            "MEDIUM" => Ok(Self::Medium),
            "HIGH" => Ok(Self::High),
            other => Err(df_error::DfError::Validation(format!(
                "unknown risk level `{other}`"
            ))),
        }
    }
}

/// A reconstruction plan over one snapshot (RFC-0001 §9.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub id: PlanId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    /// 1-based, increasing per project; a re-plan supersedes the previous.
    pub version: u32,
    pub status: PlanStatus,
    /// SHA-256 of the canonical serialization, fixed at approval (§26.4).
    pub serialized_sha256: Option<String>,
    pub created_at: Timestamp,
    pub approved_at: Option<Timestamp>,
}

impl Plan {
    pub fn new(project_id: ProjectId, snapshot_id: SnapshotId, version: u32) -> Self {
        Self {
            id: PlanId::new(),
            project_id,
            snapshot_id,
            version,
            status: PlanStatus::Draft,
            serialized_sha256: None,
            created_at: chrono::Utc::now(),
            approved_at: None,
        }
    }
}

/// One operation of a plan (RFC-0001 §9.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanOperation {
    pub id: OperationId,
    pub plan_id: PlanId,
    /// 1-based position; directories sort before the files they contain.
    pub sequence: u64,
    pub operation_type: OperationType,
    /// The occurrence this operation covers. `None` only for operations
    /// that cover a folder (`CREATE_DIRECTORY`).
    pub source_occurrence: Option<OccurrenceId>,
    pub content_id: Option<ContentId>,
    /// Destination, relative to the project output root.
    pub destination_relative_path: Option<String>,
    pub confidence: Confidence,
    pub risk: RiskLevel,
    pub approval: ApprovalState,
    pub execution_state: ExecutionState,
    /// Deterministic key (§26.3): SHA-256 over project, snapshot, plan
    /// version, occurrence, operation type and destination.
    pub idempotency_key: String,
    /// Human-readable justification (explainable-by-design, RFC §5.3).
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_canonical_names() {
        for status in [
            PlanStatus::Draft,
            PlanStatus::Ready,
            PlanStatus::Approved,
            PlanStatus::Superseded,
        ] {
            assert_eq!(PlanStatus::parse(status.as_str()).unwrap(), status);
        }
        for op in OperationType::ALL {
            assert_eq!(OperationType::parse(op.as_str()).unwrap(), op);
        }
        for state in [
            ExecutionState::Pending,
            ExecutionState::Running,
            ExecutionState::CopiedPartial,
            ExecutionState::HashVerified,
            ExecutionState::Completed,
            ExecutionState::FailedRetryable,
            ExecutionState::FailedFinal,
            ExecutionState::Blocked,
        ] {
            assert_eq!(ExecutionState::parse(state.as_str()).unwrap(), state);
        }
        for risk in [RiskLevel::Low, RiskLevel::Medium, RiskLevel::High] {
            assert_eq!(RiskLevel::parse(risk.as_str()).unwrap(), risk);
        }
        for approval in [
            ApprovalState::Pending,
            ApprovalState::Approved,
            ApprovalState::Rejected,
        ] {
            assert_eq!(ApprovalState::parse(approval.as_str()).unwrap(), approval);
        }
        assert!(PlanStatus::parse("FROZEN").is_err());
        assert!(OperationType::parse("DELETE").is_err(), "no delete exists");
    }

    #[test]
    fn executable_types_are_the_copy_and_directory_ones() {
        assert!(OperationType::CopyActive.is_executable());
        assert!(OperationType::CreateDirectory.is_executable());
        assert!(!OperationType::NoAction.is_executable());
        assert!(!OperationType::Blocked.is_executable());
        assert!(!OperationType::SkipRepresented.is_executable());
    }

    #[test]
    fn terminal_execution_states() {
        assert!(ExecutionState::Completed.is_terminal());
        assert!(ExecutionState::FailedFinal.is_terminal());
        assert!(ExecutionState::Blocked.is_terminal());
        assert!(!ExecutionState::Pending.is_terminal());
        assert!(!ExecutionState::FailedRetryable.is_terminal());
        assert!(!ExecutionState::Running.is_terminal());
    }

    #[test]
    fn new_plans_start_as_draft_without_hash() {
        let plan = Plan::new(ProjectId::new(), SnapshotId::new(), 1);
        assert_eq!(plan.status, PlanStatus::Draft);
        assert!(plan.serialized_sha256.is_none());
        assert!(plan.approved_at.is_none());
    }
}
