//! `df-tools` — the bounded, typed surface an agent drives the engine with.
//!
//! [`df_facade`] already exposes the whole pipeline as stateless functions, and
//! until now the only client consuming it was the CLI, linking the Rust ABI
//! directly. That is enough for a person and not enough for an agent, for a
//! reason that has nothing to do with ergonomics: a CLI is reached through a
//! shell, and an agent holding a shell can do anything the shell can do. The
//! bounded action vocabulary RFC-0002 relies on then rests on the model
//! behaving, which is not a guarantee.
//!
//! This crate is where that vocabulary becomes structural. It is a thin,
//! stable adapter over the facade — every tool here delegates to a facade
//! function and adds no capability of its own — whose value is what it
//! *cannot* express. There is no arbitrary-filesystem tool, no raw SQL, no
//! shell. A caller that only holds this surface cannot step outside the
//! engine's own vocabulary, whatever it decides to do (ADR-0043 §2).
//!
//! # The three capability classes
//!
//! Every tool declares one, and the split is the point of the crate:
//!
//! - [`Capability::Observe`] — reads. Reports, status, queues. Free.
//! - [`Capability::Build`] — advances inventory, analysis and the plan.
//!   Copies nothing, touches no destination, and is reversible by discarding
//!   the project.
//! - [`Capability::Commit`] — changes real state: freezes the manifest, copies
//!   bytes, verifies output.
//!
//! Only [`Capability::Commit`] will pass through authorization when `df-rules`
//! exists (ADR-0041). Looking and thinking stay free; changing state goes
//! through a gate. Today no gate is installed, so [`invoke`] runs a commit tool
//! the same as any other — [`Capability::requires_authorization`] is the seam
//! that gate will attach to, declared now so the classification is fixed before
//! anything depends on it.
//!
//! # No hidden state
//!
//! Every tool is identified by its `project_dir`. There is no session, no
//! handle and nothing cached in memory: the whole state lives in SQLite, which
//! is what lets an agent die mid-run and resume with no coordination
//! (ADR-0043 §5).
//!
//! # Stability
//!
//! Tool names and their capability classes are a frozen contract, versioned by
//! [`TOOL_SURFACE_VERSION`] and pinned by the `frozen_contracts` test in
//! `df-facade`. Changes are additive only: renaming a tool, or moving one to a
//! weaker class, is a version bump and never an edit in place, because external
//! agents depend on this surface (ADR-0043 §4).
//!
//! **Declared debt.** `search_project_content` and `query_project_content` are
//! part of the observe class in ADR-0043 but are not exposed yet: their inputs
//! are richer than the rest and deserve their own schemas rather than a rushed
//! one. Adding them is an additive change, which is exactly what the contract
//! permits.

use std::path::PathBuf;

use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use df_facade::{Actor, DuplicatePolicy, RuleAction};

/// Version of the tool surface as a whole: the set of names, their capability
/// classes and their input shapes.
///
/// Bumped when a tool is added; a tool that changes meaning gets a new name
/// instead, so a caller pinned to a version can never be silently handed
/// different semantics.
pub const TOOL_SURFACE_VERSION: &str = "dataforge.tool-surface/0.1.0";

/// What a tool is allowed to do, and therefore what it has to pass through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Capability {
    /// Reads only. Cannot change any state, so it needs no authorization.
    Observe,
    /// Advances inventory, analysis or the plan. Copies nothing and reaches no
    /// destination; discarding the project undoes it.
    Build,
    /// Changes real state: freezes the manifest, copies bytes, or verifies
    /// what was copied.
    Commit,
}

impl Capability {
    /// Stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "OBSERVE",
            Self::Build => "BUILD",
            Self::Commit => "COMMIT",
        }
    }

    /// Whether a future [`df-rules`] gate has to authorize this class.
    ///
    /// Only [`Capability::Commit`]. This is deliberately a property of the
    /// class and not of the individual tool: a gate that had to enumerate
    /// tools would silently fail open the day a tool is added and nobody
    /// remembers to list it.
    ///
    /// [`df-rules`]: https://github.com/luiscor97/dataforge/blob/main/docs/adr/ADR-0041-df-rules-canonical-recovery.md
    pub fn requires_authorization(self) -> bool {
        matches!(self, Self::Commit)
    }
}

/// One tool: a name an agent can call, and what calling it is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Tool {
    /// Stable name. Part of the frozen contract.
    pub name: &'static str,
    /// What this tool is allowed to do.
    pub capability: Capability,
    /// One line, for an agent choosing between tools.
    pub summary: &'static str,
}

/// Every tool the surface exposes, in pipeline order.
///
/// This is the single source of truth: [`invoke`] rejects any name absent
/// here, so a tool cannot exist by being reachable but undeclared.
pub const TOOLS: &[Tool] = &[
    // ---- observe -------------------------------------------------------
    Tool {
        name: "project_status",
        capability: Capability::Observe,
        summary: "Current state of the project, its roots and its counters.",
    },
    Tool {
        name: "duplicate_report",
        capability: Capability::Observe,
        summary: "Exact-duplicate sets, their representatives and their weight.",
    },
    Tool {
        name: "tree_clone_report",
        capability: Capability::Observe,
        summary: "Folder subtrees proven identical by Merkle signature.",
    },
    Tool {
        name: "tree_relation_report",
        capability: Capability::Observe,
        summary: "Structural relations between trees, including embedded ones.",
    },
    Tool {
        name: "context_report",
        capability: Capability::Observe,
        summary: "Folder context classification under the active profile.",
    },
    Tool {
        name: "structural_anomaly_report",
        capability: Capability::Observe,
        summary: "Structural anomalies the analysis flagged.",
    },
    Tool {
        name: "structural_review_queue",
        capability: Capability::Observe,
        summary: "Items awaiting a review decision, with the reason for each.",
    },
    Tool {
        name: "structural_review_classes",
        capability: Capability::Observe,
        summary: "The review queue grouped by question, not by item.",
    },
    Tool {
        name: "plan_destination_tree",
        capability: Capability::Observe,
        summary: "Where the current plan would put the data, before approving it.",
    },
    Tool {
        name: "destination_guarantees",
        capability: Capability::Observe,
        summary: "What the destination root guarantees about identity and safety.",
    },
    Tool {
        name: "verify_audit",
        capability: Capability::Observe,
        summary: "Ledger chain verification and the audit trail it yields.",
    },
    // ---- build ---------------------------------------------------------
    Tool {
        name: "scan_project",
        capability: Capability::Build,
        summary: "Inventory the source roots. Reads the origin; never writes to it.",
    },
    Tool {
        name: "hash_project",
        capability: Capability::Build,
        summary: "Prove file identity with the double hash.",
    },
    Tool {
        name: "analyze_project",
        capability: Capability::Build,
        summary: "Derive duplicates, folder signatures and context from the inventory.",
    },
    Tool {
        name: "analyze_similarity",
        capability: Capability::Build,
        summary: "Streaming content similarity and candidate lineage.",
    },
    Tool {
        name: "analyze_media",
        capability: Capability::Build,
        summary: "Bounded media analysis in an isolated worker.",
    },
    Tool {
        name: "decide_structural_review",
        capability: Capability::Build,
        summary: "Record one review decision, with its rationale, as its own event.",
    },
    Tool {
        name: "decide_structural_review_batch",
        capability: Capability::Build,
        summary: "Record many review decisions atomically, one event each.",
    },
    Tool {
        name: "create_plan",
        capability: Capability::Build,
        summary: "Produce the full plan under a duplicate policy. Copies nothing.",
    },
    Tool {
        name: "validate_plan",
        capability: Capability::Build,
        summary: "Re-check the plan invariants against the stored plan.",
    },
    // ---- commit --------------------------------------------------------
    Tool {
        name: "approve_plan",
        capability: Capability::Commit,
        summary: "Freeze the plan into an immutable manifest. The gate.",
    },
    Tool {
        name: "execute_plan",
        capability: Capability::Commit,
        summary: "Execute the frozen manifest. Copy-only, resumable.",
    },
    Tool {
        name: "verify_project_output",
        capability: Capability::Commit,
        summary: "Re-read and re-hash the output, trusting neither agent nor executor.",
    },
];

/// Look a tool up by name.
pub fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// Every tool in one capability class.
pub fn tools_with(capability: Capability) -> impl Iterator<Item = &'static Tool> {
    TOOLS
        .iter()
        .filter(move |tool| tool.capability == capability)
}

// --------------------------------------------------------------------------
// Inputs
//
// Every input denies unknown fields. A bounded surface that quietly ignored
// what it did not recognise would let a caller believe it had asked for
// something it had not — worse than a rejection, because it fails silently.
// --------------------------------------------------------------------------

/// The input of every tool that needs nothing but a project.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectInput {
    project_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanTreeInput {
    project_dir: PathBuf,
    /// Levels shown under the output root. A view, never a filter: widening it
    /// cannot change the totals.
    #[serde(default = "default_tree_depth")]
    depth: u32,
}

fn default_tree_depth() -> u32 {
    2
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePlanInput {
    project_dir: PathBuf,
    /// Defaults to the safe policy: copy everything, report the evidence.
    #[serde(default)]
    policy: DuplicatePolicy,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideReviewInput {
    project_dir: PathBuf,
    item_id: String,
    decision: RuleAction,
    rationale: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideReviewBatchInput {
    project_dir: PathBuf,
    decisions: Vec<ReviewDecision>,
}

/// One decision inside a batch. Carries its own rationale, exactly as a single
/// decision does: a batch is transport, never a weaker record.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewDecision {
    item_id: String,
    decision: RuleAction,
    rationale: String,
}

fn parse<T: serde::de::DeserializeOwned>(name: &str, input: Value) -> DfResult<T> {
    serde_json::from_value(input)
        .map_err(|error| DfError::Validation(format!("invalid input for tool `{name}`: {error}")))
}

fn encode<T: Serialize>(name: &str, value: T) -> DfResult<Value> {
    serde_json::to_value(value).map_err(|error| {
        DfError::Serialization(format!("could not encode output of tool `{name}`: {error}"))
    })
}

/// Call a tool by name.
///
/// `input` is the tool's JSON input and the result is its JSON output, so the
/// same dispatch serves an in-process caller and the MCP server without either
/// growing its own copy of the mapping.
///
/// An unknown name is a [`DfError::Validation`], never a fallthrough: a tool
/// that is not in [`TOOLS`] does not exist.
pub fn invoke(name: &str, input: Value, actor: Actor) -> DfResult<Value> {
    // Reject before dispatching, so the registry stays the only definition of
    // what exists. Without this the match arms below would be a second one.
    if tool(name).is_none() {
        return Err(DfError::Validation(format!(
            "unknown tool `{name}`; the surface is {TOOL_SURFACE_VERSION}"
        )));
    }

    match name {
        // ---- observe ---------------------------------------------------
        "project_status" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::project_status(&input.project_dir)?)
        }
        "duplicate_report" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::duplicate_report(&input.project_dir)?)
        }
        "tree_clone_report" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::tree_clone_report(&input.project_dir)?)
        }
        "tree_relation_report" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::tree_relation_report(&input.project_dir)?)
        }
        "context_report" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::context_report(&input.project_dir)?)
        }
        "structural_anomaly_report" => {
            let input: ProjectInput = parse(name, input)?;
            encode(
                name,
                df_facade::structural_anomaly_report(&input.project_dir)?,
            )
        }
        "structural_review_queue" => {
            let input: ProjectInput = parse(name, input)?;
            encode(
                name,
                df_facade::structural_review_queue(&input.project_dir)?,
            )
        }
        "structural_review_classes" => {
            let input: ProjectInput = parse(name, input)?;
            encode(
                name,
                df_facade::structural_review_classes(&input.project_dir)?,
            )
        }
        "plan_destination_tree" => {
            let input: PlanTreeInput = parse(name, input)?;
            encode(
                name,
                df_facade::plan_destination_tree(&input.project_dir, input.depth)?,
            )
        }
        "destination_guarantees" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::destination_guarantees(&input.project_dir)?)
        }
        "verify_audit" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::verify_audit(&input.project_dir)?)
        }

        // ---- build -----------------------------------------------------
        "scan_project" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::scan_project(&input.project_dir, actor)?)
        }
        "hash_project" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::hash_project(&input.project_dir, actor)?)
        }
        "analyze_project" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::analyze_project(&input.project_dir, actor)?)
        }
        "analyze_similarity" => {
            let input: ProjectInput = parse(name, input)?;
            encode(
                name,
                df_facade::analyze_similarity(&input.project_dir, actor)?,
            )
        }
        "analyze_media" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::analyze_media(&input.project_dir, actor)?)
        }
        "decide_structural_review" => {
            let input: DecideReviewInput = parse(name, input)?;
            encode(
                name,
                df_facade::decide_structural_review(
                    &input.project_dir,
                    &input.item_id,
                    input.decision,
                    &input.rationale,
                    actor,
                )?,
            )
        }
        "decide_structural_review_batch" => {
            let input: DecideReviewBatchInput = parse(name, input)?;
            let decisions: Vec<_> = input
                .decisions
                .into_iter()
                .map(|decision| df_facade::ReviewDecisionInput {
                    item_id: decision.item_id,
                    decision: decision.decision,
                    rationale: decision.rationale,
                })
                .collect();
            encode(
                name,
                df_facade::decide_structural_review_batch(&input.project_dir, &decisions, actor)?,
            )
        }
        "create_plan" => {
            let input: CreatePlanInput = parse(name, input)?;
            encode(
                name,
                df_facade::create_plan(&input.project_dir, actor, input.policy)?,
            )
        }
        "validate_plan" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::validate_plan(&input.project_dir)?)
        }

        // ---- commit ----------------------------------------------------
        "approve_plan" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::approve_plan(&input.project_dir, actor)?)
        }
        "execute_plan" => {
            let input: ProjectInput = parse(name, input)?;
            encode(name, df_facade::execute_plan(&input.project_dir, actor)?)
        }
        "verify_project_output" => {
            let input: ProjectInput = parse(name, input)?;
            encode(
                name,
                df_facade::verify_project_output(&input.project_dir, actor)?,
            )
        }

        // Unreachable: the registry check above already rejected anything not
        // listed, and every listed name has an arm — which the
        // `every_tool_is_dispatchable` test proves.
        other => Err(DfError::Validation(format!(
            "tool `{other}` is declared but not dispatchable; this is a bug"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<_> = TOOLS.iter().map(|tool| tool.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate tool name in the registry");
    }

    #[test]
    fn every_tool_is_dispatchable() {
        // Guards the gap between the registry and the match: a tool listed but
        // not dispatched would answer "this is a bug" at runtime instead of
        // failing here. Missing input fields are what we expect back; the one
        // answer that must never appear is the not-dispatchable arm.
        for tool in TOOLS {
            let error = invoke(tool.name, serde_json::json!({}), Actor::Agent)
                .expect_err("no tool can succeed on empty input");
            let message = error.to_string();
            assert!(
                !message.contains("not dispatchable"),
                "tool `{}` is in the registry with no dispatch arm",
                tool.name
            );
            assert!(
                message.contains("invalid input"),
                "tool `{}` rejected empty input for the wrong reason: {message}",
                tool.name
            );
        }
    }

    #[test]
    fn an_unknown_tool_is_rejected() {
        let error = invoke("rm_rf", serde_json::json!({}), Actor::Agent)
            .expect_err("a tool outside the registry must not resolve");
        assert!(error.to_string().contains("unknown tool"));
    }

    #[test]
    fn unknown_input_fields_are_rejected() {
        // A bounded surface must not silently drop what it does not
        // understand: the caller would believe it had asked for something it
        // had not.
        let error = invoke(
            "plan_destination_tree",
            serde_json::json!({"project_dir": "/tmp/nowhere", "recursive": true}),
            Actor::Agent,
        )
        .expect_err("an unknown field must be rejected, not ignored");
        assert!(
            error.to_string().contains("invalid input"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn only_commit_requires_authorization() {
        // The classification is the contract df-rules will attach to, so it is
        // pinned here rather than left to the registry's good behaviour.
        for tool in TOOLS {
            assert_eq!(
                tool.capability.requires_authorization(),
                tool.capability == Capability::Commit,
                "tool `{}` disagrees with its class about authorization",
                tool.name
            );
        }
    }

    #[test]
    fn the_commit_class_is_exactly_the_three_gated_calls() {
        // If a tool ever joins this class, that is a decision that has to be
        // made deliberately: it is the set a human (or df-rules) authorises.
        let mut commit: Vec<_> = tools_with(Capability::Commit)
            .map(|tool| tool.name)
            .collect();
        commit.sort_unstable();
        assert_eq!(
            commit,
            ["approve_plan", "execute_plan", "verify_project_output"]
        );
    }

    #[test]
    fn the_surface_is_frozen() {
        // External agents pin this surface, so growing it is a version bump and
        // a deliberate act, never a side effect of adding a facade function.
        // Listing the names here is what forces that conversation to happen.
        let declared: Vec<_> = TOOLS
            .iter()
            .map(|tool| (tool.name, tool.capability))
            .collect();
        assert_eq!(
            declared,
            vec![
                ("project_status", Capability::Observe),
                ("duplicate_report", Capability::Observe),
                ("tree_clone_report", Capability::Observe),
                ("tree_relation_report", Capability::Observe),
                ("context_report", Capability::Observe),
                ("structural_anomaly_report", Capability::Observe),
                ("structural_review_queue", Capability::Observe),
                ("structural_review_classes", Capability::Observe),
                ("plan_destination_tree", Capability::Observe),
                ("destination_guarantees", Capability::Observe),
                ("verify_audit", Capability::Observe),
                ("scan_project", Capability::Build),
                ("hash_project", Capability::Build),
                ("analyze_project", Capability::Build),
                ("analyze_similarity", Capability::Build),
                ("analyze_media", Capability::Build),
                ("decide_structural_review", Capability::Build),
                ("decide_structural_review_batch", Capability::Build),
                ("create_plan", Capability::Build),
                ("validate_plan", Capability::Build),
                ("approve_plan", Capability::Commit),
                ("execute_plan", Capability::Commit),
                ("verify_project_output", Capability::Commit),
            ],
            "the tool surface changed; bump TOOL_SURFACE_VERSION and say why"
        );
        assert_eq!(TOOL_SURFACE_VERSION, "dataforge.tool-surface/0.1.0");
    }

    #[test]
    fn a_missing_project_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("no-such-project");
        let error = invoke(
            "project_status",
            serde_json::json!({"project_dir": missing}),
            Actor::Agent,
        )
        .expect_err("a project that does not exist cannot report status");
        // Any typed error is fine; what matters is that it is one.
        assert!(!error.to_string().is_empty());
    }
}
