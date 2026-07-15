//! Duplicate policy: the logical representative of a duplicate set
//! (RFC-0001 §15.5).
//!
//! Within a set of exact duplicates, every occurrence is scored by how good a
//! *canonical* location it is, and the best one is recorded as the
//! representative with a human-readable reason (§5.3).
//!
//! This is evidence, never an action. §15.5 is explicit: "el representante
//! lógico no implica borrar otras apariciones", and rule 8 forbids treating a
//! duplicate as automatically dispensable. Nothing here marks a file for
//! removal or feeds the plan.
//!
//! Deterministic signals implemented (of the §15.5 list):
//! - `- Descargas / Escritorio / Backup / Copia / temporal`: the location
//!   penalty of the occurrence's ancestor folders, from `folder_contexts`
//!   (§18.3);
//! - `+ nombre limpio`: a filename carrying a copy marker is penalised;
//! - `+ ruta canónica`: shallower paths win.
//!
//! Deferred (they need signals that do not exist yet): `+ contexto
//! específico` and `+ fecha coherente` need the entity graph (§18.2); `+
//! menor anomalía` and `- ruta injertada` need the anomaly detector and the
//! grafted-tree relationships.

use std::collections::HashMap;

use df_domain::{Actor, ContextKind, ProjectId, SnapshotId};
use df_error::DfResult;
use rusqlite::params;

use crate::repository::{append_event, to_stored_timestamp};
use crate::{db_err, Db};

/// Audit event emitted when representatives have been scored.
pub const EVENT_DUPLICATE_REPRESENTATIVES: &str = "DUPLICATE_REPRESENTATIVES_SCORED";

/// Weight of one unit of location penalty (§18.3) in the cost.
const LOCATION_WEIGHT: i64 = 100;
/// Cost added when the file name carries a copy marker.
const COPY_MARKER_COST: i64 = 10;

/// Does this normalized file name look like a copy? (`+ nombre limpio`.)
fn has_copy_marker(normalized_name: &str) -> bool {
    let stem = normalized_name
        .rsplit_once('.')
        .map(|(stem, _ext)| stem)
        .unwrap_or(normalized_name);
    stem.ends_with(" - copia")
        || stem.ends_with(" - copy")
        || stem.starts_with("copia de ")
        || stem.starts_with("copy of ")
        // Windows' "name (1).ext" duplicate suffix.
        || (stem.ends_with(')')
            && stem
                .rsplit_once(" (")
                .is_some_and(|(_, n)| {
                    let n = n.trim_end_matches(')');
                    !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
                }))
}

/// Every ancestor folder path of a file, from its parent up to the root.
///
/// `"a/b"` yields `["a/b", "a", ""]` so the root's own penalty also applies.
fn ancestor_paths(parent_relative_path: &str) -> Vec<String> {
    let mut out = vec![parent_relative_path.to_string()];
    let mut current = parent_relative_path;
    while let Some((head, _)) = current.rsplit_once(['/', '\\']) {
        out.push(head.to_string());
        current = head;
    }
    if !out.iter().any(|p| p.is_empty()) {
        out.push(String::new());
    }
    out
}

/// Cost of an occurrence as a canonical location; lower is better.
fn location_cost(location_penalty: u32, copy_marker: bool, depth: i64) -> i64 {
    location_penalty as i64 * LOCATION_WEIGHT
        + if copy_marker { COPY_MARKER_COST } else { 0 }
        + depth
}

fn reason_for(
    location_penalty: u32,
    marker: Option<&str>,
    copy_marker: bool,
    depth: i64,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    match marker {
        Some(m) if location_penalty > 0 => parts.push(format!(
            "inside a generic `{m}` folder (penalty {location_penalty})"
        )),
        _ => parts.push("outside any generic folder".to_string()),
    }
    if copy_marker {
        parts.push("file name carries a copy marker".to_string());
    } else {
        parts.push("clean file name".to_string());
    }
    parts.push(format!("path depth {depth}"));
    parts.join("; ")
}

struct Candidate {
    occurrence_id: String,
    absolute_path: String,
    cost: i64,
    reason: String,
}

/// Score every exact-duplicate set of a snapshot and record its logical
/// representative. Idempotent: rows for the snapshot are replaced.
///
/// Requires `folder_contexts` to be populated (context classification runs
/// first in `analyze`); with no context rows every location penalty is 0 and
/// the decision falls back to name cleanliness and path depth.
pub fn score_duplicate_representatives(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    actor: Actor,
) -> DfResult<u64> {
    let snapshot = snapshot_id.to_string();

    // (source_root_id, folder_relative_path) -> (penalty, marker)
    let penalties: HashMap<(String, String), (u32, Option<String>)> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT f.source_root_id, fc.relative_path, fc.penalty, fc.marker
                 FROM folder_contexts fc
                 JOIN folders f ON f.id = fc.folder_id
                 WHERE fc.snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    (
                        row.get::<_, i64>(2)? as u32,
                        row.get::<_, Option<String>>(3)?,
                    ),
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows.into_iter().collect()
    };

    // duplicate set -> its occurrences
    struct Row {
        set_id: String,
        occurrence_id: String,
        source_root_id: String,
        root_path: String,
        relative_path: String,
        parent_relative_path: String,
        normalized_name: String,
    }
    let rows: Vec<Row> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT ds.id, o.id, o.source_root_id, r.absolute_path,
                        o.relative_path, o.parent_relative_path, o.normalized_name
                 FROM duplicate_sets ds
                 JOIN occurrence_content oc ON oc.content_id = ds.content_id
                 JOIN path_occurrences o ON o.id = oc.occurrence_id
                 JOIN source_roots r ON r.id = o.source_root_id
                 WHERE ds.snapshot_id = ?1 AND o.snapshot_id = ?1
                 ORDER BY ds.id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok(Row {
                    set_id: row.get(0)?,
                    occurrence_id: row.get(1)?,
                    source_root_id: row.get(2)?,
                    root_path: row.get(3)?,
                    relative_path: row.get(4)?,
                    parent_relative_path: row.get(5)?,
                    normalized_name: row.get(6)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    let mut by_set: HashMap<String, Vec<Candidate>> = HashMap::new();
    for row in &rows {
        // Worst penalty among the file's ancestor folders (§18.3).
        let mut penalty = 0u32;
        let mut marker: Option<String> = None;
        for ancestor in ancestor_paths(&row.parent_relative_path) {
            if let Some((p, m)) = penalties.get(&(row.source_root_id.clone(), ancestor)) {
                if *p > penalty {
                    penalty = *p;
                    marker = m.clone();
                }
            }
        }
        let copy_marker = has_copy_marker(&row.normalized_name);
        let depth = row.relative_path.matches(['/', '\\']).count() as i64;
        let absolute_path = if row.relative_path.is_empty() {
            row.root_path.clone()
        } else {
            format!(
                "{}{}{}",
                row.root_path,
                std::path::MAIN_SEPARATOR,
                row.relative_path
            )
        };
        by_set
            .entry(row.set_id.clone())
            .or_default()
            .push(Candidate {
                occurrence_id: row.occurrence_id.clone(),
                absolute_path,
                cost: location_cost(penalty, copy_marker, depth),
                reason: reason_for(penalty, marker.as_deref(), copy_marker, depth),
            });
    }

    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "DELETE FROM duplicate_representatives WHERE snapshot_id = ?1",
        [&snapshot],
    )
    .map_err(db_err)?;

    let mut scored: u64 = 0;
    for (set_id, mut candidates) in by_set {
        // Lowest cost wins; ties break on the path so the choice is stable.
        candidates.sort_by(|a, b| {
            a.cost
                .cmp(&b.cost)
                .then_with(|| a.absolute_path.cmp(&b.absolute_path))
        });
        let best = &candidates[0];
        tx.execute(
            "INSERT INTO duplicate_representatives
                (duplicate_set_id, snapshot_id, occurrence_id, score, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                set_id,
                snapshot,
                best.occurrence_id,
                -best.cost,
                best.reason,
                now,
            ],
        )
        .map_err(db_err)?;
        scored += 1;
    }

    let payload = serde_json::json!({
        "snapshot_id": snapshot,
        "duplicate_representatives": scored,
    });
    append_event(
        &tx,
        project_id,
        EVENT_DUPLICATE_REPRESENTATIVES,
        &payload,
        actor,
    )?;
    tx.commit().map_err(db_err)?;
    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_markers_are_detected() {
        assert!(has_copy_marker("informe - copia.pdf"));
        assert!(has_copy_marker("informe - copy.pdf"));
        assert!(has_copy_marker("copia de informe.pdf"));
        assert!(has_copy_marker("informe (1).pdf"));
        assert!(has_copy_marker("informe (12).pdf"));
        assert!(!has_copy_marker("informe.pdf"));
        assert!(!has_copy_marker("contrato final.pdf"));
        // Parenthesised text that is not a number is a legitimate name.
        assert!(!has_copy_marker("informe (final).pdf"));
    }

    #[test]
    fn ancestors_walk_up_to_the_root() {
        assert_eq!(ancestor_paths("a/b"), vec!["a/b", "a", ""]);
        assert_eq!(ancestor_paths("a"), vec!["a", ""]);
        assert_eq!(ancestor_paths(""), vec![""]);
        assert_eq!(ancestor_paths("a\\b\\c"), vec!["a\\b\\c", "a\\b", "a", ""]);
    }

    #[test]
    fn a_generic_location_costs_more_than_a_clean_deep_path() {
        // Inside Descargas (50) at depth 1 vs. a clean path at depth 5.
        let generic = location_cost(50, false, 1);
        let clean_deep = location_cost(0, false, 5);
        assert!(clean_deep < generic, "location dominates depth");
        // A copy marker breaks a tie between equally-located files.
        assert!(location_cost(0, false, 2) < location_cost(0, true, 2));
    }
}

/// One occurrence of a duplicated content, with everything the planner needs
/// to apply a [`df_domain::DuplicatePolicy`] to it.
#[derive(Debug, Clone)]
pub struct DuplicateMember {
    pub duplicate_set_id: String,
    pub occurrence_id: String,
    /// This occurrence is the set's logical representative (§15.5).
    pub is_representative: bool,
    /// Most restrictive context found along the occurrence's ancestor chain:
    /// `Protected` wins over `Generic`, which wins over `Neutral`. A file
    /// inside a protected expediente is protected even if it also sits under
    /// a folder named "Copia".
    pub context: ContextKind,
    /// The marker that classified the context, for the operation's reason.
    pub context_marker: Option<String>,
}

/// Every occurrence that belongs to some exact-duplicate set of a snapshot,
/// resolved against the folder contexts and the recorded representatives.
///
/// Occurrences that are not duplicated at all are not returned: the planner
/// copies them unconditionally.
pub fn duplicate_members(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<DuplicateMember>> {
    let snapshot = snapshot_id.to_string();

    // (source_root_id, folder_relative_path) -> (kind, marker)
    let contexts: HashMap<(String, String), (ContextKind, Option<String>)> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT f.source_root_id, fc.relative_path, fc.kind, fc.marker
                 FROM folder_contexts fc
                 JOIN folders f ON f.id = fc.folder_id
                 WHERE fc.snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    (row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?),
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows.into_iter()
            .map(|(key, (kind, marker))| Ok((key, (ContextKind::parse(&kind)?, marker))))
            .collect::<DfResult<HashMap<_, _>>>()?
    };

    let mut stmt = db
        .conn()
        .prepare(
            "SELECT ds.id, o.id, o.source_root_id, o.parent_relative_path,
                    CASE WHEN dr.occurrence_id = o.id THEN 1 ELSE 0 END
             FROM duplicate_sets ds
             JOIN occurrence_content oc ON oc.content_id = ds.content_id
             JOIN path_occurrences o ON o.id = oc.occurrence_id
             LEFT JOIN duplicate_representatives dr ON dr.duplicate_set_id = ds.id
             WHERE ds.snapshot_id = ?1 AND o.snapshot_id = ?1
             ORDER BY ds.id, o.relative_path",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([&snapshot], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? != 0,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(
            |(set_id, occurrence_id, root_id, parent, is_representative)| {
                // Walk the ancestor chain and keep the most restrictive context.
                let mut context = ContextKind::Neutral;
                let mut marker = None;
                for ancestor in ancestor_paths(&parent) {
                    if let Some((kind, found)) = contexts.get(&(root_id.clone(), ancestor)) {
                        let wins = matches!(
                            (context, kind),
                            (_, ContextKind::Protected)
                                | (ContextKind::Neutral, ContextKind::Generic)
                        );
                        if wins {
                            context = *kind;
                            marker.clone_from(found);
                        }
                    }
                }
                DuplicateMember {
                    duplicate_set_id: set_id,
                    occurrence_id,
                    is_representative,
                    context,
                    context_marker: marker,
                }
            },
        )
        .collect())
}
