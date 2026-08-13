//! `df-agent` — the loop that drives a run to completion without stopping.
//!
//! ADR-0044. The agent proposes and executes; the authority is `df-rules`, and
//! the gate seals the verdict. This crate is the part that decides **whether to
//! carry on**, which over a two-day run on an old disk is the property that
//! decides whether the work finishes at all.
//!
//! # The loop never blocks. That is the invariant.
//!
//! [`assess`] cannot return "stop and wait". Its worst answer is
//! [`Continuation::DegradeToReview`], which means *route what is left to
//! `revisar/` and move to the next stage*. Doubt, ambiguity, a spent budget and
//! a tripped breaker all resolve the same way, because a run that pauses for a
//! human on a slow disk is a run that is still unfinished two days later —
//! which is the failure RFC-0002 was written to remove.
//!
//! `revisar/` is not an error path. It is the deterministic reserve action, and
//! the reason a bounded action vocabulary is safe: "I do not know" always has
//! somewhere to go.
//!
//! # Why the stage order is a type
//!
//! Thinking happens before copying. Every expensive decision — classification,
//! rules, duplicate resolution — runs before a byte moves and produces the
//! complete plan, which is then frozen; the slow copy executes a fixed contract
//! and decides nothing. On a slow disk that ordering is what makes a run
//! resumable and sampleable *before* it starts, so [`Stage::ORDER`] is
//! enforced rather than documented.
//!
//! # No I/O
//!
//! Like `df-rules`, this crate reads nothing and writes nothing. It is fed
//! counters and returns decisions, which is what lets the non-blocking
//! guarantee be proved by a test instead of argued for in a review.

use serde::{Deserialize, Serialize};

/// Schema identifier of the agent run contract.
pub const AGENT_RUN_SCHEMA_VERSION: &str = "dataforge.agent-run/0.1.0";

/// The stages of a run, in the only order they may happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Stage {
    /// Capture what the user wants, in natural language. The one point a human
    /// may be asked anything, and even that is optional.
    Intent,
    /// `scan` + `hash`. Reads the origin; never writes to it.
    Inventory,
    /// Group by SHA-256 and choose representatives. Resolves the bulk with no
    /// AI at all.
    Deduplicate,
    /// Classify grafted subtrees by state and route them.
    Grafts,
    /// Recover a canonical tree, or invent a taxonomy for what has none.
    Diagnose,
    /// Assemble the complete plan: every file to a destination or to
    /// `revisar/<path>` with its reason.
    Plan,
    /// Freeze the manifest. From here the contract is fixed.
    Freeze,
    /// Copy. Decides nothing.
    Execute,
    /// Re-read and re-hash, trusting neither agent nor executor.
    Verify,
    /// The organisation scheme and the source→destination map.
    Report,
}

impl Stage {
    /// Every stage, in the mandatory order.
    pub const ORDER: [Stage; 10] = [
        Stage::Intent,
        Stage::Inventory,
        Stage::Deduplicate,
        Stage::Grafts,
        Stage::Diagnose,
        Stage::Plan,
        Stage::Freeze,
        Stage::Execute,
        Stage::Verify,
        Stage::Report,
    ];

    /// Stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "INTENT",
            Self::Inventory => "INVENTORY",
            Self::Deduplicate => "DEDUPLICATE",
            Self::Grafts => "GRAFTS",
            Self::Diagnose => "DIAGNOSE",
            Self::Plan => "PLAN",
            Self::Freeze => "FREEZE",
            Self::Execute => "EXECUTE",
            Self::Verify => "VERIFY",
            Self::Report => "REPORT",
        }
    }

    /// The stage that follows, or `None` at the end of a run.
    pub fn next(self) -> Option<Stage> {
        let index = Self::ORDER.iter().position(|stage| *stage == self)?;
        Self::ORDER.get(index + 1).copied()
    }

    /// Whether reaching this stage means bytes may move.
    ///
    /// Everything before [`Stage::Freeze`] is reversible by discarding the
    /// project; from `Execute` onwards a destination tree exists. A dry run
    /// stops exactly here.
    pub fn writes_to_destination(self) -> bool {
        self >= Stage::Execute
    }
}

/// What a run is allowed to spend before it degrades.
///
/// Every field is a ceiling. `0` is not "unlimited" anywhere in this crate —
/// see [`AgentBudget::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBudget {
    /// Plan operations the run may carry out.
    pub max_operations: u64,
    /// Wall-clock seconds.
    pub max_wall_seconds: u64,
    /// The circuit breaker: the share of decided items that may be ambiguous
    /// before the agent stops deciding and sends the remainder to `revisar/`
    /// in bulk.
    ///
    /// An agent that is guessing on most of what it sees is not doing the job,
    /// and the cheapest moment to notice is early — while the human still has
    /// a queue they can act on instead of an output tree they have to audit.
    pub max_ambiguity_rate: f64,
    /// How many decisions must exist before the rate means anything.
    ///
    /// Without this, the first ambiguous item in a run is a 100% rate and the
    /// breaker trips on evidence of one.
    pub ambiguity_sample_floor: u64,
}

impl Default for AgentBudget {
    fn default() -> Self {
        Self {
            max_operations: 5_000_000,
            max_wall_seconds: 60 * 60 * 48,
            max_ambiguity_rate: 0.35,
            ambiguity_sample_floor: 100,
        }
    }
}

impl AgentBudget {
    /// Reject a budget that cannot bound anything.
    pub fn validate(&self) -> df_error::DfResult<()> {
        if self.max_operations == 0 || self.max_wall_seconds == 0 {
            return Err(df_error::DfError::Validation(
                "an agent budget of zero authorises nothing; omit the run instead".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.max_ambiguity_rate) {
            return Err(df_error::DfError::Validation(format!(
                "max_ambiguity_rate must be within 0.0..=1.0, got {}",
                self.max_ambiguity_rate
            )));
        }
        Ok(())
    }
}

/// What the run has done so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTally {
    pub operations: u64,
    pub elapsed_seconds: u64,
    /// Items the agent decided on.
    pub decided: u64,
    /// Of those, the ones it could not resolve and sent to `revisar/`.
    pub ambiguous: u64,
}

impl RunTally {
    /// Share of decisions that were ambiguous, or `None` below the sample
    /// floor, where the number would not mean anything yet.
    pub fn ambiguity_rate(&self, sample_floor: u64) -> Option<f64> {
        if self.decided < sample_floor || self.decided == 0 {
            return None;
        }
        Some(self.ambiguous as f64 / self.decided as f64)
    }
}

/// Why the agent stopped deciding for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BreakerTrip {
    /// Too large a share of decisions were guesses.
    AmbiguityRate,
    /// The operation ceiling was reached.
    OperationBudget,
    /// The wall-clock ceiling was reached.
    TimeBudget,
}

impl BreakerTrip {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AmbiguityRate => "AMBIGUITY_RATE",
            Self::OperationBudget => "OPERATION_BUDGET",
            Self::TimeBudget => "TIME_BUDGET",
        }
    }
}

/// What the run does next.
///
/// There is deliberately no variant that waits for a human. See the crate
/// docs, and the `the_loop_can_never_block` test that keeps it that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "continuation")]
pub enum Continuation {
    /// Carry on deciding.
    Continue,
    /// Stop deciding; route everything still undecided to `revisar/` and
    /// continue the run to its end. The output is still produced, still
    /// verified and still reported — it just has a fuller review queue.
    DegradeToReview { trip: BreakerTrip },
}

impl Continuation {
    /// Whether the agent may still decide for itself.
    pub fn may_decide(&self) -> bool {
        matches!(self, Continuation::Continue)
    }
}

/// Decide whether the agent carries on deciding.
///
/// Hard ceilings first, then the breaker. The order matters only for which
/// trip gets reported; any of them degrades.
pub fn assess(budget: &AgentBudget, tally: &RunTally) -> Continuation {
    if tally.operations >= budget.max_operations {
        return Continuation::DegradeToReview {
            trip: BreakerTrip::OperationBudget,
        };
    }
    if tally.elapsed_seconds >= budget.max_wall_seconds {
        return Continuation::DegradeToReview {
            trip: BreakerTrip::TimeBudget,
        };
    }
    if let Some(rate) = tally.ambiguity_rate(budget.ambiguity_sample_floor) {
        if rate > budget.max_ambiguity_rate {
            return Continuation::DegradeToReview {
                trip: BreakerTrip::AmbiguityRate,
            };
        }
    }
    Continuation::Continue
}

/// Whether a run copies anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunMode {
    /// Produce the plan and the verdicts that would fire, and stop before
    /// anything is copied.
    DryRun,
    /// Run to completion.
    Execute,
}

impl RunMode {
    /// The last stage this mode reaches.
    pub fn final_stage(self) -> Stage {
        match self {
            // Freeze is included: a dry run that did not freeze would report a
            // plan that could still change, which is not the thing being
            // previewed.
            Self::DryRun => Stage::Freeze,
            Self::Execute => Stage::Report,
        }
    }

    /// Whether this mode may enter `stage`.
    pub fn reaches(self, stage: Stage) -> bool {
        stage <= self.final_stage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loop_can_never_block() {
        // The invariant the whole design rests on. Whatever the counters say,
        // `assess` either continues or degrades — it never asks anyone to wait.
        // If a `Continuation` variant is ever added that waits for a human,
        // this test is where that decision has to be argued.
        let budget = AgentBudget::default();
        let extremes = [
            RunTally::default(),
            RunTally {
                operations: u64::MAX,
                elapsed_seconds: u64::MAX,
                decided: u64::MAX,
                ambiguous: u64::MAX,
            },
            RunTally {
                operations: 0,
                elapsed_seconds: 0,
                decided: 1_000,
                ambiguous: 1_000,
            },
        ];
        for tally in extremes {
            let continuation = assess(&budget, &tally);
            assert!(
                matches!(
                    continuation,
                    Continuation::Continue | Continuation::DegradeToReview { .. }
                ),
                "no input may produce a blocking continuation: {continuation:?}"
            );
        }
    }

    #[test]
    fn a_healthy_run_carries_on() {
        let tally = RunTally {
            operations: 10,
            elapsed_seconds: 30,
            decided: 1_000,
            ambiguous: 50,
        };
        assert_eq!(
            assess(&AgentBudget::default(), &tally),
            Continuation::Continue
        );
    }

    #[test]
    fn each_ceiling_degrades_on_its_own() {
        let budget = AgentBudget {
            max_operations: 100,
            max_wall_seconds: 100,
            max_ambiguity_rate: 0.5,
            ambiguity_sample_floor: 10,
        };
        let cases = [
            (
                RunTally {
                    operations: 100,
                    ..RunTally::default()
                },
                BreakerTrip::OperationBudget,
            ),
            (
                RunTally {
                    elapsed_seconds: 100,
                    ..RunTally::default()
                },
                BreakerTrip::TimeBudget,
            ),
            (
                RunTally {
                    decided: 100,
                    ambiguous: 90,
                    ..RunTally::default()
                },
                BreakerTrip::AmbiguityRate,
            ),
        ];
        for (tally, expected) in cases {
            assert_eq!(
                assess(&budget, &tally),
                Continuation::DegradeToReview { trip: expected },
                "{expected:?} should have tripped"
            );
        }
    }

    #[test]
    fn the_breaker_needs_a_sample_before_it_means_anything() {
        // Without a floor the first ambiguous item is a 100% rate, and the
        // breaker trips on evidence of one.
        let budget = AgentBudget {
            ambiguity_sample_floor: 100,
            max_ambiguity_rate: 0.1,
            ..AgentBudget::default()
        };
        let one_bad_start = RunTally {
            decided: 1,
            ambiguous: 1,
            ..RunTally::default()
        };
        assert_eq!(one_bad_start.ambiguity_rate(100), None);
        assert_eq!(assess(&budget, &one_bad_start), Continuation::Continue);

        let settled = RunTally {
            decided: 100,
            ambiguous: 100,
            ..RunTally::default()
        };
        assert!(matches!(
            assess(&budget, &settled),
            Continuation::DegradeToReview {
                trip: BreakerTrip::AmbiguityRate
            }
        ));
    }

    #[test]
    fn a_rate_exactly_at_the_threshold_still_decides() {
        // Strictly greater, not greater-or-equal: a threshold of 0.35 means
        // "up to 35% is acceptable", which is how anyone setting it reads it.
        let budget = AgentBudget {
            max_ambiguity_rate: 0.5,
            ambiguity_sample_floor: 10,
            ..AgentBudget::default()
        };
        let tally = RunTally {
            decided: 100,
            ambiguous: 50,
            ..RunTally::default()
        };
        assert_eq!(assess(&budget, &tally), Continuation::Continue);
    }

    #[test]
    fn a_zero_budget_is_rejected_rather_than_treated_as_unlimited() {
        for budget in [
            AgentBudget {
                max_operations: 0,
                ..AgentBudget::default()
            },
            AgentBudget {
                max_wall_seconds: 0,
                ..AgentBudget::default()
            },
        ] {
            assert!(budget.validate().is_err());
        }
        assert!(AgentBudget::default().validate().is_ok());
    }

    #[test]
    fn an_impossible_ambiguity_rate_is_rejected() {
        for rate in [-0.1, 1.5] {
            let budget = AgentBudget {
                max_ambiguity_rate: rate,
                ..AgentBudget::default()
            };
            assert!(budget.validate().is_err(), "{rate} must be rejected");
        }
    }

    #[test]
    fn thinking_happens_before_any_byte_moves() {
        // The ordering that makes a run resumable and sampleable before it
        // starts: everything expensive to decide is behind us by the time the
        // destination is touched.
        for stage in Stage::ORDER {
            if stage.writes_to_destination() {
                assert!(
                    stage >= Stage::Execute,
                    "{stage:?} must not write before Execute"
                );
            }
        }
        assert!(!Stage::Plan.writes_to_destination());
        assert!(!Stage::Freeze.writes_to_destination());
        assert!(Stage::Execute.writes_to_destination());
    }

    #[test]
    fn the_stage_order_is_a_chain_with_one_end() {
        let mut visited = Vec::new();
        let mut stage = Some(Stage::Intent);
        while let Some(current) = stage {
            visited.push(current);
            stage = current.next();
        }
        assert_eq!(visited, Stage::ORDER.to_vec());
        assert_eq!(Stage::Report.next(), None);
    }

    #[test]
    fn a_dry_run_stops_before_it_copies() {
        assert!(!RunMode::DryRun.reaches(Stage::Execute));
        assert!(
            RunMode::DryRun.reaches(Stage::Freeze),
            "a dry run that did not freeze would preview a plan that could still change"
        );
        assert!(RunMode::Execute.reaches(Stage::Report));
        for stage in Stage::ORDER {
            if stage.writes_to_destination() {
                assert!(
                    !RunMode::DryRun.reaches(stage),
                    "a dry run reached {stage:?}, which writes"
                );
            }
        }
    }

    #[test]
    fn stage_names_are_unique() {
        let mut names: Vec<_> = Stage::ORDER.iter().map(|stage| stage.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Stage::ORDER.len());
    }
}
