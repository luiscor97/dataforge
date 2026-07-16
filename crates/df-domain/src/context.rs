//! Folder context classification (RFC-0001 §18).
//!
//! A first, deterministic slice of the context graph: each folder is tagged
//! as a low-value *generic* container (Descargas, Escritorio, Backup,
//! Recuperado, Copia, Temporales — §18.3) with a penalty weight, as a
//! *protected* boundary (only when a profile declares protecting markers),
//! or *neutral*. Entity anchors, weighted propagation and the full graph
//! (§18.2–§18.4) come in a later slice.
//!
//! This module is pure data: the classification logic lives in
//! `df-db::context`.

use serde::{Deserialize, Serialize};

use crate::ids::{FolderId, SnapshotId};

/// Context class of a folder (RFC-0001 §18.3, §9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContextKind {
    /// A boundary that deduplication must not collapse (e.g. an expediente or
    /// pericial). Only assigned when the active profile declares protecting
    /// markers; the conservative `generic` profile never does (§25.4).
    Protected,
    /// A low-value generic container (downloads, desktop, backup, copies,
    /// recovered, temporaries). Carries a penalty weight (§18.3).
    Generic,
    /// Everything else: no signal either way.
    Neutral,
}

impl ContextKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Protected => "PROTECTED",
            Self::Generic => "GENERIC",
            Self::Neutral => "NEUTRAL",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "PROTECTED" => Ok(Self::Protected),
            "GENERIC" => Ok(Self::Generic),
            "NEUTRAL" => Ok(Self::Neutral),
            other => Err(df_error::DfError::Validation(format!(
                "unknown context kind `{other}`"
            ))),
        }
    }
}

/// Context classification of one folder within a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderContext {
    pub folder_id: FolderId,
    pub snapshot_id: SnapshotId,
    /// Path relative to the source root; empty string for the root itself.
    pub relative_path: String,
    pub kind: ContextKind,
    /// A protected boundary must never be dissolved by duplicate
    /// consolidation (RFC-0001 rule 9, §15.2).
    pub is_protected_boundary: bool,
    /// Ranking penalty of this folder as a *representative* location: higher
    /// means a worse place for the canonical copy (§18.3). 0 for neutral and
    /// protected folders.
    pub penalty: u32,
    /// The marker that triggered a non-neutral classification, if any.
    pub marker: Option<String>,
    /// Profile-authored explanation for a protected boundary.
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_kind_round_trips() {
        for kind in [
            ContextKind::Protected,
            ContextKind::Generic,
            ContextKind::Neutral,
        ] {
            assert_eq!(ContextKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert!(ContextKind::parse("SOMETHING").is_err());
    }
}
