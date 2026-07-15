//! Duplicate consolidation policy (RFC-0001 §15.2, §15.3, §15.4, rule 9).
//!
//! Exact duplicates are *evidence*, not a verdict: "un duplicado exacto no es
//! automáticamente prescindible" (rule 8). What may be consolidated depends on
//! **where** each copy lives, and the answer is a policy the user chooses —
//! never a silent default.
//!
//! This module is pure: it maps `(policy, kind, placement)` to a disposition.
//! Classification of folders lives in `df-db::context`, the representative
//! ranking in `df-db::dedup`, and the plan emission in `df-planner`.
//!
//! # The hard rule
//!
//! A copy inside a **protected boundary** is never consolidated away, whatever
//! the policy says (rule 9: "contextos protegidos prevalecen sobre
//! deduplicación agresiva"). `CONSOLIDATE_ALL` is not an escape hatch: it
//! consolidates *everything it is allowed to*, which never includes a
//! protected context. Anything DataForge cannot classify is copied, not
//! dropped: uncertainty resolves towards preservation.

use serde::{Deserialize, Serialize};

/// What the user authorises DataForge to do with exact duplicates (§15.4).
///
/// The default is [`DuplicatePolicy::ReportOnly`]: report the evidence and
/// copy everything. Consolidation is always opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DuplicatePolicy {
    /// Copy every occurrence; only report the duplicate sets. Safe default.
    #[default]
    ReportOnly,
    /// Consolidate only when every copy lives in the same context, where
    /// keeping N identical files adds nothing.
    ConsolidateWithinContext,
    /// Consolidate copies that sit in low-value generic containers
    /// (Descargas, Escritorio, Backup, Copia…) towards the canonical one.
    ConsolidateGenericCopies,
    /// Consolidate every duplicate onto its representative — except across
    /// protected boundaries, which are never dissolved.
    ConsolidateAll,
    /// Never consolidate: copy every occurrence, explicitly and on purpose.
    PreserveAll,
}

impl DuplicatePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReportOnly => "REPORT_ONLY",
            Self::ConsolidateWithinContext => "CONSOLIDATE_WITHIN_CONTEXT",
            Self::ConsolidateGenericCopies => "CONSOLIDATE_GENERIC_COPIES",
            Self::ConsolidateAll => "CONSOLIDATE_ALL",
            Self::PreserveAll => "PRESERVE_ALL",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "REPORT_ONLY" => Ok(Self::ReportOnly),
            "CONSOLIDATE_WITHIN_CONTEXT" => Ok(Self::ConsolidateWithinContext),
            "CONSOLIDATE_GENERIC_COPIES" => Ok(Self::ConsolidateGenericCopies),
            "CONSOLIDATE_ALL" => Ok(Self::ConsolidateAll),
            "PRESERVE_ALL" => Ok(Self::PreserveAll),
            other => Err(df_error::DfError::Validation(format!(
                "unknown duplicate policy `{other}`"
            ))),
        }
    }

    /// Whether this policy may drop any copy at all. Useful for the UI and to
    /// keep `REPORT_ONLY`/`PRESERVE_ALL` provably side-effect free.
    pub fn may_consolidate(self) -> bool {
        !matches!(self, Self::ReportOnly | Self::PreserveAll)
    }

    pub const ALL: [DuplicatePolicy; 5] = [
        Self::ReportOnly,
        Self::ConsolidateWithinContext,
        Self::ConsolidateGenericCopies,
        Self::ConsolidateAll,
        Self::PreserveAll,
    ];
}

/// The shape of a duplicate set, derived from the contexts of its members
/// (§15.3). Computed per set, not per occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DuplicateKind {
    /// Every copy lives under the same context.
    WithinSameContext,
    /// Copies live in two or more *distinct protected* contexts. Consolidating
    /// would dissolve a boundary; never allowed (rule 9).
    AcrossProtectedContexts,
    /// Some copies sit in generic containers and at least one does not: the
    /// non-generic one is the canonical candidate.
    GenericToCanonical,
    /// Copies that exist only as replicas inside a backup container.
    BackupReplica,
    /// The relation between the copies' contexts could not be established.
    /// Conservative: treated as "do not touch".
    UnknownContext,
    /// An active copy versus one in an excluded area. Representable for
    /// forward-compatibility of stored plans; not emitted yet — exclusion
    /// areas arrive with declarative rules (§25).
    ActiveToExcluded,
}

impl DuplicateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WithinSameContext => "WITHIN_SAME_CONTEXT",
            Self::AcrossProtectedContexts => "ACROSS_PROTECTED_CONTEXTS",
            Self::GenericToCanonical => "GENERIC_TO_CANONICAL",
            Self::BackupReplica => "BACKUP_REPLICA",
            Self::UnknownContext => "UNKNOWN_CONTEXT",
            Self::ActiveToExcluded => "ACTIVE_TO_EXCLUDED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "WITHIN_SAME_CONTEXT" => Ok(Self::WithinSameContext),
            "ACROSS_PROTECTED_CONTEXTS" => Ok(Self::AcrossProtectedContexts),
            "GENERIC_TO_CANONICAL" => Ok(Self::GenericToCanonical),
            "BACKUP_REPLICA" => Ok(Self::BackupReplica),
            "UNKNOWN_CONTEXT" => Ok(Self::UnknownContext),
            "ACTIVE_TO_EXCLUDED" => Ok(Self::ActiveToExcluded),
            other => Err(df_error::DfError::Validation(format!(
                "unknown duplicate kind `{other}`"
            ))),
        }
    }
}

/// Where one particular occurrence of the duplicated content sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// This occurrence is the set's logical representative (§15.5).
    pub is_representative: bool,
    /// This occurrence lives inside a protected boundary.
    pub in_protected_context: bool,
    /// This occurrence lives inside a low-value generic container.
    pub in_generic_context: bool,
}

/// What the plan should do with one occurrence of a duplicate set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateDisposition {
    /// Copy it: it is the representative, the policy preserves everything, or
    /// we cannot prove it is redundant.
    Copy,
    /// Do not copy: the representative already carries this exact content.
    SkipRepresented,
    /// Copy it *and* record why it survived: it sits in a protected boundary
    /// that consolidation must not dissolve.
    PreserveAcrossContext,
}

/// Decide what to do with a single occurrence.
///
/// Order matters and encodes the safety rules:
/// 1. a protected boundary always wins (rule 9);
/// 2. the representative is always copied — something must carry the content;
/// 3. anything unclassified is copied (§15.2: do not infer redundancy);
/// 4. only then does the policy get to consolidate.
pub fn decide(
    policy: DuplicatePolicy,
    kind: DuplicateKind,
    placement: Placement,
) -> DuplicateDisposition {
    // 1. Protected boundaries are never dissolved, whatever the policy.
    if placement.in_protected_context || kind == DuplicateKind::AcrossProtectedContexts {
        return if policy.may_consolidate() {
            // Say out loud that it survived *because* it is protected.
            DuplicateDisposition::PreserveAcrossContext
        } else {
            DuplicateDisposition::Copy
        };
    }

    // 2. The representative always materialises.
    if placement.is_representative {
        return DuplicateDisposition::Copy;
    }

    // 3. Policies that never drop anything.
    if !policy.may_consolidate() {
        return DuplicateDisposition::Copy;
    }

    // 4. Uncertainty preserves.
    if matches!(
        kind,
        DuplicateKind::UnknownContext | DuplicateKind::ActiveToExcluded
    ) {
        return DuplicateDisposition::Copy;
    }

    match policy {
        DuplicatePolicy::ConsolidateWithinContext => match kind {
            DuplicateKind::WithinSameContext => DuplicateDisposition::SkipRepresented,
            _ => DuplicateDisposition::Copy,
        },
        DuplicatePolicy::ConsolidateGenericCopies => match kind {
            // Only the copies actually sitting in a generic container go.
            DuplicateKind::GenericToCanonical | DuplicateKind::BackupReplica
                if placement.in_generic_context =>
            {
                DuplicateDisposition::SkipRepresented
            }
            _ => DuplicateDisposition::Copy,
        },
        DuplicatePolicy::ConsolidateAll => DuplicateDisposition::SkipRepresented,
        // Handled in step 3; listed for exhaustiveness.
        DuplicatePolicy::ReportOnly | DuplicatePolicy::PreserveAll => DuplicateDisposition::Copy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Placement {
        Placement {
            is_representative: false,
            in_protected_context: false,
            in_generic_context: false,
        }
    }

    #[test]
    fn policies_and_kinds_round_trip() {
        for policy in DuplicatePolicy::ALL {
            assert_eq!(DuplicatePolicy::parse(policy.as_str()).unwrap(), policy);
        }
        for kind in [
            DuplicateKind::WithinSameContext,
            DuplicateKind::AcrossProtectedContexts,
            DuplicateKind::GenericToCanonical,
            DuplicateKind::BackupReplica,
            DuplicateKind::UnknownContext,
            DuplicateKind::ActiveToExcluded,
        ] {
            assert_eq!(DuplicateKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert!(DuplicatePolicy::parse("DELETE_EVERYTHING").is_err());
        assert!(DuplicateKind::parse("WHATEVER").is_err());
    }

    #[test]
    fn the_default_policy_is_report_only_and_never_consolidates() {
        assert_eq!(DuplicatePolicy::default(), DuplicatePolicy::ReportOnly);
        assert!(!DuplicatePolicy::ReportOnly.may_consolidate());
        assert!(!DuplicatePolicy::PreserveAll.may_consolidate());
        assert!(DuplicatePolicy::ConsolidateAll.may_consolidate());
    }

    /// Rule 9: a protected boundary survives every policy, including the most
    /// aggressive one. This is the single most important test in the module.
    #[test]
    fn a_protected_copy_is_never_consolidated_by_any_policy() {
        let protected = Placement {
            in_protected_context: true,
            ..plain()
        };
        for policy in DuplicatePolicy::ALL {
            for kind in [
                DuplicateKind::WithinSameContext,
                DuplicateKind::GenericToCanonical,
                DuplicateKind::BackupReplica,
                DuplicateKind::AcrossProtectedContexts,
            ] {
                let disposition = decide(policy, kind, protected);
                assert_ne!(
                    disposition,
                    DuplicateDisposition::SkipRepresented,
                    "policy {} + kind {} dropped a protected copy",
                    policy.as_str(),
                    kind.as_str()
                );
            }
        }
    }

    /// Copies spread across distinct protected contexts are preserved even if
    /// none of them is individually flagged.
    #[test]
    fn across_protected_contexts_is_preserved_even_when_not_individually_marked() {
        let d = decide(
            DuplicatePolicy::ConsolidateAll,
            DuplicateKind::AcrossProtectedContexts,
            plain(),
        );
        assert_eq!(d, DuplicateDisposition::PreserveAcrossContext);
    }

    #[test]
    fn the_representative_is_always_copied() {
        let representative = Placement {
            is_representative: true,
            ..plain()
        };
        for policy in DuplicatePolicy::ALL {
            for kind in [
                DuplicateKind::WithinSameContext,
                DuplicateKind::GenericToCanonical,
                DuplicateKind::BackupReplica,
                DuplicateKind::UnknownContext,
            ] {
                assert_eq!(
                    decide(policy, kind, representative),
                    DuplicateDisposition::Copy,
                    "policy {} + kind {} dropped the representative",
                    policy.as_str(),
                    kind.as_str()
                );
            }
        }
    }

    #[test]
    fn report_only_and_preserve_all_copy_everything() {
        for policy in [DuplicatePolicy::ReportOnly, DuplicatePolicy::PreserveAll] {
            for kind in [
                DuplicateKind::WithinSameContext,
                DuplicateKind::GenericToCanonical,
                DuplicateKind::BackupReplica,
            ] {
                assert_eq!(decide(policy, kind, plain()), DuplicateDisposition::Copy);
            }
        }
    }

    #[test]
    fn consolidate_within_context_only_touches_same_context_sets() {
        let p = DuplicatePolicy::ConsolidateWithinContext;
        assert_eq!(
            decide(p, DuplicateKind::WithinSameContext, plain()),
            DuplicateDisposition::SkipRepresented
        );
        // A set spanning different contexts is left alone by this policy.
        assert_eq!(
            decide(p, DuplicateKind::GenericToCanonical, plain()),
            DuplicateDisposition::Copy
        );
    }

    #[test]
    fn consolidate_generic_copies_only_drops_the_copies_inside_generic_folders() {
        let p = DuplicatePolicy::ConsolidateGenericCopies;
        let in_generic = Placement {
            in_generic_context: true,
            ..plain()
        };
        assert_eq!(
            decide(p, DuplicateKind::GenericToCanonical, in_generic),
            DuplicateDisposition::SkipRepresented
        );
        assert_eq!(
            decide(p, DuplicateKind::BackupReplica, in_generic),
            DuplicateDisposition::SkipRepresented
        );
        // The same set, but this copy is NOT in a generic folder: it stays.
        assert_eq!(
            decide(p, DuplicateKind::GenericToCanonical, plain()),
            DuplicateDisposition::Copy
        );
    }

    /// §15.2: never infer redundancy. If we cannot classify the contexts, the
    /// copy is kept even under the most aggressive policy.
    #[test]
    fn unknown_context_is_never_consolidated() {
        for policy in DuplicatePolicy::ALL {
            assert_eq!(
                decide(policy, DuplicateKind::UnknownContext, plain()),
                DuplicateDisposition::Copy,
                "policy {} consolidated an unclassified duplicate",
                policy.as_str()
            );
        }
    }

    #[test]
    fn active_to_excluded_is_representable_but_conservative_today() {
        assert_eq!(
            decide(
                DuplicatePolicy::ConsolidateAll,
                DuplicateKind::ActiveToExcluded,
                plain()
            ),
            DuplicateDisposition::Copy
        );
    }

    #[test]
    fn consolidate_all_drops_plain_non_representative_copies() {
        assert_eq!(
            decide(
                DuplicatePolicy::ConsolidateAll,
                DuplicateKind::WithinSameContext,
                plain()
            ),
            DuplicateDisposition::SkipRepresented
        );
    }
}
