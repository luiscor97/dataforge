//! Versioned, checksummed migrations.
//!
//! Each migration runs once, inside a transaction, and its SHA-256 is
//! recorded in `schema_migrations`. On every open the checksums of already
//! applied migrations are re-verified so silent schema drift is detected.

use df_error::{DfError, DfResult};
use sha2::{Digest, Sha256};

use crate::{db_err, Db};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Ordered list of every migration known to this build.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "foundation",
        sql: include_str!("../migrations/0001_foundation.sql"),
    },
    Migration {
        version: 2,
        name: "inventory",
        sql: include_str!("../migrations/0002_inventory.sql"),
    },
    Migration {
        version: 3,
        name: "planning",
        sql: include_str!("../migrations/0003_planning.sql"),
    },
    // 4 and 5 belong to the v0.1.1-dev hardening: they are already tagged and
    // applied to real databases, so their numbers are fixed. The Milestone 0.2
    // migrations moved to 6-8 to keep every version unique and consecutive.
    Migration {
        version: 4,
        name: "execution_manifest",
        sql: include_str!("../migrations/0004_execution_manifest.sql"),
    },
    Migration {
        version: 5,
        name: "path_identity",
        sql: include_str!("../migrations/0005_path_identity.sql"),
    },
    Migration {
        version: 6,
        name: "structure",
        sql: include_str!("../migrations/0006_structure.sql"),
    },
    Migration {
        version: 7,
        name: "contexts",
        sql: include_str!("../migrations/0007_contexts.sql"),
    },
    Migration {
        version: 8,
        name: "representatives",
        sql: include_str!("../migrations/0008_representatives.sql"),
    },
    Migration {
        version: 9,
        name: "tree_relations",
        sql: include_str!("../migrations/0009_tree_relations.sql"),
    },
    Migration {
        version: 10,
        name: "structural_review",
        sql: include_str!("../migrations/0010_structural_review.sql"),
    },
    Migration {
        version: 11,
        name: "derived_evidence_seal",
        sql: include_str!("../migrations/0011_derived_evidence_seal.sql"),
    },
    Migration {
        version: 12,
        name: "execution_partial_lease",
        sql: include_str!("../migrations/0012_execution_partial_lease.sql"),
    },
    Migration {
        version: 13,
        name: "content_similarity",
        sql: include_str!("../migrations/0013_content_similarity.sql"),
    },
    Migration {
        version: 14,
        name: "content_intelligence",
        sql: include_str!("../migrations/0014_content_intelligence.sql"),
    },
    Migration {
        version: 15,
        name: "hash_queue_index",
        sql: include_str!("../migrations/0015_hash_queue_index.sql"),
    },
    Migration {
        version: 16,
        name: "media_intelligence",
        sql: include_str!("../migrations/0016_media_intelligence.sql"),
    },
    Migration {
        version: 17,
        name: "plugin_ecosystem",
        sql: include_str!("../migrations/0017_plugin_ecosystem.sql"),
    },
];

fn sql_checksum(sql: &str) -> String {
    // Normalise line endings so the checksum is stable across git eol settings.
    let normalized = sql.replace("\r\n", "\n");
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

/// Apply every pending migration and verify checksums of applied ones.
pub fn apply_migrations(db: &mut Db) -> DfResult<()> {
    db.conn()
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version    INTEGER PRIMARY KEY,
                name       TEXT NOT NULL,
                sha256     TEXT NOT NULL,
                applied_at TEXT NOT NULL
            ) STRICT;",
        )
        .map_err(db_err)?;

    for migration in MIGRATIONS {
        let applied: Option<String> = db
            .conn()
            .query_row(
                "SELECT sha256 FROM schema_migrations WHERE version = ?1",
                [migration.version],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(db_err(other)),
            })?;

        let checksum = sql_checksum(migration.sql);
        match applied {
            Some(stored) if stored == checksum => continue,
            Some(stored) => {
                return Err(DfError::Database(format!(
                    "migration {:04} `{}` drifted: stored checksum {stored} != current {checksum}",
                    migration.version, migration.name
                )));
            }
            None => {
                let tx = db.conn_mut().transaction().map_err(db_err)?;
                tx.execute_batch(migration.sql).map_err(db_err)?;
                tx.execute(
                    "INSERT INTO schema_migrations (version, name, sha256, applied_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        migration.version,
                        migration.name,
                        checksum,
                        df_ledger::canonical_timestamp(chrono::Utc::now()),
                    ],
                )
                .map_err(db_err)?;
                tx.commit().map_err(db_err)?;
            }
        }
    }
    Ok(())
}

/// Verify (without applying anything) that applied migrations match this
/// build. Used by the integrity check.
pub fn verify_applied(db: &Db) -> DfResult<()> {
    for migration in MIGRATIONS {
        let stored: String = db
            .conn()
            .query_row(
                "SELECT sha256 FROM schema_migrations WHERE version = ?1",
                [migration.version],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DfError::Database(format!(
                    "migration {:04} `{}` has not been applied",
                    migration.version, migration.name
                )),
                other => db_err(other),
            })?;
        if stored != sql_checksum(migration.sql) {
            return Err(DfError::Database(format!(
                "migration {:04} `{}` checksum mismatch",
                migration.version, migration.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct DerivedSnapshotFixture {
        snapshot_id: df_domain::SnapshotId,
        review_item_id: String,
    }

    fn seed_derived_snapshot(
        db: &Db,
        project_id: df_domain::ProjectId,
        source_root_id: df_domain::SourceRootId,
        label: &str,
    ) -> DerivedSnapshotFixture {
        use df_domain::{
            ContentId, DuplicateSetId, FolderId, OccurrenceId, SnapshotId, TreeCloneSetId,
        };
        use rusqlite::params;

        let snapshot_id = SnapshotId::new();
        let folder_a = FolderId::new();
        let folder_b = FolderId::new();
        let occurrence_a = OccurrenceId::new();
        let occurrence_b = OccurrenceId::new();
        let content_id = ContentId::new();
        let duplicate_set_id = DuplicateSetId::new();
        let clone_set_id = TreeCloneSetId::new();
        let relation_id = format!("relation-{label}");
        let anomaly_id = format!("anomaly-{label}");
        let review_item_id = format!("review-{label}");
        let content_sha = hex::encode(Sha256::digest(format!("content-{label}").as_bytes()));
        let tree_signature = hex::encode(Sha256::digest(format!("tree-{label}").as_bytes()));

        db.conn()
            .execute(
                "INSERT INTO snapshots (id, project_id, status, created_at)
                 VALUES (?1, ?2, 'COMPLETE', 't')",
                params![snapshot_id.to_string(), project_id.to_string()],
            )
            .unwrap();
        for (folder, relative) in [(folder_a, "a"), (folder_b, "b")] {
            db.conn()
                .execute(
                    "INSERT INTO folders
                        (id, snapshot_id, source_root_id, relative_path,
                         parent_relative_path, name, normalized_name, depth,
                         status, created_at)
                     VALUES (?1, ?2, ?3, ?4, '', ?4, ?4, 1, 'OK', 't')",
                    params![
                        folder.to_string(),
                        snapshot_id.to_string(),
                        source_root_id.to_string(),
                        relative,
                    ],
                )
                .unwrap();
        }
        for (occurrence, parent, file_name) in [
            (occurrence_a, "a", "document-a.txt"),
            (occurrence_b, "b", "document-b.txt"),
        ] {
            let relative = format!("{parent}/{file_name}");
            db.conn()
                .execute(
                    "INSERT INTO path_occurrences
                        (id, snapshot_id, source_root_id, relative_path,
                         parent_relative_path, file_name, normalized_name,
                         extension, size_bytes, attributes, path_length, depth,
                         fingerprint, scan_status, name_is_lossy, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 'txt', 1, 0,
                             ?7, 2, 'v1:1:0', 'OK', 0, 't')",
                    params![
                        occurrence.to_string(),
                        snapshot_id.to_string(),
                        source_root_id.to_string(),
                        relative,
                        parent,
                        file_name,
                        relative.encode_utf16().count() as i64,
                    ],
                )
                .unwrap();
        }
        db.conn()
            .execute(
                "INSERT INTO content_objects
                    (id, size_bytes, sha256, blake3, first_seen_snapshot,
                     hash_state, created_at)
                 VALUES (?1, 1, ?2, ?2, ?3, 'HASHED', 't')",
                params![content_id.to_string(), content_sha, snapshot_id.to_string(),],
            )
            .unwrap();
        for occurrence in [occurrence_a, occurrence_b] {
            db.conn()
                .execute(
                    "INSERT INTO occurrence_content
                        (occurrence_id, content_id, created_at)
                     VALUES (?1, ?2, 't')",
                    params![occurrence.to_string(), content_id.to_string()],
                )
                .unwrap();
        }

        db.conn()
            .execute(
                "INSERT INTO duplicate_sets
                    (id, snapshot_id, content_id, occurrence_count, size_bytes,
                     created_at)
                 VALUES (?1, ?2, ?3, 2, 1, 't')",
                params![
                    duplicate_set_id.to_string(),
                    snapshot_id.to_string(),
                    content_id.to_string(),
                ],
            )
            .unwrap();
        for (folder, relative) in [(folder_a, "a"), (folder_b, "b")] {
            db.conn()
                .execute(
                    "INSERT INTO folder_signatures
                        (folder_id, snapshot_id, source_root_id, relative_path,
                         signature, is_complete, subtree_files, subtree_bytes,
                         created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, 1, 1, 't')",
                    params![
                        folder.to_string(),
                        snapshot_id.to_string(),
                        source_root_id.to_string(),
                        relative,
                        tree_signature,
                    ],
                )
                .unwrap();
        }
        db.conn()
            .execute(
                "INSERT INTO tree_clone_sets
                    (id, snapshot_id, signature, relationship, folder_count,
                     subtree_files, subtree_bytes, created_at)
                 VALUES (?1, ?2, ?3, 'EXACT_TREE_CLONE', 2, 1, 1, 't')",
                params![
                    clone_set_id.to_string(),
                    snapshot_id.to_string(),
                    tree_signature,
                ],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO folder_contexts
                    (folder_id, snapshot_id, relative_path, kind,
                     is_protected_boundary, penalty, marker, created_at, reason)
                 VALUES (?1, ?2, 'a', 'NEUTRAL', 0, 0, NULL, 't', 'test')",
                params![folder_a.to_string(), snapshot_id.to_string()],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tree_relations
                    (id, snapshot_id, folder_a, folder_b, relationship,
                     contained, shared_files, unique_a_files, unique_b_files,
                     shared_bytes, similarity, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'PARTIAL_TREE_CLONE', NULL,
                         1, 1, 1, 1, 0.5, 't')",
                params![
                    relation_id,
                    snapshot_id.to_string(),
                    folder_a.to_string(),
                    folder_b.to_string(),
                ],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO duplicate_representatives
                    (duplicate_set_id, snapshot_id, occurrence_id, score,
                     reason, created_at)
                 VALUES (?1, ?2, ?3, 1, 'test', 't')",
                params![
                    duplicate_set_id.to_string(),
                    snapshot_id.to_string(),
                    occurrence_a.to_string(),
                ],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO structural_anomalies
                    (id, snapshot_id, analysis_version, occurrence_id, folder_a,
                     folder_b, kind, severity, requires_review, summary,
                     evidence_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, NULL, 'EXTREME_PATH',
                         'WARNING', 1, 'test', '{}', 't')",
                params![
                    anomaly_id,
                    snapshot_id.to_string(),
                    crate::analysis::ANALYSIS_VERSION as i64,
                    occurrence_a.to_string(),
                ],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO review_items
                    (id, snapshot_id, analysis_version, anomaly_id,
                     rule_match_id, occurrence_id, recommended_action, risk,
                     reason, created_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'COPY_REVIEW',
                         'MEDIUM', 'test', 't')",
                params![
                    review_item_id,
                    snapshot_id.to_string(),
                    crate::analysis::ANALYSIS_VERSION as i64,
                    anomaly_id,
                    occurrence_a.to_string(),
                ],
            )
            .unwrap();

        DerivedSnapshotFixture {
            snapshot_id,
            review_item_id,
        }
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut db = Db::open_in_memory().expect("open applies migrations");
        apply_migrations(&mut db).expect("second run is a no-op");
        verify_applied(&db).expect("checksums match");
    }

    #[test]
    fn migration_versions_are_strictly_increasing() {
        let mut previous = 0;
        for migration in MIGRATIONS {
            assert!(migration.version > previous);
            previous = migration.version;
        }
    }

    #[test]
    fn checksum_is_stable_across_line_endings() {
        assert_eq!(sql_checksum("a\r\nb"), sql_checksum("a\nb"));
    }

    /// A v0.1 database must still open under v0.1.1 (encargo §6): create one
    /// with only the migrations that shipped in 0.1, close it, reopen with the
    /// current build, and check the new migrations applied on top while the
    /// existing project, inventory and ledger survived intact.
    #[test]
    fn a_v0_1_database_migrates_forward_and_keeps_its_data() {
        use df_domain::{Actor, ProfileRef, Project, SourceRoot};
        use std::path::PathBuf;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v01.sqlite");

        // --- 1. Build a database with the 0.1 schema only (0001..0003).
        let project_id;
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    version    INTEGER PRIMARY KEY,
                    name       TEXT NOT NULL,
                    sha256     TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                 ) STRICT;",
            )
            .unwrap();
            for migration in MIGRATIONS.iter().filter(|m| m.version <= 3) {
                conn.execute_batch(migration.sql).unwrap();
                conn.execute(
                    "INSERT INTO schema_migrations (version, name, sha256, applied_at)
                     VALUES (?1, ?2, ?3, '2026-01-01T00:00:00.000Z')",
                    rusqlite::params![
                        migration.version,
                        migration.name,
                        sql_checksum(migration.sql)
                    ],
                )
                .unwrap();
            }
            drop(conn);

            // Populate it through the ordinary repositories.
            let mut db = Db::open(&path).unwrap();
            let project = Project::new(
                "Proyecto 0.1",
                ProfileRef::default(),
                PathBuf::from("D:/out"),
                PathBuf::from("D:/audit"),
                "0.1.0",
            );
            project_id = project.id;
            let roots = vec![SourceRoot::new(project.id, PathBuf::from("D:/in"))];
            crate::repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
            crate::inventory::start_scan(&mut db, project.id, Actor::Test).unwrap();
        }

        // --- 2. Reopen with the current build: 0004 and 0005 apply on top.
        let db = Db::open(&path).unwrap();

        let applied: Vec<i64> = db
            .conn()
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            applied,
            MIGRATIONS.iter().map(|m| m.version).collect::<Vec<_>>(),
            "every migration must be applied after reopening"
        );
        verify_applied(&db).expect("checksums of the 0.1 migrations still match");

        // --- 3. The old data survived.
        let project = crate::repository::load_project(&db).unwrap();
        assert_eq!(project.id, project_id);
        assert_eq!(project.name, "Proyecto 0.1");
        assert_eq!(
            crate::repository::load_source_roots(&db, project.id)
                .unwrap()
                .len(),
            1
        );
        let events = crate::repository::list_events(&db, project.id).unwrap();
        assert!(!events.is_empty());
        df_ledger::verify_chain(&events).expect("the 0.1 ledger still verifies");

        // --- 4. The new tables exist and the database is sound.
        // 0004 added the manifest table.
        let manifest_tables: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'execution_manifest'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(manifest_tables, 1, "0004 must add execution_manifest");
        // 0005 added columns to pre-existing tables.
        let has_raw: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('path_occurrences')
                 WHERE name = 'raw_relative_path'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_raw, 1, "0005 must add the raw path column");

        let report = crate::integrity::check(&db).unwrap();
        assert!(report.is_ok(), "{:?}", report.problems);
    }

    #[test]
    fn completed_analysis_seals_all_derived_evidence_but_not_new_snapshots_or_decisions() {
        use df_domain::{Actor, ProfileRef, Project, SourceRoot};
        use rusqlite::params;
        use std::path::PathBuf;

        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "evidence-seal",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "test",
        );
        let source_root = SourceRoot::new(project.id, PathBuf::from("D:/in"));
        let source_root_id = source_root.id;
        crate::repository::create_project(&mut db, &project, &[source_root], Actor::Test).unwrap();

        let sealed = seed_derived_snapshot(&db, project.id, source_root_id, "sealed");
        crate::analysis::complete_analysis(
            &mut db,
            project.id,
            sealed.snapshot_id,
            "generic",
            &serde_json::json!({ "fixture": "sealed" }),
            Actor::Test,
        )
        .unwrap();

        let tables = [
            "duplicate_sets",
            "folder_signatures",
            "tree_clone_sets",
            "folder_contexts",
            "tree_relations",
            "duplicate_representatives",
        ];
        for table in tables {
            let insert = format!(
                "INSERT INTO {table}
                 SELECT * FROM {table} WHERE snapshot_id = ?1 LIMIT 1"
            );
            let error = db
                .conn()
                .execute(&insert, [sealed.snapshot_id.to_string()])
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("completed derived evidence is sealed: {table}")),
                "{table} INSERT escaped the seal: {error}"
            );

            let update =
                format!("UPDATE {table} SET created_at = created_at WHERE snapshot_id = ?1");
            let error = db
                .conn()
                .execute(&update, [sealed.snapshot_id.to_string()])
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("completed derived evidence is sealed: {table}")),
                "{table} UPDATE escaped the seal: {error}"
            );

            let delete = format!("DELETE FROM {table} WHERE snapshot_id = ?1");
            let error = db
                .conn()
                .execute(&delete, [sealed.snapshot_id.to_string()])
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("completed derived evidence is sealed: {table}")),
                "{table} DELETE escaped the seal: {error}"
            );
        }

        // Human review is the intentional post-analysis append stream.
        let inserted = db
            .conn()
            .execute(
                "INSERT INTO review_decisions
                    (id, review_item_id, sequence, decision, rationale, actor,
                     created_at)
                 VALUES ('post-completion-decision', ?1, 1, 'COPY_REVIEW',
                         'human decision remains append-only', 'test', 't')",
                params![sealed.review_item_id],
            )
            .unwrap();
        assert_eq!(inserted, 1);

        // A completion seals one snapshot, not the whole database. All six
        // tables can still be populated and revised while a fresh snapshot is
        // being analysed.
        let open = seed_derived_snapshot(&db, project.id, source_root_id, "open");
        for table in tables {
            let update =
                format!("UPDATE {table} SET created_at = created_at WHERE snapshot_id = ?1");
            let changed = db
                .conn()
                .execute(&update, [open.snapshot_id.to_string()])
                .unwrap();
            assert!(
                changed > 0,
                "{table} must remain writable for a new snapshot"
            );
        }
        for table in [
            "duplicate_representatives",
            "tree_relations",
            "folder_contexts",
            "tree_clone_sets",
            "folder_signatures",
            "duplicate_sets",
        ] {
            let delete = format!("DELETE FROM {table} WHERE snapshot_id = ?1");
            let changed = db
                .conn()
                .execute(&delete, [open.snapshot_id.to_string()])
                .unwrap();
            assert!(changed > 0, "{table} must remain mutable before completion");
        }
    }
}
