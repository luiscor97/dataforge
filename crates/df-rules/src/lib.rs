//! `df-rules` — the deterministic authority the autonomous gate delegates to.
//!
//! RFC-0002's first governing principle is that **the AI is never the
//! authority**. The model proposes; a declarative rule verifies against
//! evidence recalculated locally; the engine acts on the rule's verdict. This
//! crate is that rule engine (ADR-0041).
//!
//! # Two kinds of rule, and why they are not stored the same way
//!
//! **Hard boundaries are code.** They are invariants, not preferences: never
//! deduplicate by name, never dissolve a protected boundary, never write into
//! a destination that is not empty, never auto-collapse inside a protected
//! domain. Each is *fail-closed* — the answer when evidence is missing is
//! [`Verdict::Review`] or [`Verdict::Deny`], never [`Verdict::Authorize`].
//! They live in Rust because a boundary that a caller could edit is not a
//! boundary, and [`HARD_BOUNDARY_COUNT`] plus the tests below exist so moving
//! one into the tunable half breaks the build rather than the archive.
//!
//! **Weights are data.** Which copy of a duplicate set wins, how much a
//! generic container penalises a location, how much margin an auto-approval
//! needs — these are corpus-dependent and a user should be able to tune them
//! without recompiling. They live in [`RuleParams`], versioned and checksummed
//! with the same discipline as the migrations: editing means a new version,
//! never an edit in place, and the digest is verified before use (threat A4 in
//! RFC-0002 — silent drift of the rule set).
//!
//! # What this crate deliberately does not do
//!
//! It performs no I/O and touches neither the database nor the filesystem.
//! [`evaluate`] takes evidence already read and returns a verdict, which is
//! what lets it be tested with tables instead of fixtures, and what stops the
//! gate depending on the order things were loaded in.
//!
//! It also does not decide *whether* a proposal reaches it. Today a
//! recommendation still overrides the duplicate disposition in the planner, so
//! a proposal about an embedded tree never gets this far (see ADR-0045 and its
//! `a_recommendation_still_overrides_the_proof` test). That precedence is a
//! separate decision, and until it is made this crate's verdicts are correct
//! and unreachable for exactly the cases that motivated it.

use df_domain::DuplicateKind;
use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};
use sha2::Digest;

/// Identifier and version of the rule-set contract.
///
/// Bumped when the shape of [`RuleParams`] changes. A stored rule set carries
/// its own digest as well, so a caller can tell "written by an older engine"
/// from "modified since it was written".
pub const RULE_SET_SCHEMA_VERSION: &str = "dataforge.rule-set/0.1.0";

/// How many hard boundaries exist.
///
/// Asserted by a test against [`HardBoundary::ALL`]. Adding a boundary is
/// fine; the count is here so that *removing* one, or quietly turning one into
/// a tunable parameter, cannot happen without someone changing this number and
/// being asked why.
pub const HARD_BOUNDARY_COUNT: usize = 4;

/// What the gate is allowed to do with a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    /// Proceed without a human. Only ever returned when every hard boundary
    /// passed *and* the evidence cleared the configured threshold.
    Authorize,
    /// Send to `revisar/` and carry on. The run never blocks on this.
    Review,
    /// Refuse. Distinct from [`Verdict::Review`] because a denial is not a
    /// question for a human either — it is a proposal that contradicts an
    /// invariant, and offering it for review would invite someone to approve
    /// what the engine guarantees cannot happen.
    Deny,
}

/// An invariant that no parameter, policy or model may override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HardBoundary {
    /// Same file name, different content. They are different documents, and
    /// only the hash decides identity. Consolidating them loses one of them.
    NameOnlyIdentity,
    /// The same content lives in two or more distinct protected contexts.
    /// Consolidating would dissolve a boundary (rule 9); it may also be
    /// legitimate reuse, which is precisely why a machine does not decide it.
    SharedAcrossProtectedContexts,
    /// The destination was not proven empty. The engine never overwrites, so
    /// an unproven destination is an unproven guarantee.
    DestinationNotProvenEmpty,
    /// The proposal touches a protected domain boundary. Review by default;
    /// never auto-collapse.
    ProtectedDomain,
}

impl HardBoundary {
    /// Every boundary, in the order ADR-0041 §4 lists them.
    pub const ALL: [HardBoundary; HARD_BOUNDARY_COUNT] = [
        HardBoundary::NameOnlyIdentity,
        HardBoundary::SharedAcrossProtectedContexts,
        HardBoundary::DestinationNotProvenEmpty,
        HardBoundary::ProtectedDomain,
    ];

    /// Stable wire name, recorded in the ledger with the verdict.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NameOnlyIdentity => "NAME_ONLY_IDENTITY",
            Self::SharedAcrossProtectedContexts => "SHARED_ACROSS_PROTECTED_CONTEXTS",
            Self::DestinationNotProvenEmpty => "DESTINATION_NOT_PROVEN_EMPTY",
            Self::ProtectedDomain => "PROTECTED_DOMAIN",
        }
    }

    /// What this boundary forces when it fires.
    ///
    /// Only [`HardBoundary::NameOnlyIdentity`] denies. The other three
    /// describe situations a human can legitimately resolve — reuse across
    /// matters happens, a destination can be emptied, a protected domain can
    /// be reviewed file by file — so they route to `revisar/` and the run
    /// carries on. Consolidating two different documents is not a judgement
    /// call; it is a contradiction.
    pub fn verdict(self) -> Verdict {
        match self {
            Self::NameOnlyIdentity => Verdict::Deny,
            Self::SharedAcrossProtectedContexts
            | Self::DestinationNotProvenEmpty
            | Self::ProtectedDomain => Verdict::Review,
        }
    }
}

/// The evidence a consolidation proposal is judged on.
///
/// Every field is something the engine has already established. Nothing here
/// is a model's opinion, and nothing is optional-with-a-default: a missing
/// fact has to be expressed, so it can fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidationProposal {
    /// Content hashes of the occurrences proposed for consolidation. More than
    /// one distinct value means these are not the same document.
    pub content_ids: Vec<String>,
    /// How the engine classified the duplicate set.
    pub kind: DuplicateKind,
    /// How many *distinct* protected boundaries the occurrences sit inside.
    pub protected_boundaries: usize,
    /// Whether the destination root was proven empty before the run.
    pub destination_proven_empty: bool,
}

/// The verdict and, when one fired, the boundary that decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleOutcome {
    pub verdict: Verdict,
    /// The boundary that determined the verdict, if a hard one did.
    pub boundary: Option<HardBoundary>,
    /// Contract version of the rule set that produced this.
    pub rule_set_schema: String,
    /// Digest of the parameters in force. "Why was this authorised?" has a
    /// deterministic answer only if the parameters can be identified.
    pub rule_set_digest: String,
    /// Plain-language justification, for the report and the ledger.
    pub reason: String,
}

/// Weights that a user may tune per corpus or domain.
///
/// Not invariants. Every field here changes *which* copy wins or *how much*
/// margin an automatic decision needs; none of them can authorise something a
/// hard boundary refuses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleParams {
    pub representative: RepresentativeWeights,
    /// Minimum recomputed confidence for [`Verdict::Authorize`]. Confidence
    /// reported by a model is never used here — only confidence the engine
    /// recalculated.
    pub auto_approve_confidence: f64,
}

/// The weights of the representative score, from the formula the original
/// recovery actually used.
///
/// `score = depth·depth_weight + path_length·path_length_weight
///          + oldest_mtime·oldest_mtime_weight`
///
/// The sign of the third term is the one worth understanding. Among
/// **byte-identical** duplicates the original keeps its old timestamp and the
/// copies get a fresh one when they are copied, so **the oldest is the
/// original**. That reasoning does not carry to *variants* with different
/// hashes, where the newest is the live version — which is why this weight
/// only ever applies to exact duplicate sets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentativeWeights {
    /// Negative: deeper is worse.
    pub depth_weight: f64,
    /// Negative: a longer path is worse.
    pub path_length_weight: f64,
    /// Positive: older is better, because older means original.
    pub oldest_mtime_weight: f64,
}

impl Default for RepresentativeWeights {
    /// The weights measured on the real archive (ADR-0041 §2).
    fn default() -> Self {
        Self {
            depth_weight: -8.0,
            path_length_weight: -1.1,
            oldest_mtime_weight: 15.0,
        }
    }
}

impl Default for RuleParams {
    fn default() -> Self {
        Self {
            representative: RepresentativeWeights::default(),
            auto_approve_confidence: 0.95,
        }
    }
}

impl RuleParams {
    /// Reject parameters that would make a verdict meaningless.
    ///
    /// Fail-closed: the signs are part of the formula's meaning, not a
    /// preference. A positive `depth_weight` would prefer the *deepest* copy —
    /// the one most likely to be a stray replica — and nothing downstream
    /// would notice, because the score is only ever compared against itself.
    pub fn validate(&self) -> DfResult<()> {
        if !self.representative.depth_weight.is_finite()
            || !self.representative.path_length_weight.is_finite()
            || !self.representative.oldest_mtime_weight.is_finite()
        {
            return Err(DfError::Validation(
                "representative weights must be finite".to_string(),
            ));
        }
        if self.representative.depth_weight > 0.0 {
            return Err(DfError::Validation(
                "depth_weight must not be positive: a deeper copy is never the better \
                 representative"
                    .to_string(),
            ));
        }
        if self.representative.path_length_weight > 0.0 {
            return Err(DfError::Validation(
                "path_length_weight must not be positive: a longer path is never the better \
                 representative"
                    .to_string(),
            ));
        }
        if self.representative.oldest_mtime_weight < 0.0 {
            return Err(DfError::Validation(
                "oldest_mtime_weight must not be negative: among byte-identical duplicates the \
                 oldest copy is the original"
                    .to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.auto_approve_confidence) {
            return Err(DfError::Validation(format!(
                "auto_approve_confidence must be within 0.0..=1.0, got {}",
                self.auto_approve_confidence
            )));
        }
        Ok(())
    }

    /// SHA-256 over the canonical JSON of these parameters.
    ///
    /// Canonical because a digest that changed with key order would flag
    /// tampering on every round trip, and a check that cries wolf gets turned
    /// off.
    pub fn digest(&self) -> DfResult<String> {
        let value = serde_json::to_value(self)
            .map_err(|error| DfError::Serialization(format!("rule params: {error}")))?;
        Ok(hex::encode(sha2::Sha256::digest(
            df_ledger::canonical_json(&value).as_bytes(),
        )))
    }

    /// Score one candidate location. Higher wins.
    pub fn representative_score(&self, depth: u32, path_length: usize, is_oldest: bool) -> f64 {
        f64::from(depth) * self.representative.depth_weight
            + path_length as f64 * self.representative.path_length_weight
            + if is_oldest {
                self.representative.oldest_mtime_weight
            } else {
                0.0
            }
    }
}

/// A named, versioned, checksummed set of parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    pub id: String,
    pub version: u32,
    pub params: RuleParams,
    /// Digest recorded when this set was written.
    pub digest: String,
}

impl RuleSet {
    /// Build a set, computing its digest.
    pub fn new(id: impl Into<String>, version: u32, params: RuleParams) -> DfResult<Self> {
        params.validate()?;
        let digest = params.digest()?;
        Ok(Self {
            id: id.into(),
            version,
            params,
            digest,
        })
    }

    /// Recompute the digest and refuse the set if it moved.
    ///
    /// This is the check that makes "versioned and checksummed" mean
    /// something: without it, editing the stored parameters in place would
    /// silently change every future verdict while the recorded provenance
    /// still pointed at the old set.
    pub fn verify(&self) -> DfResult<()> {
        self.params.validate()?;
        let recomputed = self.params.digest()?;
        if recomputed != self.digest {
            return Err(DfError::Validation(format!(
                "rule set `{}` v{} has drifted: recorded digest {} but parameters hash to {}",
                self.id, self.version, self.digest, recomputed
            )));
        }
        Ok(())
    }
}

/// The first hard boundary a proposal violates, if any.
///
/// Order is the order of [`HardBoundary::ALL`], and it matters only for which
/// reason gets reported: any single violation is enough to keep a proposal
/// away from [`Verdict::Authorize`].
pub fn first_violated_boundary(proposal: &ConsolidationProposal) -> Option<HardBoundary> {
    // Never deduplicate by name: identity is the hash and nothing else. An
    // empty proposal counts as violating this — consolidating nothing is not
    // something to authorise.
    let mut distinct = proposal.content_ids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() != 1 {
        return Some(HardBoundary::NameOnlyIdentity);
    }

    // Shared evidence across matters: reuse or contamination, never a
    // machine's call.
    if proposal.kind == DuplicateKind::AcrossProtectedContexts {
        return Some(HardBoundary::SharedAcrossProtectedContexts);
    }

    if !proposal.destination_proven_empty {
        return Some(HardBoundary::DestinationNotProvenEmpty);
    }

    if proposal.protected_boundaries > 0 {
        return Some(HardBoundary::ProtectedDomain);
    }

    None
}

/// Judge a consolidation proposal.
///
/// Hard boundaries are checked first and cannot be overridden by any
/// parameter. Only if all four pass does the classification decide, and only
/// a kind the engine can *prove* reaches [`Verdict::Authorize`] — anything
/// unproven goes to `revisar/`, which is the fallback the whole design rests
/// on.
pub fn evaluate(proposal: &ConsolidationProposal, rule_set: &RuleSet) -> DfResult<RuleOutcome> {
    // Verified on every call, not once at load: a set that drifted between
    // being read and being used is exactly the case a load-time check misses.
    rule_set.verify()?;

    let outcome = |verdict, boundary, reason: String| RuleOutcome {
        verdict,
        boundary,
        rule_set_schema: RULE_SET_SCHEMA_VERSION.to_string(),
        rule_set_digest: rule_set.digest.clone(),
        reason,
    };

    if let Some(boundary) = first_violated_boundary(proposal) {
        let reason = match boundary {
            HardBoundary::NameOnlyIdentity => "the occurrences do not share one content hash; \
                 identity is the hash and nothing else"
                .to_string(),
            HardBoundary::SharedAcrossProtectedContexts => {
                "the same content lives in distinct protected contexts; that is either legitimate \
                 reuse or matter contamination, and a machine does not decide which"
                    .to_string()
            }
            HardBoundary::DestinationNotProvenEmpty => {
                "the destination was not proven empty".to_string()
            }
            HardBoundary::ProtectedDomain => format!(
                "the proposal touches {} protected boundary/ies; a protected domain is never \
                 auto-collapsed",
                proposal.protected_boundaries
            ),
        };
        return Ok(outcome(boundary.verdict(), Some(boundary), reason));
    }

    // No boundary fired. Now — and only now — the classification decides.
    let (verdict, reason) = match proposal.kind {
        DuplicateKind::WithinSameContext => (
            Verdict::Authorize,
            "every copy provably shares one context".to_string(),
        ),
        DuplicateKind::ContainedTreeReplica => (
            Verdict::Authorize,
            "the copy sits inside a subtree proved to hold nothing of its own, and the \
             representative lives outside it"
                .to_string(),
        ),
        DuplicateKind::GenericToCanonical | DuplicateKind::BackupReplica => (
            Verdict::Review,
            "a generic or backup container is evidence about location, not proof of redundancy"
                .to_string(),
        ),
        // Fail-closed, and deliberately not a catch-all that authorises:
        // a kind this engine cannot reason about must not slip through as
        // approved because nobody wrote its arm.
        other => (
            Verdict::Review,
            format!(
                "`{}` is not a classification this rule set can authorise",
                other.as_str()
            ),
        ),
    };
    Ok(outcome(verdict, None, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(kind: DuplicateKind) -> ConsolidationProposal {
        ConsolidationProposal {
            content_ids: vec!["sha-1".to_string(), "sha-1".to_string()],
            kind,
            protected_boundaries: 0,
            destination_proven_empty: true,
        }
    }

    fn rule_set() -> RuleSet {
        RuleSet::new("default", 1, RuleParams::default()).expect("default params are valid")
    }

    // ---- the four hard boundaries -------------------------------------

    #[test]
    fn same_name_different_content_is_denied() {
        let mut candidate = proposal(DuplicateKind::WithinSameContext);
        candidate.content_ids = vec!["sha-1".to_string(), "sha-2".to_string()];
        let outcome = evaluate(&candidate, &rule_set()).unwrap();
        assert_eq!(outcome.verdict, Verdict::Deny);
        assert_eq!(outcome.boundary, Some(HardBoundary::NameOnlyIdentity));
    }

    #[test]
    fn evidence_shared_across_protected_contexts_goes_to_review() {
        let outcome = evaluate(
            &proposal(DuplicateKind::AcrossProtectedContexts),
            &rule_set(),
        )
        .unwrap();
        assert_eq!(outcome.verdict, Verdict::Review);
        assert_eq!(
            outcome.boundary,
            Some(HardBoundary::SharedAcrossProtectedContexts)
        );
    }

    #[test]
    fn an_unproven_destination_is_never_authorised() {
        let mut candidate = proposal(DuplicateKind::WithinSameContext);
        candidate.destination_proven_empty = false;
        let outcome = evaluate(&candidate, &rule_set()).unwrap();
        assert_eq!(outcome.verdict, Verdict::Review);
        assert_eq!(
            outcome.boundary,
            Some(HardBoundary::DestinationNotProvenEmpty)
        );
    }

    #[test]
    fn a_protected_domain_is_never_auto_collapsed() {
        let mut candidate = proposal(DuplicateKind::WithinSameContext);
        candidate.protected_boundaries = 1;
        let outcome = evaluate(&candidate, &rule_set()).unwrap();
        assert_eq!(outcome.verdict, Verdict::Review);
        assert_eq!(outcome.boundary, Some(HardBoundary::ProtectedDomain));
    }

    #[test]
    fn no_parameter_can_authorise_past_a_hard_boundary() {
        // The property that makes the two halves different in kind and not
        // just in storage. Whatever the weights say, a boundary wins.
        let permissive = RuleSet::new(
            "permissive",
            1,
            RuleParams {
                auto_approve_confidence: 0.0,
                ..RuleParams::default()
            },
        )
        .unwrap();

        let mut candidate = proposal(DuplicateKind::WithinSameContext);
        candidate.content_ids = vec!["sha-1".to_string(), "sha-2".to_string()];
        assert_eq!(
            evaluate(&candidate, &permissive).unwrap().verdict,
            Verdict::Deny
        );

        candidate = proposal(DuplicateKind::WithinSameContext);
        candidate.protected_boundaries = 3;
        assert_eq!(
            evaluate(&candidate, &permissive).unwrap().verdict,
            Verdict::Review
        );
    }

    #[test]
    fn the_hard_boundaries_are_all_accounted_for() {
        assert_eq!(HardBoundary::ALL.len(), HARD_BOUNDARY_COUNT);
        let mut names: Vec<_> = HardBoundary::ALL.iter().map(|b| b.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), HARD_BOUNDARY_COUNT, "duplicate boundary name");
        // Only a contradiction denies; the rest are questions a human can
        // answer, so they route to `revisar/` and the run carries on.
        assert_eq!(
            HardBoundary::ALL
                .iter()
                .filter(|b| b.verdict() == Verdict::Deny)
                .count(),
            1
        );
    }

    // ---- classification, once the boundaries pass ----------------------

    #[test]
    fn only_a_proved_context_is_authorised() {
        for kind in [
            DuplicateKind::WithinSameContext,
            DuplicateKind::ContainedTreeReplica,
        ] {
            assert_eq!(
                evaluate(&proposal(kind), &rule_set()).unwrap().verdict,
                Verdict::Authorize,
                "{kind:?} is proved and must be authorised"
            );
        }
    }

    #[test]
    fn an_unproved_context_falls_through_to_review() {
        for kind in [
            DuplicateKind::UnknownContext,
            DuplicateKind::GenericToCanonical,
            DuplicateKind::BackupReplica,
            DuplicateKind::ActiveToExcluded,
        ] {
            let outcome = evaluate(&proposal(kind), &rule_set()).unwrap();
            assert_eq!(
                outcome.verdict,
                Verdict::Review,
                "{kind:?} is not proved and must not be authorised"
            );
            assert_eq!(
                outcome.boundary, None,
                "no hard boundary fired for {kind:?}"
            );
        }
    }

    // ---- the parameters ------------------------------------------------

    #[test]
    fn a_drifted_rule_set_is_refused() {
        let mut set = rule_set();
        set.params.auto_approve_confidence = 0.10;
        let error = evaluate(&proposal(DuplicateKind::WithinSameContext), &set)
            .expect_err("parameters edited in place must not be usable");
        assert!(error.to_string().contains("drifted"), "{error}");
    }

    #[test]
    fn the_digest_does_not_depend_on_key_order() {
        // A digest that moved on a round trip would flag tampering constantly,
        // and a check that cries wolf gets switched off.
        let params = RuleParams::default();
        let round_tripped: RuleParams =
            serde_json::from_str(&serde_json::to_string(&params).unwrap()).unwrap();
        assert_eq!(params.digest().unwrap(), round_tripped.digest().unwrap());
    }

    #[test]
    fn weights_that_invert_the_formula_are_rejected() {
        // Each of these compiles, looks plausible, and would quietly pick the
        // worst copy in the set as the survivor.
        let cases = [
            (8.0, -1.1, 15.0, "depth_weight"),
            (-8.0, 1.1, 15.0, "path_length_weight"),
            (-8.0, -1.1, -15.0, "oldest_mtime_weight"),
        ];
        for (depth, path, mtime, field) in cases {
            let params = RuleParams {
                representative: RepresentativeWeights {
                    depth_weight: depth,
                    path_length_weight: path,
                    oldest_mtime_weight: mtime,
                },
                ..RuleParams::default()
            };
            let error = params
                .validate()
                .expect_err("an inverted weight must be rejected");
            assert!(error.to_string().contains(field), "{error}");
        }
    }

    #[test]
    fn the_shallowest_shortest_oldest_copy_wins() {
        // The formula, checked on the shape it was measured from rather than
        // on its arithmetic: the original is shallow, short-pathed and old.
        let params = RuleParams::default();
        let original = params.representative_score(1, 12, true);
        let deep_replica = params.representative_score(5, 48, false);
        let same_depth_newer = params.representative_score(1, 12, false);
        assert!(original > deep_replica);
        assert!(
            original > same_depth_newer,
            "among byte-identical copies the oldest is the original"
        );
    }

    #[test]
    fn confidence_outside_the_unit_interval_is_rejected() {
        for value in [-0.1, 1.1] {
            let params = RuleParams {
                auto_approve_confidence: value,
                ..RuleParams::default()
            };
            assert!(params.validate().is_err(), "{value} must be rejected");
        }
    }
}
