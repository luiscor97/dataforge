//! Source roots (RFC-0001 §9.2): the immutable origins of a project.

use std::path::PathBuf;

use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, SourceRootId};

/// Filesystem family of a source root.
///
/// Real detection belongs to the validation phase (Milestone 0.1); until
/// then roots are stored as [`FileSystemKind::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FileSystemKind {
    Ntfs,
    ReFs,
    Fat32,
    ExFat,
    Network,
    Unknown,
}

impl FileSystemKind {
    /// Whether ADR-0019 physical identity (volume serial + file id) is
    /// available: the foundation of substitution detection, incremental
    /// reuse and the strong-identity leases. Network shares, FAT variants
    /// and unknown filesystems only offer degraded guarantees.
    pub fn has_physical_identity(self) -> bool {
        matches!(self, Self::Ntfs | Self::ReFs)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ntfs => "NTFS",
            Self::ReFs => "REFS",
            Self::Fat32 => "FAT32",
            Self::ExFat => "EXFAT",
            Self::Network => "NETWORK",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn parse(value: &str) -> DfResult<Self> {
        match value {
            "NTFS" => Ok(Self::Ntfs),
            "REFS" => Ok(Self::ReFs),
            "FAT32" => Ok(Self::Fat32),
            "EXFAT" => Ok(Self::ExFat),
            "NETWORK" => Ok(Self::Network),
            "UNKNOWN" => Ok(Self::Unknown),
            other => Err(DfError::Validation(format!(
                "unknown filesystem kind `{other}`"
            ))),
        }
    }
}

/// An origin directory that DataForge must never modify (RFC-0001 rule 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRoot {
    pub id: SourceRootId,
    pub project_id: ProjectId,
    pub absolute_path: PathBuf,
    pub volume_id: Option<String>,
    pub filesystem: FileSystemKind,
    pub is_network: bool,
    pub is_removable: bool,
    /// Always `true` in the MVP: the origin is read-only by policy.
    pub read_only_policy: bool,
}

impl SourceRoot {
    /// Register a new source root for a project.
    ///
    /// `read_only_policy` is forced to `true`; there is no constructor that
    /// produces a writable origin.
    pub fn new(project_id: ProjectId, absolute_path: PathBuf) -> Self {
        Self {
            id: SourceRootId::new(),
            project_id,
            absolute_path,
            volume_id: None,
            filesystem: FileSystemKind::Unknown,
            is_network: false,
            is_removable: false,
            read_only_policy: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_roots_are_read_only_by_construction() {
        let root = SourceRoot::new(ProjectId::new(), PathBuf::from("D:/origen"));
        assert!(root.read_only_policy);
        assert_eq!(root.filesystem, FileSystemKind::Unknown);
    }

    #[test]
    fn filesystem_kind_round_trips() {
        for kind in [
            FileSystemKind::Ntfs,
            FileSystemKind::ReFs,
            FileSystemKind::Fat32,
            FileSystemKind::ExFat,
            FileSystemKind::Network,
            FileSystemKind::Unknown,
        ] {
            assert_eq!(FileSystemKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert!(FileSystemKind::parse("EXT4?").is_err());
    }
}
