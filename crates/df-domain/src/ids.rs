//! Typed identifiers.
//!
//! Every entity gets its own newtype around a UUIDv4 so that a `ProjectId`
//! can never be passed where a `SnapshotId` is expected.

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Generate a fresh random identifier.
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }

            /// Access the underlying UUID.
            pub fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = df_error::DfError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(s).map(Self).map_err(|e| {
                    df_error::DfError::Validation(format!(
                        "`{s}` is not a valid {}: {e}",
                        stringify!($name)
                    ))
                })
            }
        }
    };
}

typed_id!(
    /// Identifier of a [`crate::Project`].
    ProjectId
);
typed_id!(
    /// Identifier of a [`crate::SourceRoot`].
    SourceRootId
);
typed_id!(
    /// Identifier of a [`crate::Snapshot`].
    SnapshotId
);
typed_id!(
    /// Identifier of an [`crate::AuditEvent`].
    EventId
);
typed_id!(
    /// Identifier of a [`crate::ScanRun`].
    ScanRunId
);
typed_id!(
    /// Identifier of a [`crate::FolderRecord`].
    FolderId
);
typed_id!(
    /// Identifier of a [`crate::PathOccurrence`].
    OccurrenceId
);
typed_id!(
    /// Identifier of a [`crate::ContentObject`].
    ContentId
);
typed_id!(
    /// Identifier of a hash job (RFC-0001 §14, `hash_jobs` table).
    HashJobId
);
typed_id!(
    /// Identifier of an exact duplicate set (RFC-0001 §15).
    DuplicateSetId
);
typed_id!(
    /// Identifier of a [`crate::Plan`].
    PlanId
);
typed_id!(
    /// Identifier of a [`crate::PlanOperation`].
    OperationId
);
typed_id!(
    /// Identifier of a verification run (RFC-0001 §28).
    VerificationRunId
);
typed_id!(
    /// Identifier of a verification finding (RFC-0001 §28).
    FindingId
);
typed_id!(
    /// Identifier of a tree-clone set (RFC-0001 §19).
    TreeCloneSetId
);
typed_id!(
    /// Identifier of a normalized content-defined chunk (RFC-0001 §20).
    ChunkId
);
typed_id!(
    /// Identifier of one reproducible similarity analysis run (RFC-0001 §20).
    SimilarityRunId
);
typed_id!(
    /// Identifier of an evidence-backed relationship between two contents.
    SimilarityRelationId
);
typed_id!(
    /// Identifier of one reproducible document-extraction run (M0.4).
    ExtractionRunId
);
typed_id!(
    /// Identifier of one immutable physical document representation.
    RepresentationId
);
typed_id!(
    /// Identifier of a searchable physical or embedded text subject.
    TextSubjectId
);
typed_id!(
    /// Identifier of an attachment decoded from an EML message.
    MailAttachmentId
);
typed_id!(
    /// Identifier of a virtual entry inside an archive.
    ArchiveEntryId
);
typed_id!(
    /// Identifier of a reconstructed basic mail thread.
    MailThreadId
);
typed_id!(
    /// Identifier of a rebuildable Tantivy index artifact.
    SearchIndexId
);
typed_id!(
    /// Identifier of an analytical Parquet artifact.
    AnalyticalSnapshotId
);

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn ids_round_trip_through_display_and_from_str() {
        let id = ProjectId::new();
        let text = id.to_string();
        let parsed = ProjectId::from_str(&text).expect("round trip");
        assert_eq!(id, parsed);
    }

    #[test]
    fn invalid_id_is_a_validation_error() {
        let err = ProjectId::from_str("not-a-uuid").unwrap_err();
        assert!(matches!(err, df_error::DfError::Validation(_)));
    }

    #[test]
    fn ids_serialize_as_plain_uuid_strings() {
        let id = SnapshotId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{id}\""));
        let back: SnapshotId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }
}
