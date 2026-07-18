//! SQLite persistence for DataForge.
//!
//! SQLite is the single transactional source of truth (RFC-0001 rule 5).
//! This crate owns the schema (versioned migrations), the repositories and
//! the integrity checks. No other crate issues SQL.

pub mod analysis;
pub mod context;
pub mod dedup;
pub mod extraction;
pub mod integrity;
pub mod inventory;
pub mod media;
pub mod migrations;
pub mod plans;
pub mod repository;
pub mod similarity;
pub mod structure;

use std::path::Path;

use df_error::{DfError, DfResult};

/// Map a rusqlite failure into the shared error type.
pub(crate) fn db_err(error: rusqlite::Error) -> DfError {
    DfError::Database(error.to_string())
}

/// A handle to one project database.
pub struct Db {
    conn: rusqlite::Connection,
}

impl Db {
    /// Open (or create) a database file and apply pending migrations.
    pub fn open(path: &Path) -> DfResult<Self> {
        let conn = rusqlite::Connection::open(path).map_err(db_err)?;
        Self::from_connection(conn)
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> DfResult<Self> {
        let conn = rusqlite::Connection::open_in_memory().map_err(db_err)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: rusqlite::Connection) -> DfResult<Self> {
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(db_err)?;
        // The threat model assumes an attacker may have had the `.sqlite`
        // file in hand; schema-embedded SQL from such a file must not run
        // with the application's authority.
        conn.pragma_update(None, "trusted_schema", false)
            .map_err(db_err)?;
        // A commit must be durable when it returns.
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(db_err)?;
        // WAL turns each commit into one sequential append instead of
        // rollback-journal file churn. The pragma answers with the mode that
        // is actually active: `wal` for file databases, `memory` for
        // in-memory ones, or the previous mode on filesystems without WAL
        // support — everything keeps working there, just slower.
        let _active_mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(db_err)?;
        // Another DataForge process holding the write lock (desktop and CLI
        // on the same project) should wait briefly, not fail with BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_err)?;
        let mut db = Self { conn };
        migrations::apply_migrations(&mut db)?;
        Ok(db)
    }

    /// Raw connection for security tests that must simulate an attacker with
    /// the `.sqlite` file in hand (dropping triggers, forging rows).
    ///
    /// Behind the `test-support` feature: a production build cannot reach it,
    /// so the "only df-db issues SQL" rule still holds where it matters.
    #[cfg(feature = "test-support")]
    pub fn conn_for_tests(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut rusqlite::Connection {
        &mut self.conn
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    fn pragma<T: rusqlite::types::FromSql>(db: &Db, name: &str) -> T {
        db.conn()
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn file_databases_open_hardened_and_in_wal_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        assert_eq!(pragma::<String>(&db, "journal_mode"), "wal");
        assert_eq!(pragma::<i64>(&db, "foreign_keys"), 1);
        assert_eq!(pragma::<i64>(&db, "trusted_schema"), 0);
        // 2 = FULL.
        assert_eq!(pragma::<i64>(&db, "synchronous"), 2);
        assert!(pragma::<i64>(&db, "busy_timeout") >= 5_000);
    }

    #[test]
    fn in_memory_databases_still_open_without_wal_support() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(pragma::<String>(&db, "journal_mode"), "memory");
        assert_eq!(pragma::<i64>(&db, "trusted_schema"), 0);
    }
}
