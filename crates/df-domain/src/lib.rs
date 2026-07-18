//! Domain model of the DataForge engine.
//!
//! This crate is pure: no I/O, no SQL, no clock other than `Utc::now()`.
//! Every state change flows through the [`state::ProjectState`] machine and
//! is later persisted and audited by `df-db` / `df-ledger`.

pub mod analysis;
pub mod context;
pub mod duplicate_policy;
pub mod event;
pub mod extraction;
pub mod fingerprint;
pub mod ids;
pub mod inventory;
pub mod manifest;
pub mod media;
pub mod plan;
pub mod profile;
pub mod project;
pub mod raw_path;
pub mod similarity;
pub mod snapshot;
pub mod source_root;
pub mod state;
pub mod structure;

pub use analysis::{
    AnomalyKind, AnomalySeverity, RuleAction, RuleClassification, RuleDefinition, RuleMatch,
};
pub use context::{ContextKind, FolderContext};
pub use duplicate_policy::{
    decide as decide_duplicate, DuplicateDisposition, DuplicateKind, DuplicatePolicy, Placement,
};
pub use event::{Actor, AuditEvent};
pub use extraction::{
    AnalyticalSnapshotArtifact, ArchiveEntry, DocumentFormat, DocumentRepresentation,
    ExtractionRun, ExtractionRunCounters, ExtractionRunStatus, ExtractionStatus, MailAttachment,
    MailMessage, MailThread, MailThreadMember, SearchIndexArtifact, TextSubject, TextSubjectKind,
};
pub use fingerprint::{
    FileFingerprint, FingerprintGuarantee, FingerprintV1, FingerprintV2, FingerprintVerdict,
    PhysicalIdentity,
};
pub use ids::{
    AnalyticalSnapshotId, ArchiveEntryId, ChunkId, ContentId, DuplicateSetId, EventId,
    ExtractionRunId, FindingId, FolderId, HashJobId, MailAttachmentId, MailThreadId,
    MediaEvidenceId, MediaRelationId, MediaRunId, OccurrenceId, OperationId, PlanId, ProjectId,
    RepresentationId, ScanRunId, SearchIndexId, SimilarityRelationId, SimilarityRunId, SnapshotId,
    SourceRootId, TextSubjectId, TreeCloneSetId, VerificationRunId,
};
pub use inventory::{
    ContentObject, FolderRecord, HashState, PathOccurrence, ScanCounters, ScanEntryStatus, ScanRun,
    ScanRunStatus,
};
pub use manifest::ManifestEntry;
pub use media::{
    MediaRelationKind, MediaRelationRecord, MediaRun, MediaRunCounters, MediaRunStatus,
};
pub use plan::{
    ApprovalState, Confidence, ExecutionState, OperationErrorCode, OperationType, Plan,
    PlanOperation, PlanStatus, RiskLevel,
};
pub use profile::{GenericMarker, Profile, ProtectedMarker, DEFAULT_PROFILE_ID};
pub use project::{ProfileRef, Project};
pub use raw_path::RawPath;
pub use similarity::{
    ContentRelationship, ContentRelationshipKind, RelationshipDirection, SimilarityRun,
    SimilarityRunCounters, SimilarityRunStatus,
};
pub use snapshot::{Snapshot, SnapshotStatus};
pub use source_root::{FileSystemKind, SourceRoot};
pub use state::ProjectState;
pub use structure::{FolderSignature, TreeCloneSet, TreeRelation, TreeRelationship};

/// Canonical timestamp type of the engine (UTC, RFC 3339 when serialized).
pub type Timestamp = chrono::DateTime<chrono::Utc>;
