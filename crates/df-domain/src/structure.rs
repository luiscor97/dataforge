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
///
/// Milestone 0.2 implements [`TreeRelationship::ExactClone`] only; the
/// partial and embedded variants are named here so the vocabulary is stable
/// but are computed in a later slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TreeRelationship {
    /// Two or more folders whose complete subtrees are byte-for-byte equal.
    ExactClone,
}

impl TreeRelationship {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactClone => "EXACT_TREE_CLONE",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "EXACT_TREE_CLONE" => Ok(Self::ExactClone),
            other => Err(df_error::DfError::Validation(format!(
                "unknown tree relationship `{other}`"
            ))),
        }
    }
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
        assert_eq!(
            TreeRelationship::parse(TreeRelationship::ExactClone.as_str()).unwrap(),
            TreeRelationship::ExactClone
        );
        assert!(TreeRelationship::parse("PARTIAL_TREE_CLONE").is_err());
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
