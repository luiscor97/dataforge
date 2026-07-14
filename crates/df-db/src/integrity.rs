//! Database + ledger integrity checks, surfaced by `project status`.

use df_error::{DfError, DfResult};
use serde::Serialize;

use crate::{db_err, migrations, repository, Db};

/// Result of a full integrity pass.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrityReport {
    /// `PRAGMA integrity_check` returned `ok`.
    pub database_ok: bool,
    /// `PRAGMA foreign_key_check` returned no violations.
    pub foreign_keys_ok: bool,
    /// Applied migrations match the checksums compiled into this build.
    pub migrations_ok: bool,
    /// The audit event chain verifies cryptographically.
    pub ledger_ok: bool,
    /// Human-readable description of every problem found.
    pub problems: Vec<String>,
}

impl IntegrityReport {
    pub fn is_ok(&self) -> bool {
        self.database_ok && self.foreign_keys_ok && self.migrations_ok && self.ledger_ok
    }
}

/// Run every integrity check. Returns `Ok(report)` even when checks fail;
/// only infrastructure errors (e.g. unreadable file) become `Err`.
pub fn check(db: &Db) -> DfResult<IntegrityReport> {
    let mut report = IntegrityReport {
        database_ok: true,
        foreign_keys_ok: true,
        migrations_ok: true,
        ledger_ok: true,
        problems: Vec::new(),
    };

    let verdict: String = db
        .conn()
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(db_err)?;
    if verdict != "ok" {
        report.database_ok = false;
        report.problems.push(format!("integrity_check: {verdict}"));
    }

    let mut stmt = db
        .conn()
        .prepare("PRAGMA foreign_key_check")
        .map_err(db_err)?;
    let violations = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_err)?
        .count();
    if violations > 0 {
        report.foreign_keys_ok = false;
        report
            .problems
            .push(format!("foreign_key_check: {violations} violation(s)"));
    }

    if let Err(e) = migrations::verify_applied(db) {
        report.migrations_ok = false;
        report.problems.push(e.to_string());
    }

    match repository::load_project(db) {
        Ok(project) => {
            let events = repository::list_events(db, project.id)?;
            if let Err(e) = df_ledger::verify_chain(&events) {
                report.ledger_ok = false;
                report.problems.push(e.to_string());
            }
        }
        Err(DfError::NotFound(_)) => {
            // An empty database is internally consistent.
        }
        Err(other) => return Err(other),
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use df_domain::{Actor, ProfileRef, Project, SourceRoot};

    use super::*;

    #[test]
    fn fresh_database_passes_all_checks() {
        let db = Db::open_in_memory().unwrap();
        let report = check(&db).unwrap();
        assert!(report.is_ok(), "{:?}", report.problems);
    }

    #[test]
    fn populated_database_passes_all_checks() {
        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "p",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "0.0.1-dev",
        );
        let roots = vec![SourceRoot::new(project.id, PathBuf::from("D:/in"))];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        let report = check(&db).unwrap();
        assert!(report.is_ok(), "{:?}", report.problems);
    }

    #[test]
    fn ledger_tampering_is_reported() {
        let mut db = Db::open_in_memory().unwrap();
        let project = Project::new(
            "p",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "0.0.1-dev",
        );
        repository::create_project(&mut db, &project, &[], Actor::Test).unwrap();
        // Bypass the append-only triggers to simulate offline tampering.
        db.conn()
            .execute_batch("DROP TRIGGER audit_events_no_update;")
            .unwrap();
        db.conn()
            .execute(
                "UPDATE audit_events SET payload_json = '{\"forged\":true}'",
                [],
            )
            .unwrap();
        let report = check(&db).unwrap();
        assert!(!report.ledger_ok);
        assert!(!report.is_ok());
    }
}
