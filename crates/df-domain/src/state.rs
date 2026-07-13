//! Project state machine, exactly as specified in RFC-0001 §11.
//!
//! Invariants enforced here:
//! - a state only changes through [`ProjectState::transition_to`];
//! - forbidden transitions return [`DfError::InvalidTransition`];
//! - `ARCHIVED` currently has no inbound transition in the RFC table, so it
//!   is representable but unreachable until a future RFC/ADR defines it.

use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};

/// Lifecycle state of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProjectState {
    Created,
    Validating,
    Ready,
    Scanning,
    ScanPaused,
    Scanned,
    Hashing,
    HashPaused,
    Hashed,
    Analyzing,
    AnalysisPaused,
    Analyzed,
    Planning,
    PlanReady,
    PlanReview,
    PlanApproved,
    Executing,
    ExecutionPaused,
    Executed,
    Verifying,
    Completed,
    CompletedWithWarnings,
    Failed,
    Archived,
}

impl ProjectState {
    /// Canonical uppercase name used in SQLite and exports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Validating => "VALIDATING",
            Self::Ready => "READY",
            Self::Scanning => "SCANNING",
            Self::ScanPaused => "SCAN_PAUSED",
            Self::Scanned => "SCANNED",
            Self::Hashing => "HASHING",
            Self::HashPaused => "HASH_PAUSED",
            Self::Hashed => "HASHED",
            Self::Analyzing => "ANALYZING",
            Self::AnalysisPaused => "ANALYSIS_PAUSED",
            Self::Analyzed => "ANALYZED",
            Self::Planning => "PLANNING",
            Self::PlanReady => "PLAN_READY",
            Self::PlanReview => "PLAN_REVIEW",
            Self::PlanApproved => "PLAN_APPROVED",
            Self::Executing => "EXECUTING",
            Self::ExecutionPaused => "EXECUTION_PAUSED",
            Self::Executed => "EXECUTED",
            Self::Verifying => "VERIFYING",
            Self::Completed => "COMPLETED",
            Self::CompletedWithWarnings => "COMPLETED_WITH_WARNINGS",
            Self::Failed => "FAILED",
            Self::Archived => "ARCHIVED",
        }
    }

    /// Parse a canonical state name.
    pub fn parse(value: &str) -> DfResult<Self> {
        ALL_STATES
            .iter()
            .copied()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| DfError::Validation(format!("unknown project state `{value}`")))
    }

    /// Whether the RFC-0001 §11 transition table allows `self → next`.
    pub fn can_transition_to(self, next: ProjectState) -> bool {
        use ProjectState::*;
        matches!(
            (self, next),
            (Created, Validating)
                | (Validating, Ready | Failed)
                | (Ready, Scanning)
                | (Scanning, ScanPaused | Scanned | Failed)
                | (ScanPaused, Scanning | Failed)
                | (Scanned, Hashing)
                | (Hashing, HashPaused | Hashed | Failed)
                | (HashPaused, Hashing | Failed)
                | (Hashed, Analyzing)
                | (Analyzing, AnalysisPaused | Analyzed | Failed)
                | (AnalysisPaused, Analyzing | Failed)
                | (Analyzed, Planning)
                | (Planning, PlanReady | Failed)
                | (PlanReady, PlanReview)
                | (PlanReview, PlanReview | PlanApproved)
                | (PlanApproved, Executing)
                | (Executing, ExecutionPaused | Executed | Failed)
                | (ExecutionPaused, Executing | Failed)
                | (Executed, Verifying)
                | (Verifying, Completed | CompletedWithWarnings | Failed)
        )
    }

    /// Validate a transition, returning the new state on success.
    pub fn transition_to(self, next: ProjectState) -> DfResult<ProjectState> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(DfError::InvalidTransition {
                from: self.as_str().to_string(),
                to: next.as_str().to_string(),
            })
        }
    }
}

impl std::fmt::Display for ProjectState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every representable state, in RFC order.
pub const ALL_STATES: [ProjectState; 24] = [
    ProjectState::Created,
    ProjectState::Validating,
    ProjectState::Ready,
    ProjectState::Scanning,
    ProjectState::ScanPaused,
    ProjectState::Scanned,
    ProjectState::Hashing,
    ProjectState::HashPaused,
    ProjectState::Hashed,
    ProjectState::Analyzing,
    ProjectState::AnalysisPaused,
    ProjectState::Analyzed,
    ProjectState::Planning,
    ProjectState::PlanReady,
    ProjectState::PlanReview,
    ProjectState::PlanApproved,
    ProjectState::Executing,
    ProjectState::ExecutionPaused,
    ProjectState::Executed,
    ProjectState::Verifying,
    ProjectState::Completed,
    ProjectState::CompletedWithWarnings,
    ProjectState::Failed,
    ProjectState::Archived,
];

#[cfg(test)]
mod tests {
    use super::ProjectState::*;
    use super::*;

    #[test]
    fn every_state_round_trips_through_its_canonical_name() {
        for state in ALL_STATES {
            assert_eq!(ProjectState::parse(state.as_str()).unwrap(), state);
        }
    }

    #[test]
    fn happy_path_pipeline_is_fully_allowed() {
        let path = [
            Created,
            Validating,
            Ready,
            Scanning,
            Scanned,
            Hashing,
            Hashed,
            Analyzing,
            Analyzed,
            Planning,
            PlanReady,
            PlanReview,
            PlanApproved,
            Executing,
            Executed,
            Verifying,
            Completed,
        ];
        let mut current = path[0];
        for next in &path[1..] {
            current = current.transition_to(*next).expect("allowed transition");
        }
        assert_eq!(current, Completed);
    }

    #[test]
    fn pause_and_resume_transitions_are_allowed() {
        assert!(Scanning.can_transition_to(ScanPaused));
        assert!(ScanPaused.can_transition_to(Scanning));
        assert!(Hashing.can_transition_to(HashPaused));
        assert!(HashPaused.can_transition_to(Hashing));
        assert!(Executing.can_transition_to(ExecutionPaused));
        assert!(ExecutionPaused.can_transition_to(Executing));
    }

    #[test]
    fn plan_review_may_loop_on_itself() {
        assert!(PlanReview.can_transition_to(PlanReview));
        assert!(PlanReview.can_transition_to(PlanApproved));
    }

    #[test]
    fn forbidden_transitions_are_rejected_with_typed_error() {
        let err = Created.transition_to(Executing).unwrap_err();
        match err {
            DfError::InvalidTransition { from, to } => {
                assert_eq!(from, "CREATED");
                assert_eq!(to, "EXECUTING");
            }
            other => panic!("expected InvalidTransition, got {other:?}"),
        }
        assert!(Completed.transition_to(Created).is_err());
        assert!(Failed.transition_to(Ready).is_err());
        // No phase skipping (RFC-0001 §11 invariant).
        assert!(Ready.transition_to(Hashing).is_err());
        assert!(PlanReady.transition_to(Executing).is_err());
    }

    #[test]
    fn archived_is_currently_unreachable() {
        for state in ALL_STATES {
            assert!(
                !state.can_transition_to(Archived),
                "ARCHIVED must stay unreachable until an ADR defines its inbound transition"
            );
        }
    }

    #[test]
    fn terminal_states_have_no_outbound_transitions() {
        for state in [Completed, CompletedWithWarnings, Failed, Archived] {
            for next in ALL_STATES {
                assert!(
                    !state.can_transition_to(next),
                    "{state} → {next} must be forbidden"
                );
            }
        }
    }
}
