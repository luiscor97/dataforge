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
    Migration {
        version: 4,
        name: "structure",
        sql: include_str!("../migrations/0004_structure.sql"),
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
}
