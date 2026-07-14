//! Folder context classification (RFC-0001 §18.3).
//!
//! Deterministic, profile-driven: a folder whose name matches a *generic*
//! marker (Descargas, Escritorio, Backup, Recuperado, Copia, Temporales) is
//! tagged `GENERIC` with the penalty weight of §18.3; a folder whose name
//! matches a profile's *protected* marker is a `PROTECTED` boundary that
//! deduplication must never dissolve (rule 9); everything else is `NEUTRAL`.
//!
//! Only the conservative `generic` profile exists for now, and it declares
//! no protected markers (§25.4). Entity anchors and weighted propagation
//! (§18.2–§18.4) are a later slice.

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

/// A generic-container marker and its representative-location penalty
/// (RFC-0001 §18.3). Names are compared in lowercase.
struct Marker {
    name: &'static str,
    penalty: u32,
}

/// Generic markers of the conservative `generic` profile. Penalties follow
/// RFC-0001 §18.3. Matched against a folder's normalized (lowercase) name.
const GENERIC_MARKERS: &[Marker] = &[
    Marker {
        name: "descargas",
        penalty: 50,
    },
    Marker {
        name: "downloads",
        penalty: 50,
    },
    Marker {
        name: "escritorio",
        penalty: 45,
    },
    Marker {
        name: "desktop",
        penalty: 45,
    },
    Marker {
        name: "backup",
        penalty: 40,
    },
    Marker {
        name: "backups",
        penalty: 40,
    },
    Marker {
        name: "copia de seguridad",
        penalty: 40,
    },
    Marker {
        name: "copias de seguridad",
        penalty: 40,
    },
    Marker {
        name: "respaldo",
        penalty: 40,
    },
    Marker {
        name: "respaldos",
        penalty: 40,
    },
    Marker {
        name: "recuperado",
        penalty: 35,
    },
    Marker {
        name: "recuperados",
        penalty: 35,
    },
    Marker {
        name: "recovered",
        penalty: 35,
    },
    Marker {
        name: "recovery",
        penalty: 35,
    },
    Marker {
        name: "temp",
        penalty: 30,
    },
    Marker {
        name: "tmp",
        penalty: 30,
    },
    Marker {
        name: "temporal",
        penalty: 30,
    },
    Marker {
        name: "temporales",
        penalty: 30,
    },
    Marker {
        name: "temporary",
        penalty: 30,
    },
    Marker {
        name: "copia",
        penalty: 30,
    },
    Marker {
        name: "copias",
        penalty: 30,
    },
    Marker {
        name: "copy",
        penalty: 30,
    },
    Marker {
        name: "nueva carpeta",
        penalty: 30,
    },
    Marker {
        name: "new folder",
        penalty: 30,
    },
];

/// Classify a folder name. Returns `(penalty, matched_marker)` when generic,
/// or `None` when neutral. Deterministic and pure.
fn classify_generic(normalized_name: &str) -> Option<(u32, &'static str)> {
    let name = normalized_name.trim();
    for marker in GENERIC_MARKERS {
        if name == marker.name {
            return Some((marker.penalty, marker.name));
        }
    }
    // Common copy patterns produced by Windows Explorer and manual copies.
    if name.ends_with(" - copia") || name.ends_with(" - copy") {
        return Some((30, "copy-suffix"));
    }
    if name.starts_with("copia de ") || name.starts_with("copy of ") {
        return Some((30, "copy-prefix"));
    }
    if name.starts_with("nueva carpeta") || name.starts_with("new folder") {
        return Some((30, "new-folder"));
    }
    None
}

/// Classify every folder of a snapshot under the given profile and persist
/// the result. Idempotent: rows for the snapshot are replaced.
///
/// `profile` selects the marker set; only `generic` is implemented, and any
/// unknown profile falls back to it (conservative default, §25.4).
pub fn classify_folders(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    profile: &str,
    actor: Actor,
) -> DfResult<ContextSummary> {
    // Only the generic profile exists; protected markers are empty for it.
    let _ = profile;

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
        let (kind, penalty, marker) = match classify_generic(normalized_name) {
            Some((penalty, marker)) => {
                summary.generic_folders += 1;
                (ContextKind::Generic, penalty, Some(marker.to_string()))
            }
            None => {
                summary.neutral_folders += 1;
                (ContextKind::Neutral, 0u32, None)
            }
        };
        tx.execute(
            "INSERT INTO folder_contexts
                (folder_id, snapshot_id, relative_path, kind,
                 is_protected_boundary, penalty, marker, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
            params![
                folder_id,
                snapshot,
                relative_path,
                kind.as_str(),
                penalty as i64,
                marker,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use df_domain::{ProfileRef, Project, SourceRoot, SourceRootId};

    use crate::repository;

    use super::*;

    #[test]
    fn generic_markers_and_patterns_are_recognised() {
        assert_eq!(classify_generic("descargas"), Some((50, "descargas")));
        assert_eq!(classify_generic("escritorio"), Some((45, "escritorio")));
        assert_eq!(classify_generic("backup"), Some((40, "backup")));
        assert_eq!(classify_generic("recuperado"), Some((35, "recuperado")));
        assert_eq!(classify_generic("temp"), Some((30, "temp")));
        assert_eq!(
            classify_generic("documento - copia").map(|(p, _)| p),
            Some(30)
        );
        assert_eq!(
            classify_generic("copia de informe").map(|(p, _)| p),
            Some(30)
        );
        // A real materia name is neutral.
        assert_eq!(classify_generic("expediente 1234-2020"), None);
        assert_eq!(classify_generic("periciales"), None);
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
