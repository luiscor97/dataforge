//! Persistent M0.2 rule matches, structural anomalies and human review.
//!
//! All automatic findings are deterministic append-only evidence over an
//! immutable snapshot. An interrupted analysis can safely rerun: identical
//! findings use identical versioned IDs. Idempotent insert conflicts are
//! verified column-for-column, while human decisions remain a separate
//! append-only history.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use df_domain::{
    Actor, AnomalyKind, AnomalySeverity, OccurrenceId, OperationType, Profile, ProjectId,
    RiskLevel, RuleAction, ScanEntryStatus, SnapshotId,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::repository::{append_event, to_stored_timestamp};
use crate::{db_err, Db};

pub const EVENT_RULES_EVALUATED: &str = "RULES_EVALUATED";
pub const EVENT_ANOMALIES_DETECTED: &str = "ANOMALIES_DETECTED";
pub const EVENT_REVIEW_DECIDED: &str = "REVIEW_DECIDED";
pub const EVENT_STRUCTURAL_ANALYSIS_COMPLETED: &str = "STRUCTURAL_ANALYSIS_COMPLETED";
pub const ANALYSIS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RuleEvaluationSummary {
    pub matches: u64,
    pub review_items: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AnomalySummary {
    pub anomalies: u64,
    pub high: u64,
    pub review_items: u64,
}

/// Canonical counters sealed by one completed structural-analysis run.
///
/// This is deliberately typed rather than an open JSON object: phase recovery
/// must either reproduce the exact public outcome or fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisCompletionSummary {
    pub duplicate_sets: u64,
    pub folder_signatures: u64,
    pub tree_clone_sets: u64,
    pub partial_tree_clones: u64,
    pub embedded_trees: u64,
    /// Added while `ANALYSIS_VERSION` was still 1. Old v1 markers omit this
    /// field, which is equivalent to zero because that evidence did not exist.
    #[serde(default)]
    pub repeated_components: u64,
    /// True when bounded relation generation stopped before exhausting all
    /// possible distinct candidates. Added compatibly within v1.
    #[serde(default)]
    pub candidate_cap_reached: bool,
    pub generic_folders: u64,
    pub protected_boundaries: u64,
    pub duplicate_representatives: u64,
    pub rule_matches: u64,
    pub anomalies: u64,
    pub high_anomalies: u64,
    pub review_items: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewItemView {
    pub id: String,
    pub snapshot_id: String,
    pub occurrence_id: Option<String>,
    pub folder_a: Option<String>,
    pub folder_b: Option<String>,
    pub source: String,
    pub kind: String,
    pub risk: String,
    pub recommended_action: String,
    /// False when the immutable snapshot contains no readable bytes to route;
    /// the source must be repaired and rescanned instead of "decided".
    pub materializable: bool,
    pub reason: String,
    pub status: String,
    pub decision: Option<String>,
    pub rationale: Option<String>,
    /// Canonical automatic evidence for anomaly-backed items. Rule-backed
    /// reviews have no structural anomaly payload.
    pub evidence: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewQueue {
    pub snapshot_id: String,
    pub pending: u64,
    pub decided: u64,
    pub items: Vec<ReviewItemView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnomalyView {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub requires_review: bool,
    pub summary: String,
    pub occurrence_id: Option<String>,
    pub folder_a: Option<String>,
    pub folder_b: Option<String>,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnomalyReport {
    pub snapshot_id: String,
    pub high: u64,
    pub warnings: u64,
    pub information: u64,
    pub anomalies: Vec<AnomalyView>,
}

/// Planner-facing result of automatic rules plus the latest human decision.
#[derive(Debug, Clone, PartialEq)]
pub struct OccurrenceGuidance {
    pub operation_type: OperationType,
    pub risk: RiskLevel,
    pub confidence: f64,
    pub reason: String,
}

/// Compact diagnostic exposed by both CLI status and the desktop UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StructuralDiagnostics {
    pub analysis_complete: bool,
    pub folder_signatures: u64,
    pub exact_tree_clone_sets: u64,
    pub partial_tree_clones: u64,
    pub embedded_trees: u64,
    pub repeated_components: u64,
    pub candidate_cap_reached: bool,
    pub generic_folders: u64,
    pub protected_boundaries: u64,
    pub rule_matches: u64,
    pub anomalies: u64,
    pub high_anomalies: u64,
    pub pending_review: u64,
}

fn stable_id(namespace: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"dataforge-structural-analysis-id");
    hasher.update(ANALYSIS_VERSION.to_be_bytes());
    hasher.update((namespace.len() as u64).to_be_bytes());
    hasher.update(namespace.as_bytes());
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn canonical_json(value: &serde_json::Value) -> String {
    df_ledger::canonical_json(value)
}

fn resolved_profile_sha256(profile: &Profile) -> DfResult<String> {
    let value = serde_json::to_value(profile)
        .map_err(|error| DfError::Serialization(format!("resolved profile: {error}")))?;
    Ok(hex::encode(Sha256::digest(
        canonical_json(&value).as_bytes(),
    )))
}

/// Bind every automatic analysis write to the immutable complete snapshot and
/// to the profile configured on its owning project.
fn analysis_scope(db: &Db, project_id: ProjectId, snapshot_id: SnapshotId) -> DfResult<String> {
    let scope: Option<(String, String)> = db
        .conn()
        .query_row(
            "SELECT p.profile, s.status
             FROM snapshots s JOIN projects p ON p.id = s.project_id
             WHERE s.id = ?1 AND s.project_id = ?2",
            params![snapshot_id.to_string(), project_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    let (profile_id, status) = scope.ok_or_else(|| {
        DfError::Conflict(format!(
            "snapshot `{snapshot_id}` does not belong to project `{project_id}`"
        ))
    })?;
    if status != "COMPLETE" {
        return Err(DfError::Validation(format!(
            "snapshot `{snapshot_id}` is {status}; structural analysis requires COMPLETE"
        )));
    }
    Ok(profile_id)
}

fn load_analysis_profile(
    db: &Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    requested_profile: &str,
) -> DfResult<(Profile, String)> {
    let configured_profile = analysis_scope(db, project_id, snapshot_id)?;
    if configured_profile != requested_profile {
        return Err(DfError::Conflict(format!(
            "project profile is `{configured_profile}`, not requested `{requested_profile}`"
        )));
    }
    let profile = Profile::load(&configured_profile)?;
    let digest = resolved_profile_sha256(&profile)?;
    Ok((profile, digest))
}

fn require_matching_completion_scope(
    db: &Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile_id: &str,
    profile_sha256: &str,
) -> DfResult<()> {
    let stored: Option<(String, String, String)> = db
        .conn()
        .query_row(
            "SELECT project_id, profile_id, profile_sha256
             FROM analysis_completions
             WHERE snapshot_id = ?1 AND analysis_version = ?2",
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_err)?;
    if let Some((stored_project, stored_profile, stored_digest)) = stored {
        if stored_project != project_id.to_string()
            || stored_profile != profile_id
            || stored_digest != profile_sha256
        {
            return Err(DfError::Conflict(format!(
                "completed analysis scope for snapshot `{snapshot_id}` does not match"
            )));
        }
    }
    Ok(())
}

/// Read the bounded-relation cutoff from immutable ledger evidence. Current
/// events store the boolean directly; early v1 events stored the exact
/// `pairs_skipped` count, which can be losslessly projected to the boolean.
fn relation_candidate_cap_reached(db: &Db, snapshot_id: SnapshotId) -> DfResult<Option<bool>> {
    let snapshot = snapshot_id.to_string();
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT payload_json
             FROM audit_events
             WHERE project_id = (
                       SELECT project_id FROM snapshots WHERE id = ?1
                   )
               AND event_type = 'STRUCTURE_ANALYZED'
             ORDER BY sequence DESC",
        )
        .map_err(db_err)?;
    let payloads = stmt
        .query_map([&snapshot], |row| row.get::<_, String>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    for stored in payloads {
        let payload: serde_json::Value = serde_json::from_str(&stored)
            .map_err(|error| DfError::Serialization(format!("structure event: {error}")))?;
        if payload
            .get("snapshot_id")
            .and_then(serde_json::Value::as_str)
            != Some(snapshot.as_str())
        {
            continue;
        }
        if let Some(value) = payload.get("candidate_cap_reached") {
            return value.as_bool().map(Some).ok_or_else(|| {
                DfError::Serialization(
                    "structure event candidate_cap_reached is not boolean".to_string(),
                )
            });
        }
        if let Some(value) = payload.get("pairs_skipped") {
            return value.as_u64().map(|count| Some(count > 0)).ok_or_else(|| {
                DfError::Serialization(
                    "legacy structure event pairs_skipped is not an unsigned integer".to_string(),
                )
            });
        }
    }
    Ok(None)
}

fn summary_from_immutable_evidence(
    db: &Db,
    snapshot_id: SnapshotId,
) -> DfResult<AnalysisCompletionSummary> {
    let scalar = |sql: &str| -> DfResult<u64> {
        let value: i64 = db
            .conn()
            .query_row(sql, [snapshot_id.to_string()], |row| row.get(0))
            .map_err(db_err)?;
        Ok(value as u64)
    };
    let scalar_versioned = |sql: &str| -> DfResult<u64> {
        let value: i64 = db
            .conn()
            .query_row(
                sql,
                params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(value as u64)
    };
    Ok(AnalysisCompletionSummary {
        duplicate_sets: scalar("SELECT COUNT(*) FROM duplicate_sets WHERE snapshot_id = ?1")?,
        folder_signatures: scalar("SELECT COUNT(*) FROM folder_signatures WHERE snapshot_id = ?1")?,
        tree_clone_sets: scalar("SELECT COUNT(*) FROM tree_clone_sets WHERE snapshot_id = ?1")?,
        partial_tree_clones: scalar(
            "SELECT COUNT(*) FROM tree_relations
             WHERE snapshot_id = ?1 AND relationship = 'PARTIAL_TREE_CLONE'",
        )?,
        embedded_trees: scalar(
            "SELECT COUNT(*) FROM tree_relations
             WHERE snapshot_id = ?1 AND relationship = 'TREE_EMBEDDED'",
        )?,
        repeated_components: scalar(
            "SELECT COUNT(*) FROM tree_relations
             WHERE snapshot_id = ?1 AND relationship = 'REPEATED_COMPONENT_ONLY'",
        )?,
        candidate_cap_reached: relation_candidate_cap_reached(db, snapshot_id)?.ok_or_else(|| {
            DfError::Conflict(format!(
                "completed analysis for snapshot `{snapshot_id}` has no immutable tree-relation cap evidence"
            ))
        })?,
        generic_folders: scalar(
            "SELECT COUNT(*) FROM folder_contexts
             WHERE snapshot_id = ?1 AND kind = 'GENERIC'",
        )?,
        protected_boundaries: scalar(
            "SELECT COUNT(*) FROM folder_contexts
             WHERE snapshot_id = ?1 AND is_protected_boundary = 1",
        )?,
        duplicate_representatives: scalar(
            "SELECT COUNT(*) FROM duplicate_representatives WHERE snapshot_id = ?1",
        )?,
        rule_matches: scalar_versioned(
            "SELECT COUNT(*) FROM rule_matches
             WHERE snapshot_id = ?1 AND analysis_version = ?2",
        )?,
        anomalies: scalar_versioned(
            "SELECT COUNT(*) FROM structural_anomalies
             WHERE snapshot_id = ?1 AND analysis_version = ?2",
        )?,
        high_anomalies: scalar_versioned(
            "SELECT COUNT(*) FROM structural_anomalies
             WHERE snapshot_id = ?1 AND analysis_version = ?2 AND severity = 'HIGH'",
        )?,
        review_items: scalar_versioned(
            "SELECT COUNT(*) FROM review_items
             WHERE snapshot_id = ?1 AND analysis_version = ?2",
        )?,
    })
}

/// Load and validate the current sealed completion, if one exists for this
/// snapshot. A completion from another analysis version deliberately requires
/// a new project/snapshot through the supported creation flow: the unversioned
/// derived tables are immutable once sealed, so silently rewriting their
/// historical meaning is forbidden.
pub fn sealed_analysis_summary(
    db: &Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile_id: &str,
) -> DfResult<Option<AnalysisCompletionSummary>> {
    let (profile, profile_sha256) = load_analysis_profile(db, project_id, snapshot_id, profile_id)?;
    let stored: Option<(String, i64, String, String, String)> = db
        .conn()
        .query_row(
            "SELECT project_id, analysis_version, profile_id, profile_sha256,
                    summary_json
             FROM analysis_completions
             WHERE snapshot_id = ?1 AND analysis_version = ?2",
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?;
    let Some((stored_project, stored_version, stored_profile, stored_digest, stored_json)) = stored
    else {
        let sealed_version: Option<i64> = db
            .conn()
            .query_row(
                "SELECT MAX(analysis_version) FROM analysis_completions
                 WHERE snapshot_id = ?1",
                [snapshot_id.to_string()],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if let Some(sealed_version) = sealed_version {
            return Err(DfError::Validation(format!(
                "snapshot `{snapshot_id}` is sealed by analysis version {sealed_version}; \
                 analysis version {ANALYSIS_VERSION} requires a new project/snapshot through \
                 the supported project creation flow"
            )));
        }
        return Ok(None);
    };
    if stored_project != project_id.to_string()
        || stored_version != ANALYSIS_VERSION as i64
        || stored_profile != profile.id
        || stored_digest != profile_sha256
    {
        return Err(DfError::Conflict(format!(
            "completed analysis scope for snapshot `{snapshot_id}` does not match"
        )));
    }
    let summary_value: serde_json::Value = serde_json::from_str(&stored_json)
        .map_err(|error| DfError::Serialization(format!("stored analysis summary: {error}")))?;
    if canonical_json(&summary_value) != stored_json {
        return Err(DfError::Conflict(format!(
            "completed analysis summary for snapshot `{snapshot_id}` is not canonical"
        )));
    }
    // Deserialize only after checking the original JSON. Reserializing the
    // typed value would add defaulted fields and falsely reject canonical v1
    // markers written before those counters existed.
    let candidate_cap_was_stored = summary_value.get("candidate_cap_reached").is_some();
    let mut summary: AnalysisCompletionSummary = serde_json::from_value(summary_value)
        .map_err(|error| DfError::Serialization(format!("stored analysis summary: {error}")))?;
    let materialized = summary_from_immutable_evidence(db, snapshot_id)?;
    if !candidate_cap_was_stored {
        // Early v1 markers predate this summary field. Recover the honest value
        // from the immutable STRUCTURE_ANALYZED event (`pairs_skipped` in the
        // old payload) instead of claiming the cap was not reached.
        summary.candidate_cap_reached = materialized.candidate_cap_reached;
    }
    if materialized != summary {
        return Err(DfError::Conflict(format!(
            "completed analysis summary for snapshot `{snapshot_id}` does not match sealed evidence"
        )));
    }
    Ok(Some(summary))
}

/// Require the current structural-analysis contract before planning or
/// approving. A lifecycle state alone is not evidence: pre-0010 databases and
/// manually advanced projects may say ANALYZED without having evaluated the
/// configured profile. The profile digest also prevents planning against a
/// profile file that changed after analysis completed.
pub fn require_current_analysis_completion(
    db: &Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile_id: &str,
) -> DfResult<()> {
    let (profile, profile_sha256) = load_analysis_profile(db, project_id, snapshot_id, profile_id)?;
    if !is_analysis_complete(db, snapshot_id)? {
        return Err(DfError::Validation(format!(
            "structural analysis for snapshot `{snapshot_id}` has not completed; \
             run analyze before planning or requesting reports"
        )));
    }
    require_matching_completion_scope(db, project_id, snapshot_id, &profile.id, &profile_sha256)
}

/// Evaluate the ordered rules of a resolved profile. Only the first match is
/// selected as the default action, making rule precedence explicit and stable.
pub fn evaluate_rules(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile_id: &str,
    actor: Actor,
) -> DfResult<RuleEvaluationSummary> {
    let (profile, profile_sha256) = load_analysis_profile(db, project_id, snapshot_id, profile_id)?;
    if is_analysis_complete(db, snapshot_id)? {
        require_matching_completion_scope(
            db,
            project_id,
            snapshot_id,
            &profile.id,
            &profile_sha256,
        )?;
        return rule_evaluation_summary(db, snapshot_id);
    }
    let occurrences = crate::inventory::list_occurrences(db, snapshot_id)?;
    let mut matches = Vec::new();

    for occurrence in occurrences
        .iter()
        .filter(|occurrence| occurrence.scan_status == ScanEntryStatus::Ok)
    {
        let matching: Vec<_> = profile
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.matches_file_name(&occurrence.file_name))
            .collect();
        for (selection_index, (priority, rule)) in matching.into_iter().enumerate() {
            let selected = selection_index == 0;
            let id = stable_id(
                "rule-match",
                &[
                    &snapshot_id.to_string(),
                    &occurrence.id.to_string(),
                    &rule.id,
                    &rule.version.to_string(),
                ],
            );
            let evidence = canonical_json(&serde_json::json!({
                "file_name": occurrence.file_name,
                "file_name_glob": rule.match_spec.file_name_glob,
                "relative_path": occurrence.relative_path,
                "profile_id": profile.id,
                "profile_sha256": profile_sha256,
                "rule_id": rule.id,
                "rule_version": rule.version,
                "priority": priority,
                "selected": selected,
            }));
            matches.push((
                id,
                occurrence.id,
                rule.clone(),
                priority,
                selected,
                evidence,
            ));
        }
    }

    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let mut inserted_matches = 0_u64;
    let mut inserted_reviews = 0_u64;
    for (id, occurrence_id, rule, priority, selected, evidence) in &matches {
        let match_inserted = tx
            .execute(
                "INSERT INTO rule_matches
                    (id, snapshot_id, occurrence_id, analysis_version,
                     profile_id, profile_sha256, rule_id, rule_version,
                     priority, is_selected, category, action, confidence, risk,
                     evidence_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT DO NOTHING",
                params![
                    id,
                    snapshot_id.to_string(),
                    occurrence_id.to_string(),
                    ANALYSIS_VERSION as i64,
                    profile.id,
                    profile_sha256,
                    rule.id,
                    rule.version as i64,
                    *priority as i64,
                    *selected as i64,
                    rule.classification.category,
                    rule.action.as_str(),
                    rule.classification.confidence,
                    rule.risk.as_str(),
                    evidence,
                    now,
                ],
            )
            .map_err(db_err)?;
        if match_inserted == 0 {
            let identical: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM rule_matches
                        WHERE id = ?1 AND snapshot_id = ?2
                          AND occurrence_id = ?3 AND analysis_version = ?4
                          AND profile_id = ?5 AND profile_sha256 = ?6
                          AND rule_id = ?7 AND rule_version = ?8
                          AND priority = ?9 AND is_selected = ?10
                          AND category = ?11 AND action = ?12
                          AND confidence = ?13 AND risk = ?14
                          AND evidence_json = ?15
                     )",
                    params![
                        id,
                        snapshot_id.to_string(),
                        occurrence_id.to_string(),
                        ANALYSIS_VERSION as i64,
                        profile.id,
                        profile_sha256,
                        rule.id,
                        rule.version as i64,
                        *priority as i64,
                        *selected as i64,
                        rule.classification.category,
                        rule.action.as_str(),
                        rule.classification.confidence,
                        rule.risk.as_str(),
                        evidence,
                    ],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            if !identical {
                return Err(DfError::Conflict(format!(
                    "rule match `{id}` conflicts with previously stored evidence"
                )));
            }
        }
        inserted_matches += match_inserted as u64;

        if (*selected && rule.action == RuleAction::CopyReview) || rule.risk == RiskLevel::High {
            let review_id = stable_id("rule-review", &[id]);
            let recommended_action = if *selected {
                rule.action
            } else {
                RuleAction::CopyReview
            };
            let reason = format!(
                "rule `{}` classified the occurrence as `{}` with {:.0}% confidence",
                rule.id,
                rule.classification.category,
                rule.classification.confidence * 100.0
            );
            let review_inserted = tx
                .execute(
                    "INSERT INTO review_items
                        (id, snapshot_id, analysis_version, anomaly_id,
                         rule_match_id, occurrence_id, recommended_action, risk,
                         reason, created_at)
                     VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT DO NOTHING",
                    params![
                        review_id,
                        snapshot_id.to_string(),
                        ANALYSIS_VERSION as i64,
                        id,
                        occurrence_id.to_string(),
                        recommended_action.as_str(),
                        rule.risk.as_str(),
                        reason,
                        now,
                    ],
                )
                .map_err(db_err)?;
            if review_inserted == 0 {
                let identical: bool = tx
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM review_items
                            WHERE id = ?1 AND snapshot_id = ?2
                              AND analysis_version = ?3 AND anomaly_id IS NULL
                              AND rule_match_id = ?4 AND occurrence_id = ?5
                              AND recommended_action = ?6 AND risk = ?7
                              AND reason = ?8
                         )",
                        params![
                            review_id,
                            snapshot_id.to_string(),
                            ANALYSIS_VERSION as i64,
                            id,
                            occurrence_id.to_string(),
                            recommended_action.as_str(),
                            rule.risk.as_str(),
                            reason,
                        ],
                        |row| row.get(0),
                    )
                    .map_err(db_err)?;
                if !identical {
                    return Err(DfError::Conflict(format!(
                        "review item `{review_id}` conflicts with previously stored evidence"
                    )));
                }
            }
            inserted_reviews += review_inserted as u64;
        }
    }
    let payload = serde_json::json!({
        "snapshot_id": snapshot_id.to_string(),
        "profile_id": profile.id,
        "matches_inserted": inserted_matches,
        "review_items_inserted": inserted_reviews,
    });
    append_event(&tx, project_id, EVENT_RULES_EVALUATED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    rule_evaluation_summary(db, snapshot_id)
}

#[derive(Debug, Clone)]
struct OccurrenceRow {
    id: String,
    source_root_id: String,
    parent: String,
    relative_path: String,
    display_path: String,
    file_name: String,
    normalized_name: String,
    path_length: u64,
    status: String,
    error: Option<String>,
    path_is_lossy: bool,
    content_id: Option<String>,
}

#[derive(Debug, Clone)]
struct AnomalyCandidate {
    id: String,
    occurrence_id: Option<String>,
    folder_a: Option<String>,
    folder_b: Option<String>,
    kind: AnomalyKind,
    severity: AnomalySeverity,
    requires_review: bool,
    summary: String,
    evidence: String,
}

impl AnomalyCandidate {
    fn occurrence(
        snapshot_id: SnapshotId,
        row: &OccurrenceRow,
        kind: AnomalyKind,
        severity: AnomalySeverity,
        summary: String,
        evidence: serde_json::Value,
    ) -> Self {
        Self {
            id: stable_id(
                "occurrence-anomaly",
                &[&snapshot_id.to_string(), &row.id, kind.as_str()],
            ),
            occurrence_id: Some(row.id.clone()),
            folder_a: None,
            folder_b: None,
            kind,
            severity,
            requires_review: true,
            summary,
            evidence: canonical_json(&evidence),
        }
    }
}

fn load_occurrence_rows(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<OccurrenceRow>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT o.id, o.source_root_id, o.parent_relative_path,
                    o.relative_path, o.file_name, o.normalized_name,
                    o.path_length, o.scan_status, o.error, o.name_is_lossy,
                    oc.content_id, o.raw_relative_path
             FROM path_occurrences o
             LEFT JOIN occurrence_content oc ON oc.occurrence_id = o.id
             WHERE o.snapshot_id = ?1
             ORDER BY o.source_root_id, o.relative_path, o.id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<Vec<u8>>>(11)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    rows.into_iter()
        .map(
            |(
                id,
                source_root_id,
                parent,
                relative_path,
                file_name,
                normalized_name,
                path_length,
                status,
                error,
                name_is_lossy,
                content_id,
                raw_blob,
            )| {
                let raw_path = raw_blob
                    .as_deref()
                    .map(df_domain::RawPath::from_blob)
                    .transpose()?;
                let display_path = raw_path
                    .as_ref()
                    .map(df_domain::RawPath::display)
                    .unwrap_or_else(|| relative_path.clone());
                let path_is_lossy = name_is_lossy != 0
                    || raw_path.as_ref().is_some_and(df_domain::RawPath::is_lossy);
                Ok(OccurrenceRow {
                    id,
                    source_root_id,
                    parent,
                    relative_path,
                    display_path,
                    file_name,
                    normalized_name,
                    path_length: path_length as u64,
                    status,
                    error,
                    path_is_lossy,
                    content_id,
                })
            },
        )
        .collect()
}

/// Detect deterministic structural anomalies and create review items for
/// every ambiguous or high-risk finding.
pub fn detect_anomalies(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    actor: Actor,
) -> DfResult<AnomalySummary> {
    let configured_profile = analysis_scope(db, project_id, snapshot_id)?;
    let profile = Profile::load(&configured_profile)?;
    let profile_sha256 = resolved_profile_sha256(&profile)?;
    if is_analysis_complete(db, snapshot_id)? {
        require_matching_completion_scope(
            db,
            project_id,
            snapshot_id,
            &configured_profile,
            &profile_sha256,
        )?;
        return anomaly_summary(db, snapshot_id);
    }
    let rows = load_occurrence_rows(db, snapshot_id)?;
    let mut candidates = Vec::new();

    // Same folder + same comparison name + different content identity is a
    // collision/variant, never an exact duplicate (§12.6).
    let mut by_name: BTreeMap<(String, String, String), Vec<&OccurrenceRow>> = BTreeMap::new();
    for row in &rows {
        if row.content_id.is_some() {
            by_name
                .entry((
                    row.source_root_id.clone(),
                    row.parent.clone(),
                    row.normalized_name.clone(),
                ))
                .or_default()
                .push(row);
        }
    }
    for ((source_root_id, parent, comparison_name), group) in &by_name {
        let contents: HashSet<&str> = group
            .iter()
            .filter_map(|row| row.content_id.as_deref())
            .collect();
        if contents.len() <= 1 {
            continue;
        }
        let group_id = stable_id(
            "same-name-group",
            &[
                &snapshot_id.to_string(),
                source_root_id,
                parent,
                comparison_name,
            ],
        );
        for row in group {
            candidates.push(AnomalyCandidate::occurrence(
                snapshot_id,
                row,
                AnomalyKind::SameNameDifferentContent,
                AnomalySeverity::Warning,
                format!(
                    "`{}` shares its name with different content in the same folder",
                    row.relative_path
                ),
                serde_json::json!({
                    "group_id": group_id,
                    "comparison_name": comparison_name,
                    "group_occurrences": group.len(),
                    "distinct_contents": contents.len(),
                    "this_content_id": row.content_id,
                    "this_relative_path": row.relative_path,
                }),
            ));
        }
    }

    for row in &rows {
        if row.path_is_lossy {
            candidates.push(AnomalyCandidate::occurrence(
                snapshot_id,
                row,
                AnomalyKind::LossyPathIdentity,
                AnomalySeverity::High,
                format!("`{}` has a lossy path identity", row.display_path),
                serde_json::json!({
                    "file_name": row.file_name,
                    "relative_path_display": row.display_path,
                    "relative_path_storage_key": row.relative_path,
                    "raw_identity_preserved": true,
                }),
            ));
        }
        if row.status != ScanEntryStatus::Ok.as_str() {
            candidates.push(AnomalyCandidate::occurrence(
                snapshot_id,
                row,
                AnomalyKind::UnreadableEntry,
                AnomalySeverity::High,
                format!("`{}` was not read normally", row.relative_path),
                serde_json::json!({
                    "scan_status": row.status,
                    "error": row.error,
                }),
            ));
        }
        if row.path_length >= 240 {
            candidates.push(AnomalyCandidate::occurrence(
                snapshot_id,
                row,
                AnomalyKind::ExtremePath,
                AnomalySeverity::Warning,
                format!(
                    "`{}` is close to or beyond common Windows path limits",
                    row.relative_path
                ),
                serde_json::json!({ "utf16_path_length": row.path_length }),
            ));
        }
    }

    // Folder read errors and unfollowed reparse points make a branch
    // incomplete; surface them instead of hiding that fact in its signature.
    {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT id, relative_path, status, error, raw_relative_path
                 FROM folders
                 WHERE snapshot_id = ?1
                 ORDER BY source_root_id, relative_path, id",
            )
            .map_err(db_err)?;
        let folders = stmt
            .query_map([snapshot_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        for (folder_id, path, status, error, raw_blob) in folders {
            let raw_path = raw_blob
                .as_deref()
                .map(df_domain::RawPath::from_blob)
                .transpose()?;
            let display_path = raw_path
                .as_ref()
                .map(df_domain::RawPath::display)
                .unwrap_or_else(|| path.clone());
            if raw_path.as_ref().is_some_and(df_domain::RawPath::is_lossy) {
                let kind = AnomalyKind::LossyPathIdentity;
                candidates.push(AnomalyCandidate {
                    id: stable_id(
                        "folder-anomaly",
                        &[&snapshot_id.to_string(), &folder_id, kind.as_str()],
                    ),
                    occurrence_id: None,
                    folder_a: Some(folder_id.clone()),
                    folder_b: None,
                    kind,
                    severity: AnomalySeverity::High,
                    requires_review: true,
                    summary: format!("folder `{display_path}` has a lossy path identity"),
                    evidence: canonical_json(&serde_json::json!({
                        "relative_path_display": display_path,
                        "relative_path_storage_key": path,
                        "raw_identity_preserved": true,
                    })),
                });
            }
            let kind = AnomalyKind::UnreadableEntry;
            if status != ScanEntryStatus::Ok.as_str() {
                candidates.push(AnomalyCandidate {
                    id: stable_id(
                        "folder-anomaly",
                        &[&snapshot_id.to_string(), &folder_id, kind.as_str()],
                    ),
                    occurrence_id: None,
                    folder_a: Some(folder_id),
                    folder_b: None,
                    kind,
                    severity: AnomalySeverity::High,
                    requires_review: true,
                    summary: format!("folder `{display_path}` is incomplete ({status})"),
                    evidence: canonical_json(&serde_json::json!({
                        "relative_path_display": display_path,
                        "relative_path_storage_key": path,
                        "scan_status": status,
                        "error": error,
                    })),
                });
            }
        }
    }

    // Tree relations already contain the preservation evidence from §19.4.
    {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT folder_a, folder_b, relationship, shared_files,
                        unique_a_files, unique_b_files, shared_bytes, similarity
                 FROM tree_relations
                 WHERE snapshot_id = ?1
                 ORDER BY folder_a, folder_b",
            )
            .map_err(db_err)?;
        let relations = stmt
            .query_map([snapshot_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, i64>(4)? as u64,
                    row.get::<_, i64>(5)? as u64,
                    row.get::<_, i64>(6)? as u64,
                    row.get::<_, f64>(7)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        for (a, b, relationship, shared, unique_a, unique_b, bytes, similarity) in relations {
            let (kind, severity, summary) = if relationship == "PARTIAL_TREE_CLONE" {
                (
                    AnomalyKind::PartialTreeUniqueContent,
                    AnomalySeverity::Warning,
                    format!(
                        "related trees contain unique content on both sides ({unique_a} vs {unique_b})"
                    ),
                )
            } else if relationship == "TREE_EMBEDDED" {
                (
                    AnomalyKind::EmbeddedTree,
                    AnomalySeverity::Info,
                    "one folder tree is embedded in another; preserve until reviewed".to_string(),
                )
            } else {
                continue;
            };
            candidates.push(AnomalyCandidate {
                id: stable_id(
                    "tree-anomaly",
                    &[&snapshot_id.to_string(), &a, &b, kind.as_str()],
                ),
                occurrence_id: None,
                folder_a: Some(a),
                folder_b: Some(b),
                kind,
                severity,
                requires_review: true,
                summary,
                evidence: canonical_json(&serde_json::json!({
                    "relationship": relationship,
                    "shared_files": shared,
                    "unique_a_files": unique_a,
                    "unique_b_files": unique_b,
                    "shared_bytes": bytes,
                    "similarity": similarity,
                })),
            });
        }
    }

    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    candidates.dedup_by(|a, b| a.id == b.id);
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let mut inserted = 0_u64;
    let mut inserted_reviews = 0_u64;
    for candidate in &candidates {
        let anomaly_inserted = tx
            .execute(
                "INSERT INTO structural_anomalies
                    (id, snapshot_id, analysis_version, occurrence_id, folder_a,
                     folder_b, kind, severity, requires_review, summary,
                     evidence_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT DO NOTHING",
                params![
                    candidate.id,
                    snapshot_id.to_string(),
                    ANALYSIS_VERSION as i64,
                    candidate.occurrence_id,
                    candidate.folder_a,
                    candidate.folder_b,
                    candidate.kind.as_str(),
                    candidate.severity.as_str(),
                    candidate.requires_review as i64,
                    candidate.summary,
                    candidate.evidence,
                    now,
                ],
            )
            .map_err(db_err)?;
        if anomaly_inserted == 0 {
            let identical: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM structural_anomalies
                        WHERE id = ?1 AND snapshot_id = ?2
                          AND analysis_version = ?3
                          AND occurrence_id IS ?4 AND folder_a IS ?5
                          AND folder_b IS ?6 AND kind = ?7 AND severity = ?8
                          AND requires_review = ?9 AND summary = ?10
                          AND evidence_json = ?11
                     )",
                    params![
                        candidate.id,
                        snapshot_id.to_string(),
                        ANALYSIS_VERSION as i64,
                        candidate.occurrence_id,
                        candidate.folder_a,
                        candidate.folder_b,
                        candidate.kind.as_str(),
                        candidate.severity.as_str(),
                        candidate.requires_review as i64,
                        candidate.summary,
                        candidate.evidence,
                    ],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            if !identical {
                return Err(DfError::Conflict(format!(
                    "structural anomaly `{}` conflicts with previously stored evidence",
                    candidate.id
                )));
            }
        }
        inserted += anomaly_inserted as u64;
        if candidate.requires_review {
            let review_id = stable_id("anomaly-review", &[&candidate.id]);
            let review_inserted = tx
                .execute(
                    "INSERT INTO review_items
                        (id, snapshot_id, analysis_version, anomaly_id,
                         rule_match_id, occurrence_id, recommended_action, risk,
                         reason, created_at)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'COPY_REVIEW', ?6, ?7, ?8)
                     ON CONFLICT DO NOTHING",
                    params![
                        review_id,
                        snapshot_id.to_string(),
                        ANALYSIS_VERSION as i64,
                        candidate.id,
                        candidate.occurrence_id,
                        candidate.severity.risk().as_str(),
                        candidate.summary,
                        now,
                    ],
                )
                .map_err(db_err)?;
            if review_inserted == 0 {
                let identical: bool = tx
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM review_items
                            WHERE id = ?1 AND snapshot_id = ?2
                              AND analysis_version = ?3 AND anomaly_id = ?4
                              AND rule_match_id IS NULL AND occurrence_id IS ?5
                              AND recommended_action = 'COPY_REVIEW'
                              AND risk = ?6 AND reason = ?7
                         )",
                        params![
                            review_id,
                            snapshot_id.to_string(),
                            ANALYSIS_VERSION as i64,
                            candidate.id,
                            candidate.occurrence_id,
                            candidate.severity.risk().as_str(),
                            candidate.summary,
                        ],
                        |row| row.get(0),
                    )
                    .map_err(db_err)?;
                if !identical {
                    return Err(DfError::Conflict(format!(
                        "review item `{review_id}` conflicts with previously stored evidence"
                    )));
                }
            }
            inserted_reviews += review_inserted as u64;
        }
    }
    let payload = serde_json::json!({
        "snapshot_id": snapshot_id.to_string(),
        "anomalies_inserted": inserted,
        "review_items_inserted": inserted_reviews,
    });
    append_event(&tx, project_id, EVENT_ANOMALIES_DETECTED, &payload, actor)?;
    tx.commit().map_err(db_err)?;

    anomaly_summary(db, snapshot_id)
}

fn rule_evaluation_summary(db: &Db, snapshot_id: SnapshotId) -> DfResult<RuleEvaluationSummary> {
    let review_items: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM review_items
             WHERE snapshot_id = ?1 AND analysis_version = ?2
               AND rule_match_id IS NOT NULL",
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    Ok(RuleEvaluationSummary {
        matches: count_where(db, "rule_matches", snapshot_id, None)?,
        review_items: review_items as u64,
    })
}

fn anomaly_summary(db: &Db, snapshot_id: SnapshotId) -> DfResult<AnomalySummary> {
    Ok(AnomalySummary {
        anomalies: count_where(db, "structural_anomalies", snapshot_id, None)?,
        high: count_where(
            db,
            "structural_anomalies",
            snapshot_id,
            Some(("severity", "HIGH")),
        )?,
        review_items: count_where(db, "review_items", snapshot_id, None)?,
    })
}

fn count_where(
    db: &Db,
    table: &str,
    snapshot_id: SnapshotId,
    filter: Option<(&str, &str)>,
) -> DfResult<u64> {
    // Callers pass only these internal constants; keeping one helper avoids a
    // dozen subtly different diagnostic queries without exposing SQL outside
    // df-db.
    let allowed_table = matches!(
        table,
        "rule_matches" | "structural_anomalies" | "review_items"
    );
    let allowed_filter =
        filter.is_none() || matches!(filter, Some(("severity", "HIGH" | "WARNING" | "INFO")));
    if !allowed_table || !allowed_filter {
        return Err(DfError::Database(
            "internal diagnostic query was not allow-listed".to_string(),
        ));
    }
    let sql = match filter {
        Some((column, _)) => {
            format!(
                "SELECT COUNT(*) FROM {table}
                 WHERE snapshot_id = ?1 AND analysis_version = ?2 AND {column} = ?3"
            )
        }
        None => format!(
            "SELECT COUNT(*) FROM {table}
             WHERE snapshot_id = ?1 AND analysis_version = ?2"
        ),
    };
    let count: i64 = match filter {
        Some((_, value)) => db.conn().query_row(
            &sql,
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64, value],
            |row| row.get(0),
        ),
        None => db.conn().query_row(
            &sql,
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |row| row.get(0),
        ),
    }
    .map_err(db_err)?;
    Ok(count as u64)
}

fn parse_risk(value: &str) -> DfResult<RiskLevel> {
    RiskLevel::parse(value)
}

fn max_risk(a: RiskLevel, b: RiskLevel) -> RiskLevel {
    let rank = |risk| match risk {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// Resolve automatic guidance and human overrides for planning.
pub fn occurrence_guidance(
    db: &Db,
    snapshot_id: SnapshotId,
) -> DfResult<HashMap<OccurrenceId, OccurrenceGuidance>> {
    let mut guidance = HashMap::new();
    {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT occurrence_id, rule_id, action, confidence, risk
                 FROM rule_matches
                 WHERE snapshot_id = ?1 AND analysis_version = ?2
                   AND is_selected = 1
                 ORDER BY occurrence_id, priority, id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(
                params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        for (occurrence, rule_id, action, confidence, risk) in rows {
            guidance
                .entry(OccurrenceId::from_str(&occurrence)?)
                .or_insert(OccurrenceGuidance {
                    operation_type: RuleAction::parse(&action)?.operation_type(),
                    risk: parse_risk(&risk)?,
                    confidence,
                    reason: format!("declarative rule `{rule_id}` selected `{action}`"),
                });
        }
    }

    #[derive(Debug, Clone)]
    struct ReviewRow {
        item_id: String,
        occurrence_id: String,
        risk: String,
        decision: Option<String>,
        rationale: Option<String>,
    }
    let mut reviews: Vec<ReviewRow> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT ri.id, ri.occurrence_id, ri.risk,
                        (SELECT rd.decision FROM review_decisions rd
                         WHERE rd.review_item_id = ri.id
                         ORDER BY rd.sequence DESC LIMIT 1),
                        (SELECT rd.rationale FROM review_decisions rd
                         WHERE rd.review_item_id = ri.id
                         ORDER BY rd.sequence DESC LIMIT 1)
                 FROM review_items ri
                 WHERE ri.snapshot_id = ?1 AND ri.analysis_version = ?2
                   AND ri.occurrence_id IS NOT NULL
                 ORDER BY ri.occurrence_id, ri.id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(
                params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
                |row| {
                    Ok(ReviewRow {
                        item_id: row.get(0)?,
                        occurrence_id: row.get(1)?,
                        risk: row.get(2)?,
                        decision: row.get(3)?,
                        rationale: row.get(4)?,
                    })
                },
            )
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    // A structural anomaly can concern a whole folder pair rather than one
    // occurrence. Expand those review items over the occurrence-bearing
    // subtrees before aggregating decisions. The lookup is indexed by folder
    // path and each occurrence walks only its ancestors, avoiding an
    // unbounded review-items × occurrences cross-product.
    #[derive(Debug, Clone)]
    struct FolderReview {
        item_id: String,
        risk: String,
        decision: Option<String>,
        rationale: Option<String>,
        folder_a: Option<String>,
        folder_b: Option<String>,
    }
    let folder_reviews: Vec<FolderReview> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT ri.id, ri.risk,
                        (SELECT rd.decision FROM review_decisions rd
                         WHERE rd.review_item_id = ri.id
                         ORDER BY rd.sequence DESC LIMIT 1),
                        (SELECT rd.rationale FROM review_decisions rd
                         WHERE rd.review_item_id = ri.id
                         ORDER BY rd.sequence DESC LIMIT 1),
                        sa.folder_a, sa.folder_b
                 FROM review_items ri
                 JOIN structural_anomalies sa ON sa.id = ri.anomaly_id
                 WHERE ri.snapshot_id = ?1 AND ri.analysis_version = ?2
                   AND ri.occurrence_id IS NULL
                   AND (sa.folder_a IS NOT NULL OR sa.folder_b IS NOT NULL)
                 ORDER BY ri.id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(
                params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
                |row| {
                    Ok(FolderReview {
                        item_id: row.get(0)?,
                        risk: row.get(1)?,
                        decision: row.get(2)?,
                        rationale: row.get(3)?,
                        folder_a: row.get(4)?,
                        folder_b: row.get(5)?,
                    })
                },
            )
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };
    if !folder_reviews.is_empty() {
        let folders = crate::inventory::list_folders(db, snapshot_id)?;
        let folder_scope: HashMap<String, (df_domain::SourceRootId, String)> = folders
            .into_iter()
            .map(|folder| {
                (
                    folder.id.to_string(),
                    (folder.source_root_id, folder.relative_path),
                )
            })
            .collect();
        let mut by_folder: HashMap<(df_domain::SourceRootId, String), Vec<FolderReview>> =
            HashMap::new();
        for review in folder_reviews {
            let mut seen_scopes = HashSet::new();
            for folder_id in [review.folder_a.as_ref(), review.folder_b.as_ref()]
                .into_iter()
                .flatten()
            {
                let scope = folder_scope.get(folder_id).ok_or_else(|| {
                    DfError::Conflict(format!(
                        "review item `{}` references missing folder `{folder_id}`",
                        review.item_id
                    ))
                })?;
                if seen_scopes.insert(scope.clone()) {
                    by_folder
                        .entry(scope.clone())
                        .or_default()
                        .push(review.clone());
                }
            }
        }

        for occurrence in crate::inventory::list_occurrences(db, snapshot_id)? {
            let mut seen_items = HashSet::new();
            for ancestor in std::path::Path::new(&occurrence.parent_relative_path).ancestors() {
                let key = (occurrence.source_root_id, ancestor.display().to_string());
                for review in by_folder.get(&key).into_iter().flatten() {
                    if seen_items.insert(review.item_id.as_str()) {
                        reviews.push(ReviewRow {
                            item_id: review.item_id.clone(),
                            occurrence_id: occurrence.id.to_string(),
                            risk: review.risk.clone(),
                            decision: review.decision.clone(),
                            rationale: review.rationale.clone(),
                        });
                    }
                }
            }
        }
    }
    #[derive(Debug)]
    struct ReviewAggregate {
        risk: RiskLevel,
        pending_items: Vec<String>,
        decisions: Vec<(String, RuleAction, String)>,
    }
    let mut by_occurrence: HashMap<OccurrenceId, ReviewAggregate> = HashMap::new();
    for review in reviews {
        let occurrence_id = OccurrenceId::from_str(&review.occurrence_id)?;
        let risk = parse_risk(&review.risk)?;
        let aggregate = by_occurrence
            .entry(occurrence_id)
            .or_insert(ReviewAggregate {
                risk: RiskLevel::Low,
                pending_items: Vec::new(),
                decisions: Vec::new(),
            });
        aggregate.risk = max_risk(aggregate.risk, risk);
        match review.decision {
            Some(decision) => aggregate.decisions.push((
                review.item_id,
                RuleAction::parse(&decision)?,
                review
                    .rationale
                    .unwrap_or_else(|| "no rationale".to_string()),
            )),
            None => aggregate.pending_items.push(review.item_id),
        }
    }
    for (occurrence_id, aggregate) in by_occurrence {
        let entry = guidance.entry(occurrence_id).or_insert(OccurrenceGuidance {
            operation_type: OperationType::CopyActive,
            risk: RiskLevel::Low,
            confidence: 1.0,
            reason: "no declarative rule changed the active copy".to_string(),
        });
        entry.risk = max_risk(entry.risk, aggregate.risk);
        if !aggregate.pending_items.is_empty() {
            entry.operation_type = OperationType::CopyReview;
            entry.reason = format!(
                "pending human review item(s): {}",
                aggregate.pending_items.join(", ")
            );
            entry.confidence = 0.0;
            continue;
        }

        let mut actions: Vec<RuleAction> = aggregate
            .decisions
            .iter()
            .map(|(_, action, _)| *action)
            .collect();
        actions.sort_by_key(|action| action.as_str());
        actions.dedup();
        if actions.len() == 1 {
            entry.operation_type = actions[0].operation_type();
            entry.reason = format!(
                "human review decided `{}`: {}",
                actions[0].as_str(),
                aggregate
                    .decisions
                    .iter()
                    .map(|(item, _, rationale)| format!("{item}: {rationale}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            entry.confidence = 1.0;
        } else if !actions.is_empty() {
            // Conflicting human decisions must never be resolved through row
            // or hash order. Keep the occurrence in review until the user
            // appends aligned decisions to the affected items.
            entry.operation_type = OperationType::CopyReview;
            entry.reason = format!(
                "conflicting human review decisions: {}",
                actions
                    .iter()
                    .map(|action| action.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            entry.confidence = 0.0;
        }
    }
    Ok(guidance)
}

/// Append a human decision. Re-deciding appends another row; the latest row
/// wins for future plans while the complete history remains auditable.
pub fn decide_review_item(
    db: &mut Db,
    project_id: ProjectId,
    item_id: &str,
    decision: RuleAction,
    rationale: &str,
    actor: Actor,
) -> DfResult<()> {
    let rationale = rationale.trim();
    if rationale.is_empty() {
        return Err(DfError::Validation(
            "a review decision requires a rationale".to_string(),
        ));
    }
    let belongs: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM review_items ri
                JOIN snapshots s ON s.id = ri.snapshot_id
                WHERE ri.id = ?1 AND s.project_id = ?2
                  AND ri.analysis_version = ?3
             )",
            params![item_id, project_id.to_string(), ANALYSIS_VERSION as i64],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !belongs {
        return Err(DfError::Validation(format!(
            "review item `{item_id}` does not exist in this project"
        )));
    }
    let unmaterializable: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM review_items ri
                JOIN structural_anomalies sa ON sa.id = ri.anomaly_id
                WHERE ri.id = ?1 AND sa.kind = 'UNREADABLE_ENTRY'
             )",
            [item_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if unmaterializable {
        return Err(DfError::Validation(format!(
            "review item `{item_id}` describes unreadable source evidence; copy-bucket decisions cannot materialize it — repair access and rescan"
        )));
    }

    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let sequence: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM review_decisions WHERE review_item_id = ?1",
            [item_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let decision_id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO review_decisions
            (id, review_item_id, sequence, decision, rationale, actor, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            decision_id,
            item_id,
            sequence,
            decision.as_str(),
            rationale,
            actor.as_str(),
            now,
        ],
    )
    .map_err(db_err)?;
    append_event(
        &tx,
        project_id,
        EVENT_REVIEW_DECIDED,
        &serde_json::json!({
            "review_item_id": item_id,
            "decision": decision.as_str(),
            "rationale": rationale,
        }),
        actor,
    )?;
    tx.commit().map_err(db_err)
}

pub fn review_queue(db: &Db, snapshot_id: SnapshotId) -> DfResult<ReviewQueue> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT ri.id, ri.occurrence_id, sa.folder_a, sa.folder_b,
                     CASE WHEN ri.anomaly_id IS NOT NULL THEN 'ANOMALY' ELSE 'RULE' END,
                     COALESCE(sa.kind, rm.rule_id), ri.risk,
                     ri.recommended_action, ri.reason,
                    (SELECT rd.decision FROM review_decisions rd
                     WHERE rd.review_item_id = ri.id
                     ORDER BY rd.sequence DESC LIMIT 1),
                    (SELECT rd.rationale FROM review_decisions rd
                     WHERE rd.review_item_id = ri.id
                     ORDER BY rd.sequence DESC LIMIT 1),
                    sa.evidence_json
             FROM review_items ri
             LEFT JOIN structural_anomalies sa ON sa.id = ri.anomaly_id
             LEFT JOIN rule_matches rm ON rm.id = ri.rule_match_id
             WHERE ri.snapshot_id = ?1 AND ri.analysis_version = ?2
             ORDER BY CASE ri.risk WHEN 'HIGH' THEN 0 WHEN 'MEDIUM' THEN 1 ELSE 2 END,
                      ri.id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |row| {
                let decision: Option<String> = row.get(9)?;
                let kind: String = row.get(5)?;
                let materializable = kind != AnomalyKind::UnreadableEntry.as_str();
                Ok(ReviewItemView {
                    id: row.get(0)?,
                    snapshot_id: snapshot_id.to_string(),
                    occurrence_id: row.get(1)?,
                    folder_a: row.get(2)?,
                    folder_b: row.get(3)?,
                    source: row.get(4)?,
                    kind,
                    risk: row.get(6)?,
                    recommended_action: row.get(7)?,
                    materializable,
                    reason: row.get(8)?,
                    status: if !materializable {
                        "SOURCE_BLOCKED".to_string()
                    } else if decision.is_some() {
                        "DECIDED".to_string()
                    } else {
                        "PENDING".to_string()
                    },
                    decision,
                    rationale: row.get(10)?,
                    evidence: row
                        .get::<_, Option<String>>(11)?
                        .map(|stored| serde_json::from_str(&stored))
                        .transpose()
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                11,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                })
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    let pending = rows.iter().filter(|item| item.decision.is_none()).count() as u64;
    Ok(ReviewQueue {
        snapshot_id: snapshot_id.to_string(),
        pending,
        decided: rows.len() as u64 - pending,
        items: rows,
    })
}

pub fn anomaly_report(db: &Db, snapshot_id: SnapshotId) -> DfResult<AnomalyReport> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, kind, severity, requires_review, summary, occurrence_id,
                    folder_a, folder_b, evidence_json
             FROM structural_anomalies
             WHERE snapshot_id = ?1 AND analysis_version = ?2
             ORDER BY CASE severity WHEN 'HIGH' THEN 0 WHEN 'WARNING' THEN 1 ELSE 2 END,
                      kind, id",
        )
        .map_err(db_err)?;
    let anomalies = stmt
        .query_map(
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .map_err(db_err)?
        .map(|row| {
            let (id, kind, severity, requires_review, summary, occurrence, a, b, evidence) =
                row.map_err(db_err)?;
            Ok(AnomalyView {
                id,
                kind,
                severity,
                requires_review,
                summary,
                occurrence_id: occurrence,
                folder_a: a,
                folder_b: b,
                evidence: serde_json::from_str(&evidence).map_err(|error| {
                    DfError::Serialization(format!("stored anomaly evidence: {error}"))
                })?,
            })
        })
        .collect::<DfResult<Vec<_>>>()?;
    let count = |severity: &str| {
        anomalies
            .iter()
            .filter(|anomaly| anomaly.severity == severity)
            .count() as u64
    };
    Ok(AnomalyReport {
        snapshot_id: snapshot_id.to_string(),
        high: count("HIGH"),
        warnings: count("WARNING"),
        information: count("INFO"),
        anomalies,
    })
}

/// Final append-only marker used to distinguish a valid empty report from an
/// interrupted analysis that happened not to persist a given stage.
pub fn complete_analysis(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile_id: &str,
    summary: &impl Serialize,
    actor: Actor,
) -> DfResult<()> {
    let (profile, profile_sha256) = load_analysis_profile(db, project_id, snapshot_id, profile_id)?;
    let summary_value = serde_json::to_value(summary)
        .map_err(|error| DfError::Serialization(format!("analysis summary: {error}")))?;
    let summary_json = canonical_json(&summary_value);
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    let inserted = tx
        .execute(
            "INSERT INTO analysis_completions
                (snapshot_id, project_id, analysis_version, profile_id,
                 profile_sha256, summary_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(snapshot_id, analysis_version) DO NOTHING",
            params![
                snapshot_id.to_string(),
                project_id.to_string(),
                ANALYSIS_VERSION as i64,
                profile.id,
                profile_sha256,
                &summary_json,
                now,
            ],
        )
        .map_err(db_err)?;
    if inserted > 0 {
        append_event(
            &tx,
            project_id,
            EVENT_STRUCTURAL_ANALYSIS_COMPLETED,
            &serde_json::json!({
                "snapshot_id": snapshot_id.to_string(),
                "analysis_version": ANALYSIS_VERSION,
                "profile_id": profile.id,
                "profile_sha256": profile_sha256,
                "summary": summary_value,
            }),
            actor,
        )?;
    } else {
        let (stored_project, stored_version, stored_profile, stored_digest, stored_summary): (
            String,
            i64,
            String,
            String,
            String,
        ) = tx
            .query_row(
                "SELECT project_id, analysis_version, profile_id, profile_sha256,
                        summary_json
                 FROM analysis_completions
                 WHERE snapshot_id = ?1 AND analysis_version = ?2",
                params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(db_err)?;
        if stored_project != project_id.to_string()
            || stored_version != ANALYSIS_VERSION as i64
            || stored_profile != profile.id
            || stored_digest != profile_sha256
            || stored_summary != summary_json
        {
            return Err(DfError::Conflict(format!(
                "snapshot `{snapshot_id}` already has a different completed structural analysis"
            )));
        }
    }
    tx.commit().map_err(db_err)
}

pub fn is_analysis_complete(db: &Db, snapshot_id: SnapshotId) -> DfResult<bool> {
    db.conn()
        .query_row(
            "SELECT 1 FROM analysis_completions
             WHERE snapshot_id = ?1 AND analysis_version = ?2",
            params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(db_err)
}

pub fn require_analysis_complete(db: &Db, snapshot_id: SnapshotId) -> DfResult<()> {
    if is_analysis_complete(db, snapshot_id)? {
        Ok(())
    } else {
        Err(DfError::Validation(format!(
            "structural analysis for snapshot `{snapshot_id}` has not completed; \
             run `dataforge analyze`"
        )))
    }
}

pub fn diagnostics(db: &Db, snapshot_id: SnapshotId) -> DfResult<StructuralDiagnostics> {
    let scalar = |sql: &str| -> DfResult<u64> {
        let value: i64 = db
            .conn()
            .query_row(sql, [snapshot_id.to_string()], |row| row.get(0))
            .map_err(db_err)?;
        Ok(value as u64)
    };
    let scalar_versioned = |sql: &str| -> DfResult<u64> {
        let value: i64 = db
            .conn()
            .query_row(
                sql,
                params![snapshot_id.to_string(), ANALYSIS_VERSION as i64],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(value as u64)
    };
    Ok(StructuralDiagnostics {
        analysis_complete: is_analysis_complete(db, snapshot_id)?,
        folder_signatures: scalar("SELECT COUNT(*) FROM folder_signatures WHERE snapshot_id = ?1")?,
        exact_tree_clone_sets: scalar(
            "SELECT COUNT(*) FROM tree_clone_sets WHERE snapshot_id = ?1",
        )?,
        partial_tree_clones: scalar(
            "SELECT COUNT(*) FROM tree_relations
             WHERE snapshot_id = ?1 AND relationship = 'PARTIAL_TREE_CLONE'",
        )?,
        embedded_trees: scalar(
            "SELECT COUNT(*) FROM tree_relations
             WHERE snapshot_id = ?1 AND relationship = 'TREE_EMBEDDED'",
        )?,
        repeated_components: scalar(
            "SELECT COUNT(*) FROM tree_relations
             WHERE snapshot_id = ?1 AND relationship = 'REPEATED_COMPONENT_ONLY'",
        )?,
        candidate_cap_reached: relation_candidate_cap_reached(db, snapshot_id)?.unwrap_or(false),
        generic_folders: scalar(
            "SELECT COUNT(*) FROM folder_contexts
             WHERE snapshot_id = ?1 AND kind = 'GENERIC'",
        )?,
        protected_boundaries: scalar(
            "SELECT COUNT(*) FROM folder_contexts
             WHERE snapshot_id = ?1 AND is_protected_boundary = 1",
        )?,
        rule_matches: count_where(db, "rule_matches", snapshot_id, None)?,
        anomalies: count_where(db, "structural_anomalies", snapshot_id, None)?,
        high_anomalies: count_where(
            db,
            "structural_anomalies",
            snapshot_id,
            Some(("severity", "HIGH")),
        )?,
        pending_review: scalar_versioned(
            "SELECT COUNT(*) FROM review_items ri
             WHERE ri.snapshot_id = ?1
               AND ri.analysis_version = ?2
               AND NOT EXISTS (
                   SELECT 1 FROM review_decisions rd WHERE rd.review_item_id = ri.id
               )",
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use df_domain::{ProfileRef, Project, SourceRoot};

    fn seed_rule_occurrence(db: &mut Db, file_name: &str) -> (ProjectId, SnapshotId, String) {
        let project = Project::new(
            "analysis-test",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "test",
        );
        let root = SourceRoot::new(project.id, PathBuf::from("D:/in"));
        let root_id = root.id;
        crate::repository::create_project(db, &project, &[root], Actor::Test).unwrap();
        let (snapshot, run) = crate::inventory::start_scan(db, project.id, Actor::Test).unwrap();
        let occurrence_id = OccurrenceId::new().to_string();
        db.conn()
            .execute(
                "INSERT INTO path_occurrences
                    (id, snapshot_id, source_root_id, relative_path,
                     parent_relative_path, file_name, normalized_name, extension,
                     size_bytes, attributes, path_length, depth, fingerprint,
                     scan_status, name_is_lossy, created_at)
                 VALUES (?1, ?2, ?3, ?4, '', ?4, ?5, NULL,
                         1, 0, ?6, 1, 'v1:1:0', 'OK', 0, 't')",
                params![
                    occurrence_id,
                    snapshot.id.to_string(),
                    root_id.to_string(),
                    file_name,
                    file_name.to_lowercase(),
                    file_name.encode_utf16().count() as i64,
                ],
            )
            .unwrap();
        crate::inventory::finish_scan(
            db,
            &run,
            df_domain::ScanRunStatus::Completed,
            df_domain::ScanCounters {
                files: 1,
                ..df_domain::ScanCounters::default()
            },
            Actor::Test,
        )
        .unwrap();
        (project.id, snapshot.id, occurrence_id)
    }

    #[test]
    fn stable_ids_are_unambiguous_and_repeatable() {
        assert_eq!(stable_id("x", &["ab", "c"]), stable_id("x", &["ab", "c"]));
        assert_ne!(stable_id("x", &["ab", "c"]), stable_id("x", &["a", "bc"]));
        assert_ne!(stable_id("x", &["a"]), stable_id("y", &["a"]));
    }

    #[test]
    fn rule_evaluation_is_idempotent_and_keeps_one_evidence_row() {
        let mut db = Db::open_in_memory().unwrap();
        let (project, snapshot, _) = seed_rule_occurrence(&mut db, "~$Contrato.docx");
        let first = evaluate_rules(&mut db, project, snapshot, "generic", Actor::Test).unwrap();
        let second = evaluate_rules(&mut db, project, snapshot, "generic", Actor::Test).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.matches, 1);
        assert_eq!(first.review_items, 0);
    }

    #[test]
    fn analysis_rejects_a_profile_different_from_the_projects_profile() {
        let mut db = Db::open_in_memory().unwrap();
        let (project, snapshot, _) = seed_rule_occurrence(&mut db, "~$Contrato.docx");
        let error = evaluate_rules(&mut db, project, snapshot, "legal", Actor::Test).unwrap_err();
        assert!(matches!(error, DfError::Conflict(_)), "{error}");
        assert_eq!(count_where(&db, "rule_matches", snapshot, None).unwrap(), 0);
    }

    #[test]
    fn completion_seals_automatic_evidence_but_idempotent_replay_still_works() {
        let mut db = Db::open_in_memory().unwrap();
        let (project, snapshot, occurrence) = seed_rule_occurrence(&mut db, "~$Contrato.docx");
        let rules = evaluate_rules(&mut db, project, snapshot, "generic", Actor::Test).unwrap();
        let anomalies = detect_anomalies(&mut db, project, snapshot, Actor::Test).unwrap();
        let summary = serde_json::json!({
            "rule_matches": rules.matches,
            "anomalies": anomalies.anomalies,
            "review_items": anomalies.review_items,
        });
        complete_analysis(&mut db, project, snapshot, "generic", &summary, Actor::Test).unwrap();

        let replay = evaluate_rules(&mut db, project, snapshot, "generic", Actor::Test).unwrap();
        assert_eq!(replay, rules);
        let profile = Profile::load("generic").unwrap();
        let profile_sha256 = resolved_profile_sha256(&profile).unwrap();
        let error = db
            .conn()
            .execute(
                "INSERT INTO rule_matches
                    (id, snapshot_id, occurrence_id, analysis_version,
                     profile_id, profile_sha256, rule_id, rule_version,
                     priority, is_selected, category, action, confidence, risk,
                     evidence_json, created_at)
                 VALUES ('late-rule', ?1, ?2, ?3, 'generic', ?4,
                         'late.rule', 1, 99, 0, 'test', 'COPY_ACTIVE', 1.0,
                         'LOW', '{}', 't')",
                params![
                    snapshot.to_string(),
                    occurrence,
                    ANALYSIS_VERSION as i64,
                    profile_sha256,
                ],
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("completed rule evidence is sealed"));
    }

    #[test]
    fn completion_and_evidence_are_selected_by_analysis_version() {
        let mut db = Db::open_in_memory().unwrap();
        let (project, snapshot, _) = seed_rule_occurrence(&mut db, "~$Contrato.docx");
        let profile = Profile::load("generic").unwrap();
        let profile_sha256 = resolved_profile_sha256(&profile).unwrap();
        db.conn()
            .execute(
                "INSERT INTO analysis_completions
                    (snapshot_id, project_id, analysis_version, profile_id,
                     profile_sha256, summary_json, created_at)
                 VALUES (?1, ?2, ?3, 'generic', ?4, '{}', 't')",
                params![
                    snapshot.to_string(),
                    project.to_string(),
                    ANALYSIS_VERSION as i64 + 1,
                    profile_sha256,
                ],
            )
            .unwrap();
        assert!(!is_analysis_complete(&db, snapshot).unwrap());

        let rules = evaluate_rules(&mut db, project, snapshot, "generic", Actor::Test).unwrap();
        complete_analysis(
            &mut db,
            project,
            snapshot,
            "generic",
            &serde_json::json!({ "rule_matches": rules.matches }),
            Actor::Test,
        )
        .unwrap();
        assert!(is_analysis_complete(&db, snapshot).unwrap());
        let completions: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM analysis_completions WHERE snapshot_id = ?1",
                [snapshot.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(completions, 2);
    }

    #[test]
    fn rule_idempotency_rejects_conflicting_stored_evidence() {
        let mut db = Db::open_in_memory().unwrap();
        let (project, snapshot, occurrence) = seed_rule_occurrence(&mut db, "~$Contrato.docx");
        let id = stable_id(
            "rule-match",
            &[
                &snapshot.to_string(),
                &occurrence,
                "temporary.office-lock",
                "1",
            ],
        );
        let profile = Profile::load("generic").unwrap();
        let profile_sha256 = resolved_profile_sha256(&profile).unwrap();
        db.conn()
            .execute(
                "INSERT INTO rule_matches
                    (id, snapshot_id, occurrence_id, analysis_version,
                     profile_id, profile_sha256, rule_id, rule_version,
                     priority, is_selected, category, action, confidence, risk,
                     evidence_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'generic', ?5,
                         'temporary.office-lock', 1, 0, 1,
                         'temporary', 'COPY_ACTIVE', 1.0, 'LOW', '{}', 't')",
                params![
                    id,
                    snapshot.to_string(),
                    occurrence,
                    ANALYSIS_VERSION as i64,
                    profile_sha256,
                ],
            )
            .unwrap();

        let error = evaluate_rules(&mut db, project, snapshot, "generic", Actor::Test).unwrap_err();
        assert!(matches!(error, DfError::Conflict(_)), "{error}");
    }

    #[test]
    fn review_items_must_match_their_sources_analysis_version() {
        let mut db = Db::open_in_memory().unwrap();
        let (_, snapshot, occurrence) = seed_rule_occurrence(&mut db, "documento.txt");
        let profile = Profile::load("generic").unwrap();
        let profile_sha256 = resolved_profile_sha256(&profile).unwrap();
        db.conn()
            .execute(
                "INSERT INTO rule_matches
                    (id, snapshot_id, occurrence_id, analysis_version,
                     profile_id, profile_sha256, rule_id, rule_version,
                     priority, is_selected, category, action, confidence, risk,
                     evidence_json, created_at)
                 VALUES ('rule-evidence', ?1, ?2, ?3, 'generic', ?4,
                         'test.rule', 1, 0, 1,
                         'test', 'COPY_REVIEW', 1.0, 'MEDIUM', '{}', 't')",
                params![
                    snapshot.to_string(),
                    occurrence,
                    ANALYSIS_VERSION as i64,
                    profile_sha256,
                ],
            )
            .unwrap();

        let error = db
            .conn()
            .execute(
                "INSERT INTO review_items
                    (id, snapshot_id, analysis_version, anomaly_id,
                     rule_match_id, occurrence_id, recommended_action, risk,
                     reason, created_at)
                 VALUES ('bad-review', ?1, ?2, NULL, 'rule-evidence', ?3,
                         'COPY_REVIEW', 'MEDIUM', 'test', 't')",
                params![
                    snapshot.to_string(),
                    ANALYSIS_VERSION as i64 + 1,
                    occurrence,
                ],
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("review source ownership or version does not match"),
            "{error}"
        );
    }

    #[test]
    fn unreadable_evidence_cannot_be_falsely_decided_into_a_copy_bucket() {
        let mut db = Db::open_in_memory().unwrap();
        let (project, snapshot, occurrence) = seed_rule_occurrence(&mut db, "inaccesible.txt");
        db.conn()
            .execute(
                "INSERT INTO structural_anomalies
                    (id, snapshot_id, analysis_version, occurrence_id, folder_a,
                     folder_b, kind, severity, requires_review, summary,
                     evidence_json, created_at)
                 VALUES ('unreadable-anomaly', ?1, ?2, ?3, NULL, NULL,
                         'UNREADABLE_ENTRY', 'HIGH', 1, 'unreadable', '{}', 't')",
                params![snapshot.to_string(), ANALYSIS_VERSION as i64, occurrence,],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO review_items
                    (id, snapshot_id, analysis_version, anomaly_id,
                     rule_match_id, occurrence_id, recommended_action, risk,
                     reason, created_at)
                 VALUES ('unreadable-review', ?1, ?2, 'unreadable-anomaly',
                         NULL, ?3, 'COPY_REVIEW', 'HIGH', 'unreadable', 't')",
                params![snapshot.to_string(), ANALYSIS_VERSION as i64, occurrence,],
            )
            .unwrap();

        let queue = review_queue(&db, snapshot).unwrap();
        assert_eq!(queue.items[0].status, "SOURCE_BLOCKED");
        assert!(!queue.items[0].materializable);
        let error = decide_review_item(
            &mut db,
            project,
            "unreadable-review",
            RuleAction::CopyActive,
            "intentarlo",
            Actor::Test,
        )
        .unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("repair access and rescan")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn structural_review_tables_are_append_only() {
        let db = Db::open_in_memory().unwrap();
        for table in [
            "rule_matches",
            "structural_anomalies",
            "review_items",
            "review_decisions",
            "analysis_completions",
        ] {
            let trigger_count: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'trigger' AND tbl_name = ?1
                       AND (name LIKE '%_no_update' OR name LIKE '%_no_delete')",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(trigger_count, 2, "{table} must reject update and delete");
        }
    }
}
