//! Domain model of the DataForge engine.
//!
//! This crate is pure: no I/O, no SQL, no clock other than `Utc::now()`.
//! Every state change flows through the [`state::ProjectState`] machine and
//! is later persisted and audited by `df-db` / `df-ledger`.

pub mod context;
pub mod duplicate_policy;
pub mod event;
pub mod fingerprint;
pub mod ids;
pub mod inventory;
pub mod manifest;
pub mod plan;
pub mod profile;
pub mod project;
pub mod raw_path;
pub mod snapshot;
pub mod source_root;
pub mod state;
pub mod structure;

pub use context::{ContextKind, FolderContext};
pub use duplicate_policy::{
    decide as decide_duplicate, DuplicateDisposition, DuplicateKind, DuplicatePolicy, Placement,
};
pub use event::{Actor, AuditEvent};
pub use fingerprint::{
    FileFingerprint, FingerprintGuarantee, FingerprintV1, FingerprintV2, FingerprintVerdict,
    PhysicalIdentity,
};
pub use ids::{
    ContentId, DuplicateSetId, EventId, FindingId, FolderId, HashJobId, OccurrenceId, OperationId,
    PlanId, ProjectId, ScanRunId, SnapshotId, SourceRootId, TreeCloneSetId, VerificationRunId,
};
pub use inventory::{
    ContentObject, FolderRecord, HashState, PathOccurrence, ScanCounters, ScanEntryStatus, ScanRun,
    ScanRunStatus,
};
pub use manifest::ManifestEntry;
pub use plan::{
    ApprovalState, Confidence, ExecutionState, OperationErrorCode, OperationType, Plan,
    PlanOperation, PlanStatus, RiskLevel,
};
pub use profile::{GenericMarker, Profile, ProtectedMarker, DEFAULT_PROFILE_ID};
pub use project::{ProfileRef, Project};
pub use raw_path::RawPath;
pub use snapshot::{Snapshot, SnapshotStatus};
pub use source_root::{FileSystemKind, SourceRoot};
pub use state::ProjectState;
pub use structure::{FolderSignature, TreeCloneSet, TreeRelation, TreeRelationship};

/// Canonical timestamp type of the engine (UTC, RFC 3339 when serialized).
pub type Timestamp = chrono::DateTime<chrono::Utc>;
