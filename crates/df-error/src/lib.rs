//! Shared, typed error surface for every DataForge crate.
//!
//! Crates map their internal failures into [`DfError`] so that clients
//! (CLI, desktop facade) can translate errors into stable exit codes and
//! user-facing messages without depending on implementation crates.

use std::path::PathBuf;

use thiserror::Error;

/// Convenience alias used across the workspace.
pub type DfResult<T> = Result<T, DfError>;

/// Canonical error type of the DataForge engine.
#[derive(Debug, Error)]
pub enum DfError {
    /// A project state machine transition that the domain forbids.
    #[error("invalid state transition from `{from}` to `{to}`")]
    InvalidTransition { from: String, to: String },

    /// Input rejected before any side effect happened.
    #[error("validation failed: {0}")]
    Validation(String),

    /// A requested entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The operation conflicts with existing state (e.g. duplicate project).
    #[error("conflict: {0}")]
    Conflict(String),

    /// SQLite / persistence failure.
    #[error("database error: {0}")]
    Database(String),

    /// The audit ledger chain failed cryptographic verification.
    #[error("ledger integrity violation: {0}")]
    LedgerIntegrity(String),

    /// (De)serialization failure of a versioned format.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Filesystem failure, annotated with the offending path.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl DfError {
    /// Helper to build an [`DfError::Io`] with path context.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Stable process exit code, documented in RFC-0001 §33.
    ///
    /// ```text
    /// 0 success | 1 generic failure | 2 validation failure
    /// 3 partial completion | 4 verification failure
    /// 5 permission failure | 6 insufficient space
    /// ```
    pub fn exit_code(&self) -> i32 {
        match self {
            DfError::Validation(_) | DfError::InvalidTransition { .. } => 2,
            DfError::LedgerIntegrity(_) => 4,
            DfError::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied => {
                5
            }
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_rfc_0001_section_33() {
        assert_eq!(DfError::Validation("x".into()).exit_code(), 2);
        assert_eq!(
            DfError::InvalidTransition {
                from: "CREATED".into(),
                to: "EXECUTING".into()
            }
            .exit_code(),
            2
        );
        assert_eq!(DfError::LedgerIntegrity("broken".into()).exit_code(), 4);
        assert_eq!(DfError::Database("x".into()).exit_code(), 1);
        let denied = DfError::io(
            "C:/x",
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(denied.exit_code(), 5);
    }
}
