//! Domain model of the DataForge engine.
//!
//! This crate is pure: no I/O, no SQL, no clock other than `Utc::now()`.
//! Every state change flows through the [`state::ProjectState`] machine and
//! is later persisted and audited by `df-db` / `df-ledger`.

pub mod event;
pub mod ids;
pub mod inventory;
pub mod plan;
pub mod project;
pub mod snapshot;
pub mod source_root;
pub mod state;
pub mod structure;

pub use event::{Actor, AuditEvent};
pub use ids::{
    ContentId, DuplicateSetId, EventId, FindingId, FolderId, HashJobId, OccurrenceId, OperationId,
    PlanId, ProjectId, ScanRunId, SnapshotId, SourceRootId, TreeCloneSetId, VerificationRunId,
};
pub use inventory::{
    ContentObject, FileFingerprint, FolderRecord, HashState, PathOccurrence, ScanCounters,
    ScanEntryStatus, ScanRun, ScanRunStatus,
};
pub use plan::{
    ApprovalState, Confidence, ExecutionState, OperationErrorCode, OperationType, Plan,
    PlanOperation, PlanStatus, RiskLevel,
};
pub use project::{ProfileRef, Project};
pub use snapshot::{Snapshot, SnapshotStatus};
pub use source_root::{FileSystemKind, SourceRoot};
pub use state::ProjectState;
pub use structure::{FolderSignature, TreeCloneSet, TreeRelationship};

/// Canonical timestamp type of the engine (UTC, RFC 3339 when serialized).
pub type Timestamp = chrono::DateTime<chrono::Utc>;
