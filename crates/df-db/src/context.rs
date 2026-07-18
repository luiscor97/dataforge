//! Folder context classification (RFC-0001 §18.3).
//!
//! Deterministic, profile-driven: a folder whose name matches a *generic*
//! marker (Descargas, Escritorio, Backup, Recuperado, Copia, Temporales) is
//! tagged `GENERIC` with the penalty weight of §18.3; a folder whose name
//! matches a profile's *protected* marker is a `PROTECTED` boundary that
//! deduplication must never dissolve (rule 9); everything else is `NEUTRAL`.
//!
//! The markers are not hardcoded here: they come from the declarative profile
//! in `profiles/<id>/profile.json` (ADR-0026), so what counts as a boundary is
//! reviewable data, not code. The `generic` profile declares no protected
//! markers (§25.4); `legal` declares expedientes and periciales. Entity
//! anchors and weighted propagation (§18.2–§18.4) are a later slice.

use df_domain::{Actor, ContextKind, ProjectId, SnapshotId};
use df_error::DfResult;
use rusqlite::params;

use crate::repository::{append_event, to_stored_timestamp};
use crate::{db_err, Db};

/// Audit event emitted when context classification finishes.
pub const EVENT_CONTEXTS_CLASSIFIED: &str = "CONTEXTS_CLASSIFIED";

/// Counts returned by [`classify_folders`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ContextSummary {
    pub generic_folders: u64,
    pub protected_boundaries: u64,
    pub neutral_folders: u64,
}

/// Classify every folder of a snapshot under the given profile and persist
/// the result. Idempotent: rows for the snapshot are replaced.
///
/// `profile` selects the marker set from `profiles/<id>/profile.json`; an
/// unknown id is rejected rather than silently losing domain protections.
pub fn classify_folders(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile: &str,
    actor: Actor,
) -> DfResult<ContextSummary> {
    // The profile is data (ADR-0026): typos must fail closed rather than
    // silently selecting a different marker set.
    let profile = df_domain::Profile::load(profile)?;

    let snapshot = snapshot_id.to_string();
    let folders: Vec<(String, String, String)> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT id, relative_path, normalized_name
                 FROM folders WHERE snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "DELETE FROM folder_contexts WHERE snapshot_id = ?1",
        [&snapshot],
    )
    .map_err(db_err)?;

    let mut summary = ContextSummary {
        generic_folders: 0,
        protected_boundaries: 0,
        neutral_folders: 0,
    };
    for (folder_id, relative_path, normalized_name) in &folders {
        let (kind, penalty, marker, reason) = match profile.classify(normalized_name) {
            (ContextKind::Protected, _, marker) => {
                summary.protected_boundaries += 1;
                let reason = marker
                    .as_deref()
                    .and_then(|name| profile.protected_reason(name))
                    .map(str::to_owned);
                (ContextKind::Protected, 0u32, marker, reason)
            }
            (ContextKind::Generic, penalty, marker) => {
                summary.generic_folders += 1;
                (ContextKind::Generic, penalty, marker, None)
            }
            (ContextKind::Neutral, _, _) => {
                summary.neutral_folders += 1;
                (ContextKind::Neutral, 0u32, None, None)
            }
        };
        tx.execute(
            "INSERT INTO folder_contexts
                (folder_id, snapshot_id, relative_path, kind,
                 is_protected_boundary, penalty, marker, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                folder_id,
                snapshot,
                relative_path,
                kind.as_str(),
                // Derived from the kind, never hardcoded: a PROTECTED folder
                // whose flag said 0 would be a silent security false negative.
                (kind == ContextKind::Protected) as i64,
                penalty as i64,
                marker,
                reason,
                now,
            ],
        )
        .map_err(db_err)?;
    }

    let payload = serde_json::json!({
        "snapshot_id": snapshot,
        "generic_folders": summary.generic_folders,
        "protected_boundaries": summary.protected_boundaries,
        "neutral_folders": summary.neutral_folders,
    });
    append_event(&tx, project_id, EVENT_CONTEXTS_CLASSIFIED, &payload, actor)?;
    tx.commit().map_err(db_err)?;

    Ok(summary)
}

/// A folder flagged as generic, with its absolute path and penalty.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GenericFolder {
    pub path: String,
    pub marker: Option<String>,
    pub penalty: u32,
}

/// A protected domain boundary with the exact profile evidence that created
/// it. These rows explain why duplicate consolidation stops at the folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProtectedFolder {
    pub path: String,
    pub marker: String,
    pub reason: String,
}

/// Read the generic folders of a snapshot, worst (highest penalty) first.
pub fn generic_folders(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<GenericFolder>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT r.absolute_path, fc.relative_path, fc.marker, fc.penalty
             FROM folder_contexts fc
             JOIN folders f ON f.id = fc.folder_id
             JOIN source_roots r ON r.id = f.source_root_id
             WHERE fc.snapshot_id = ?1 AND fc.kind = 'GENERIC'
             ORDER BY fc.penalty DESC, r.absolute_path, fc.relative_path",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            let root: String = row.get(0)?;
            let relative: String = row.get(1)?;
            let marker: Option<String> = row.get(2)?;
            let penalty: i64 = row.get(3)?;
            let path = if relative.is_empty() {
                root
            } else {
                format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
            };
            Ok(GenericFolder {
                path,
                marker,
                penalty: penalty as u32,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

pub fn protected_folders(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<ProtectedFolder>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT r.absolute_path, fc.relative_path, fc.marker, fc.reason
             FROM folder_contexts fc
             JOIN folders f ON f.id = fc.folder_id
             JOIN source_roots r ON r.id = f.source_root_id
             WHERE fc.snapshot_id = ?1 AND fc.kind = 'PROTECTED'
             ORDER BY r.absolute_path, fc.relative_path",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            let root: String = row.get(0)?;
            let relative: String = row.get(1)?;
            let path = if relative.is_empty() {
                root
            } else {
                format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
            };
            Ok(ProtectedFolder {
                path,
                marker: row.get(2)?,
                reason: row.get(3)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use df_domain::{ProfileRef, Project, SourceRoot, SourceRootId};

    use crate::repository;

    use super::*;

    /// The penalties of §18.3 survived the move from hardcoded constants to
    /// the declarative `generic` profile (ADR-0026).
    #[test]
    fn generic_markers_and_patterns_are_recognised() {
        let generic = df_domain::Profile::load("generic").unwrap();
        let penalty = |name: &str| {
            let (kind, penalty, _) = generic.classify(name);
            (kind, penalty)
        };
        assert_eq!(penalty("descargas"), (ContextKind::Generic, 50));
        assert_eq!(penalty("escritorio"), (ContextKind::Generic, 45));
        assert_eq!(penalty("backup"), (ContextKind::Generic, 40));
        assert_eq!(penalty("recuperado"), (ContextKind::Generic, 35));
        assert_eq!(penalty("temp"), (ContextKind::Generic, 30));
        assert_eq!(penalty("documento - copia"), (ContextKind::Generic, 30));
        assert_eq!(penalty("copia de informe"), (ContextKind::Generic, 30));
        // A real materia name is neutral under the generic profile.
        assert_eq!(
            generic.classify("expediente 1234-2020").0,
            ContextKind::Neutral
        );
        assert_eq!(generic.classify("periciales").0, ContextKind::Neutral);
    }

    /// The legal profile is what makes rule 9 bite: the same folder name is
    /// neutral under `generic` and a protected boundary under `legal`.
    #[test]
    fn the_legal_profile_turns_expedientes_into_boundaries() {
        let legal = df_domain::Profile::load("legal").unwrap();
        assert_eq!(legal.classify("expediente").0, ContextKind::Protected);
        assert_eq!(legal.classify("periciales").0, ContextKind::Protected);
        // It still recognises the inherited generic containers.
        assert_eq!(
            legal.classify("descargas"),
            (ContextKind::Generic, 50, Some("descargas".to_string()))
        );

        let generic = df_domain::Profile::load("generic").unwrap();
        assert_eq!(generic.classify("expediente").0, ContextKind::Neutral);
    }

    /// `is_protected_boundary` must always agree with `kind`. It was once
    /// hardcoded to 0, which would have made a PROTECTED folder look
    /// unprotected to any reader of that column.
    #[test]
    fn the_protected_flag_always_agrees_with_the_kind() {
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, snapshot_id, root_id) = seed(&mut db);
        add_folder(&db, snapshot_id, root_id, "Expediente", "Expediente");
        add_folder(&db, snapshot_id, root_id, "Descargas", "Descargas");
        add_folder(&db, snapshot_id, root_id, "Informes", "Informes");

        let summary =
            classify_folders(&mut db, project_id, snapshot_id, "legal", Actor::Test).unwrap();
        assert_eq!(summary.protected_boundaries, 1);
        assert_eq!(summary.generic_folders, 1);

        let mismatched: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM folder_contexts
                 WHERE (kind = 'PROTECTED') != (is_protected_boundary = 1)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mismatched, 0, "kind and is_protected_boundary disagree");
        let protected = protected_folders(&db, snapshot_id).unwrap();
        assert_eq!(protected.len(), 1);
        assert!(protected[0].reason.contains("expediente"));
    }

    fn seed(db: &mut Db) -> (ProjectId, SnapshotId, SourceRootId) {
        let project = Project::new(
            "p",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "test",
        );
        let root = SourceRoot::new(project.id, PathBuf::from("D:/in"));
        let root_id = root.id;
        repository::create_project(db, &project, &[root], Actor::Test).unwrap();
        let (snapshot, _run) = crate::inventory::start_scan(db, project.id, Actor::Test).unwrap();
        (project.id, snapshot.id, root_id)
    }

    fn add_folder(db: &Db, snapshot: SnapshotId, root: SourceRootId, rel: &str, name: &str) {
        db.conn()
            .execute(
                "INSERT INTO folders
                    (id, snapshot_id, source_root_id, relative_path,
                     parent_relative_path, name, normalized_name, depth, status,
                     created_at)
                 VALUES (?1,?2,?3,?4,NULL,?5,?6,0,'OK','t')",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    snapshot.to_string(),
                    root.to_string(),
                    rel,
                    name,
                    name.to_lowercase(),
                ],
            )
            .unwrap();
    }

    #[test]
    fn classifies_generic_folders_and_lists_them_by_penalty() {
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, snapshot, root) = seed(&mut db);
        add_folder(&db, snapshot, root, "Descargas", "Descargas");
        add_folder(&db, snapshot, root, "Expediente 12", "Expediente 12");
        add_folder(&db, snapshot, root, "Backup", "Backup");

        let summary =
            classify_folders(&mut db, project_id, snapshot, "generic", Actor::Test).unwrap();
        assert_eq!(summary.generic_folders, 2);
        assert_eq!(summary.neutral_folders, 1);
        assert_eq!(summary.protected_boundaries, 0);

        let generic = generic_folders(&db, snapshot).unwrap();
        assert_eq!(generic.len(), 2);
        // Descargas (50) ranks above Backup (40).
        assert!(generic[0].path.ends_with("Descargas"));
        assert_eq!(generic[0].penalty, 50);
        assert!(generic[1].path.ends_with("Backup"));
        assert_eq!(generic[1].penalty, 40);
    }

    #[test]
    fn reclassification_is_idempotent() {
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, snapshot, root) = seed(&mut db);
        add_folder(&db, snapshot, root, "Downloads", "Downloads");
        let first =
            classify_folders(&mut db, project_id, snapshot, "generic", Actor::Test).unwrap();
        let second =
            classify_folders(&mut db, project_id, snapshot, "generic", Actor::Test).unwrap();
        assert_eq!(first, second);
        assert_eq!(generic_folders(&db, snapshot).unwrap().len(), 1);
    }
}
