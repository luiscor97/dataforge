//! Persistence for rule sets — the tunable half of the gate (ADR-0041).
//!
//! This module stores and returns **opaque canonical JSON**. It does not parse
//! params, does not know what a weight means and cannot recompute a digest,
//! which is deliberate: the meaning of a rule set belongs to `df-rules`, and a
//! second implementation of the digest here would be a second thing that can
//! disagree. Persistence keeps the bytes and their digest together; deciding
//! whether those bytes still say what they said is the caller's, on every use
//! (ADR-0041, threat A4).
//!
//! # Append-only
//!
//! A stored `(id, version)` is immutable. Every gate decision names the set
//! that produced it, so editing one in place would rewrite the meaning of
//! decisions already taken and audited. [`save`] therefore refuses a version
//! that exists with different bytes rather than updating it: a change is a new
//! version, always.

use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension};

use crate::repository::to_stored_timestamp;
use crate::{db_err, Db};

/// One stored rule set, as bytes plus the digest taken over them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuleSetRecord {
    pub id: String,
    pub version: u32,
    /// Contract version of the params shape.
    pub schema: String,
    /// Canonical JSON, exactly the bytes the digest covers.
    pub params: String,
    pub digest: String,
    pub created_at: String,
}

/// Store a rule set version.
///
/// Idempotent for identical bytes, and a [`DfError::Conflict`] for the same
/// version with different ones. Overwriting would change what past decisions
/// meant, so it is not offered — not even as an option a caller could pass.
pub fn save(db: &Db, record: &RuleSetRecord) -> DfResult<()> {
    let existing: Option<(String, String)> = db
        .conn()
        .query_row(
            "SELECT params, digest FROM rule_sets WHERE id = ?1 AND version = ?2",
            params![record.id, record.version],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;

    if let Some((params, digest)) = existing {
        if params == record.params && digest == record.digest {
            return Ok(());
        }
        return Err(DfError::Conflict(format!(
            "rule set `{}` version {} already exists with different parameters; \
             a change is a new version, because decisions already name this one",
            record.id, record.version
        )));
    }

    db.conn()
        .execute(
            "INSERT INTO rule_sets (id, version, schema, params, digest, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.id,
                record.version,
                record.schema,
                record.params,
                record.digest,
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
    Ok(())
}

/// Read one version back, byte for byte.
pub fn load(db: &Db, id: &str, version: u32) -> DfResult<Option<RuleSetRecord>> {
    db.conn()
        .query_row(
            "SELECT id, version, schema, params, digest, created_at
             FROM rule_sets WHERE id = ?1 AND version = ?2",
            params![id, version],
            |row| {
                Ok(RuleSetRecord {
                    id: row.get(0)?,
                    version: row.get::<_, i64>(1)? as u32,
                    schema: row.get(2)?,
                    params: row.get(3)?,
                    digest: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(db_err)
}

/// The highest stored version of a set.
pub fn latest(db: &Db, id: &str) -> DfResult<Option<RuleSetRecord>> {
    let version: Option<i64> = db
        .conn()
        .query_row(
            "SELECT MAX(version) FROM rule_sets WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .flatten();
    match version {
        Some(version) => load(db, id, version as u32),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(version: u32, params: &str) -> RuleSetRecord {
        RuleSetRecord {
            id: "legal".to_string(),
            version,
            schema: "dataforge.rule-set/0.1.0".to_string(),
            params: params.to_string(),
            digest: "a".repeat(64),
            created_at: String::new(),
        }
    }

    #[test]
    fn a_stored_set_comes_back_byte_for_byte() {
        let db = Db::open_in_memory().unwrap();
        let written = record(1, r#"{"depth_weight":-8.0}"#);
        save(&db, &written).unwrap();

        let read = load(&db, "legal", 1).unwrap().expect("stored");
        // Byte-identical, because the digest was taken over these bytes and a
        // round trip that reformatted them would invalidate it.
        assert_eq!(read.params, written.params);
        assert_eq!(read.digest, written.digest);
        assert_eq!(read.schema, written.schema);
    }

    #[test]
    fn rewriting_a_version_with_different_parameters_is_refused() {
        // The property that makes a recorded decision mean something a year
        // later: the set it names cannot have changed underneath it.
        let db = Db::open_in_memory().unwrap();
        save(&db, &record(1, r#"{"depth_weight":-8.0}"#)).unwrap();

        let error = save(&db, &record(1, r#"{"depth_weight":-99.0}"#))
            .expect_err("the same version cannot mean two things");
        assert!(matches!(error, DfError::Conflict(_)));

        // And the original survived the attempt.
        let read = load(&db, "legal", 1).unwrap().expect("stored");
        assert_eq!(read.params, r#"{"depth_weight":-8.0}"#);
    }

    #[test]
    fn storing_the_same_bytes_again_is_not_an_error() {
        // Two processes converging on the same set is not a conflict, and
        // failing there would make a retry harder than it should be.
        let db = Db::open_in_memory().unwrap();
        save(&db, &record(1, r#"{"depth_weight":-8.0}"#)).unwrap();
        save(&db, &record(1, r#"{"depth_weight":-8.0}"#)).expect("idempotent");
    }

    #[test]
    fn a_new_version_is_how_a_rule_set_changes() {
        let db = Db::open_in_memory().unwrap();
        save(&db, &record(1, r#"{"depth_weight":-8.0}"#)).unwrap();
        save(&db, &record(2, r#"{"depth_weight":-12.0}"#)).unwrap();

        assert_eq!(latest(&db, "legal").unwrap().expect("latest").version, 2);
        // The old one is still readable, which is what lets an old decision
        // still be explained.
        assert_eq!(
            load(&db, "legal", 1).unwrap().expect("v1").params,
            r#"{"depth_weight":-8.0}"#
        );
    }

    #[test]
    fn an_unknown_set_is_absent_rather_than_an_error() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(load(&db, "nope", 1).unwrap(), None);
        assert_eq!(latest(&db, "nope").unwrap(), None);
    }
}
