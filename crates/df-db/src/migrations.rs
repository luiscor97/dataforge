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
}
