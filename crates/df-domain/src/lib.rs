//! Domain model of the DataForge engine.
//!
//! This crate is pure: no I/O, no SQL, no clock other than `Utc::now()`.
//! Every state change flows through the [`state::ProjectState`] machine and
//! is later persisted and audited by `df-db` / `df-ledger`.

pub mod content;
pub mod event;
pub mod ids;
pub mod occurrence;
pub mod project;
pub mod snapshot;
pub mod source_root;
pub mod state;

pub use content::{ContentObject, HashMode, HashState};
pub use event::{Actor, AuditEvent};
pub use ids::{
    ContentId, DuplicateSetId, EventId, FolderId, HashJobId, OccurrenceId, ProjectId, ScanRunId,
    SnapshotId, SourceRootId,
};
pub use occurrence::{
    EntryKind, Fingerprint, FolderEntry, PathOccurrence, ScanCounters, ScanRun, ScanRunStatus,
    ScanStatus,
};
pub use project::{ProfileRef, Project};
pub use snapshot::{Snapshot, SnapshotStatus};
pub use source_root::{FileSystemKind, SourceRoot};
pub use state::ProjectState;

/// Canonical timestamp type of the engine (UTC, RFC 3339 when serialized).
pub type Timestamp = chrono::DateTime<chrono::Utc>;
