//! The [`Project`] aggregate (RFC-0001 §9.1).

use std::path::PathBuf;

use chrono::Utc;
use df_error::DfResult;
use serde::{Deserialize, Serialize};

use crate::{ids::ProjectId, ids::SourceRootId, state::ProjectState, Timestamp};

/// Reference to a rules/behaviour profile.
///
/// Profiles become real (files under `profiles/`) in Milestone 0.2; until
/// then only the name travels with the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileRef(String);

impl ProfileRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ProfileRef {
    fn default() -> Self {
        Self("generic".to_string())
    }
}

impl std::fmt::Display for ProfileRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A reconstruction project (RFC-0001 §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub state: ProjectState,
    pub profile: ProfileRef,
    pub source_roots: Vec<SourceRootId>,
    pub output_root: PathBuf,
    pub audit_root: PathBuf,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Version of the application that created the project.
    pub app_version: String,
}

impl Project {
    /// Create a new project in `CREATED` state.
    pub fn new(
        name: impl Into<String>,
        profile: ProfileRef,
        output_root: PathBuf,
        audit_root: PathBuf,
        app_version: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: ProjectId::new(),
            name: name.into(),
            state: ProjectState::Created,
            profile,
            source_roots: Vec::new(),
            output_root,
            audit_root,
            created_at: now,
            updated_at: now,
            app_version: app_version.into(),
        }
    }

    /// Apply a state machine transition, updating `updated_at` on success.
    ///
    /// This is the only supported way to mutate `state`.
    pub fn transition_to(&mut self, next: ProjectState) -> DfResult<()> {
        self.state = self.state.transition_to(next)?;
        self.updated_at = Utc::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Project {
        Project::new(
            "Migración servidor histórico",
            ProfileRef::default(),
            PathBuf::from("D:/salida"),
            PathBuf::from("D:/auditoria"),
            "0.0.1-dev",
        )
    }

    #[test]
    fn new_projects_start_in_created_state() {
        let p = sample();
        assert_eq!(p.state, ProjectState::Created);
        assert_eq!(p.profile.as_str(), "generic");
        assert!(p.source_roots.is_empty());
    }

    #[test]
    fn transition_updates_state_and_timestamp() {
        let mut p = sample();
        let before = p.updated_at;
        p.transition_to(ProjectState::Validating).expect("allowed");
        assert_eq!(p.state, ProjectState::Validating);
        assert!(p.updated_at >= before);
    }

    #[test]
    fn invalid_transition_leaves_project_untouched() {
        let mut p = sample();
        let before = p.clone();
        let err = p.transition_to(ProjectState::Executing).unwrap_err();
        assert!(matches!(err, df_error::DfError::InvalidTransition { .. }));
        assert_eq!(p, before);
    }

    #[test]
    fn project_serializes_with_canonical_state_names() {
        let p = sample();
        let json = serde_json::to_value(&p).expect("serialize");
        assert_eq!(json["state"], "CREATED");
        let back: Project = serde_json::from_value(json).expect("deserialize");
        assert_eq!(p, back);
    }
}
