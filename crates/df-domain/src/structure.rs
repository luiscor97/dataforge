//! Structural entities (RFC-0001 §18, §19).
//!
//! A folder signature is a Merkle hash of a directory computed bottom-up
//! from the content identities of its files and the signatures of its
//! subfolders (§19.2). Two folders with the same *complete* signature hold
//! byte-for-byte the same tree, which is how grafted / cloned directories
//! are detected (§19.3).
//!
//! This module is pure data: the computation lives in `df-db::structure`.

use serde::{Deserialize, Serialize};

use crate::ids::{FolderId, SnapshotId, SourceRootId, TreeCloneSetId};

/// Structural relationship between two or more folders (RFC-0001 §19.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TreeRelationship {
    /// Two or more folders whose complete subtrees are byte-for-byte equal.
    ExactClone,
    /// Every content of one folder's subtree is also in the other's, which
    /// holds strictly more. The smaller is *embedded* in the larger.
    Embedded,
    /// The two subtrees share content but **both** hold something the other
    /// does not. Neither may be dropped for the other: this is exactly the
    /// case §19.4 warns about.
    PartialClone,
    /// They share content, but so little that calling them related would be
    /// misleading — typically a handful of common files (a logo, a template)
    /// repeated across unrelated folders. Representable so the vocabulary is
    /// stable; not emitted, since it is indistinguishable from coincidence
    /// without the entity graph (§18.2).
    RepeatedComponentOnly,
}

impl TreeRelationship {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactClone => "EXACT_TREE_CLONE",
            Self::Embedded => "TREE_EMBEDDED",
            Self::PartialClone => "PARTIAL_TREE_CLONE",
            Self::RepeatedComponentOnly => "REPEATED_COMPONENT_ONLY",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "EXACT_TREE_CLONE" => Ok(Self::ExactClone),
            "TREE_EMBEDDED" => Ok(Self::Embedded),
            "PARTIAL_TREE_CLONE" => Ok(Self::PartialClone),
            "REPEATED_COMPONENT_ONLY" => Ok(Self::RepeatedComponentOnly),
            other => Err(df_error::DfError::Validation(format!(
                "unknown tree relationship `{other}`"
            ))),
        }
    }
}

/// A pairwise structural relationship between two folders (§19.3), with the
/// evidence that justifies it.
///
/// `unique_a`/`unique_b` are the whole point: they are the content that would
/// be lost if someone dropped that branch believing it was a duplicate
/// (§19.4). A relation with unique content on both sides is a warning, not a
/// consolidation opportunity.
// No `Eq`: `similarity` is an f64, and f64 is only PartialEq (NaN).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeRelation {
    pub snapshot_id: SnapshotId,
    pub folder_a: FolderId,
    pub folder_b: FolderId,
    pub relationship: TreeRelationship,
    /// Distinct contents present in both subtrees.
    pub shared_files: u64,
    /// Distinct contents only in `folder_a`.
    pub unique_a_files: u64,
    /// Distinct contents only in `folder_b`.
    pub unique_b_files: u64,
    pub shared_bytes: u64,
    /// Jaccard index over distinct contents: `shared / (shared + only_a +
    /// only_b)`, in `[0, 1]`.
    pub similarity: f64,
}

/// Merkle signature of one folder within a snapshot (RFC-0001 §19.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSignature {
    pub folder_id: FolderId,
    pub snapshot_id: SnapshotId,
    pub source_root_id: SourceRootId,
    /// Path relative to the source root; empty string for the root itself.
    pub relative_path: String,
    /// BLAKE3 hex of the folder, present only when the subtree is complete.
    pub signature: Option<String>,
    /// A subtree is complete when every descendant file has a content hash
    /// and it contains no error entries or unfollowed reparse points. Only
    /// complete folders may take part in a clone set (safety, §19.4).
    pub is_complete: bool,
    /// Number of file occurrences anywhere in the subtree.
    pub subtree_files: u64,
    /// Total bytes of those file occurrences.
    pub subtree_bytes: u64,
}

/// A group of folders that share the same complete signature (§19.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeCloneSet {
    pub id: TreeCloneSetId,
    pub snapshot_id: SnapshotId,
    pub signature: String,
    pub relationship: TreeRelationship,
    /// Absolute paths of the cloned folders, sorted.
    pub folders: Vec<String>,
    pub subtree_files: u64,
    pub subtree_bytes: u64,
}

impl TreeCloneSet {
    /// Bytes that are redundant across the clones: every copy beyond the
    /// first repeats `subtree_bytes` (report only — §19.4 forbids removing a
    /// branch before its unique content is identified).
    pub fn redundant_bytes(&self) -> u64 {
        self.subtree_bytes
            .saturating_mul(self.folders.len().saturating_sub(1) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_relationship_round_trips() {
        for relationship in [
            TreeRelationship::ExactClone,
            TreeRelationship::Embedded,
            TreeRelationship::PartialClone,
            TreeRelationship::RepeatedComponentOnly,
        ] {
            assert_eq!(
                TreeRelationship::parse(relationship.as_str()).unwrap(),
                relationship
            );
        }
        assert!(TreeRelationship::parse("SOMETHING_ELSE").is_err());
    }

    #[test]
    fn redundant_bytes_counts_copies_beyond_the_first() {
        let set = TreeCloneSet {
            id: TreeCloneSetId::new(),
            snapshot_id: SnapshotId::new(),
            signature: "a".repeat(64),
            relationship: TreeRelationship::ExactClone,
            folders: vec!["A".into(), "B".into(), "C".into()],
            subtree_files: 4,
            subtree_bytes: 100,
        };
        assert_eq!(set.redundant_bytes(), 200);
    }
}
