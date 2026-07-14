//! SQLite persistence for DataForge.
//!
//! SQLite is the single transactional source of truth (RFC-0001 rule 5).
//! This crate owns the schema (versioned migrations), the repositories and
//! the integrity checks. No other crate issues SQL.

pub mod integrity;
pub mod inventory;
pub mod migrations;
pub mod plans;
pub mod repository;

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
        let mut db = Self { conn };
        migrations::apply_migrations(&mut db)?;
        Ok(db)
    }

    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut rusqlite::Connection {
        &mut self.conn
    }
}
