//! Persistence for consent by policy and its spending (ADR-0042).
//!
//! Like [`crate::rules`], this module keeps **opaque canonical JSON** and does
//! not parse a policy or recompute its digest: the format belongs to `df-ai`,
//! and a second implementation of the digest here is a second thing that can
//! disagree.
//!
//! # Consumption is a sum, not a stored total
//!
//! There is no counter to increment. [`consumption`] sums the charges, so what
//! a caller is protected by is reconstructible from evidence rather than
//! asserted by a number somebody could set. A stored total is one UPDATE from
//! a budget that never runs out; this one can only be rewound by deleting
//! rows, and the triggers refuse that.
//!
//! Charges name the policy **digest**, so superseding a policy leaves old
//! charges pointing at the terms actually agreed rather than at whatever the
//! name means afterwards.

use df_domain::{Actor, ProjectId};
use df_error::{DfError, DfResult};
use rusqlite::{params, OptionalExtension};

use crate::repository::to_stored_timestamp;
use crate::{db_err, Db};

/// One approved disclosure policy, as bytes plus the digest over them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StoredPolicy {
    pub id: String,
    pub version: u32,
    pub schema: String,
    /// Canonical JSON, exactly the bytes the digest covers.
    pub policy: String,
    pub digest: String,
    pub approved_at: String,
    pub approved_by: String,
}

/// What has been spent under one policy, summed from its charges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Spent {
    pub calls: u32,
    pub disclosed_bytes: u64,
    pub spend_cents: u64,
}

/// The terms being approved, kept together because they only mean anything
/// together: a digest without the bytes it covers proves nothing.
#[derive(Debug, Clone, Copy)]
pub struct PolicyApproval<'a> {
    pub id: &'a str,
    pub version: u32,
    /// Contract version of the policy shape.
    pub schema: &'a str,
    /// Canonical JSON, exactly the bytes the digest covers.
    pub policy: &'a str,
    pub digest: &'a str,
}

/// Record a human approving a policy version.
///
/// Idempotent for identical bytes; a [`DfError::Conflict`] for the same
/// version with different ones. An approval that could be edited afterwards
/// would not be an approval of anything in particular.
pub fn approve(
    db: &Db,
    project_id: ProjectId,
    approval: &PolicyApproval<'_>,
    actor: Actor,
) -> DfResult<()> {
    let PolicyApproval {
        id,
        version,
        schema,
        policy,
        digest,
    } = *approval;

    let existing: Option<(String, String)> = db
        .conn()
        .query_row(
            "SELECT policy, digest FROM disclosure_policies
             WHERE project_id = ?1 AND id = ?2 AND version = ?3",
            params![project_id.to_string(), id, version],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;

    if let Some((stored_policy, stored_digest)) = existing {
        if stored_policy == policy && stored_digest == digest {
            return Ok(());
        }
        return Err(DfError::Conflict(format!(
            "disclosure policy `{id}` version {version} is already approved with different \
             terms; approve a new version, because charges already name this one"
        )));
    }

    db.conn()
        .execute(
            "INSERT INTO disclosure_policies
                (project_id, id, version, schema, policy, digest, approved_at, approved_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project_id.to_string(),
                id,
                version,
                schema,
                policy,
                digest,
                to_stored_timestamp(chrono::Utc::now()),
                actor.as_str(),
            ],
        )
        .map_err(db_err)?;
    Ok(())
}

/// The highest approved version of a policy.
pub fn latest(db: &Db, project_id: ProjectId, id: &str) -> DfResult<Option<StoredPolicy>> {
    db.conn()
        .query_row(
            "SELECT id, version, schema, policy, digest, approved_at, approved_by
             FROM disclosure_policies
             WHERE project_id = ?1 AND id = ?2
             ORDER BY version DESC LIMIT 1",
            params![project_id.to_string(), id],
            |row| {
                Ok(StoredPolicy {
                    id: row.get(0)?,
                    version: row.get::<_, i64>(1)? as u32,
                    schema: row.get(2)?,
                    policy: row.get(3)?,
                    digest: row.get(4)?,
                    approved_at: row.get(5)?,
                    approved_by: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(db_err)
}

/// Charge one completed invocation against the terms it happened under.
///
/// Called after the disclosure, with what actually left rather than what was
/// estimated. The budget check reads [`consumption`], so a charge that is
/// never written is a budget that never depletes — which is why this is a
/// separate, explicit call and not something inferred from an audit row.
pub fn charge(
    db: &Db,
    project_id: ProjectId,
    policy_digest: &str,
    disclosed_bytes: u64,
    spend_cents: u64,
) -> DfResult<()> {
    db.conn()
        .execute(
            "INSERT INTO disclosure_charges
                (id, project_id, policy_digest, disclosed_bytes, spend_cents, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                uuid::Uuid::new_v4().to_string(),
                project_id.to_string(),
                policy_digest,
                disclosed_bytes as i64,
                spend_cents as i64,
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
    Ok(())
}

/// What has been spent under these exact terms.
pub fn consumption(db: &Db, project_id: ProjectId, policy_digest: &str) -> DfResult<Spent> {
    db.conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(disclosed_bytes), 0), COALESCE(SUM(spend_cents), 0)
             FROM disclosure_charges
             WHERE project_id = ?1 AND policy_digest = ?2",
            params![project_id.to_string(), policy_digest],
            |row| {
                Ok(Spent {
                    calls: row.get::<_, i64>(0)? as u32,
                    disclosed_bytes: row.get::<_, i64>(1)? as u64,
                    spend_cents: row.get::<_, i64>(2)? as u64,
                })
            },
        )
        .map_err(db_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{ProfileRef, Project};

    const DIGEST: &str = "ab12";

    fn digest(seed: &str) -> String {
        format!("{seed}{}", "0".repeat(64 - seed.len()))
    }

    fn terms<'a>(
        id: &'a str,
        version: u32,
        policy: &'a str,
        digest: &'a str,
    ) -> PolicyApproval<'a> {
        PolicyApproval {
            id,
            version,
            schema: "dataforge.ai-disclosure-policy/0.1.0",
            policy,
            digest,
        }
    }

    fn project(db: &mut Db) -> ProjectId {
        let project = Project::new(
            "Consentimiento",
            ProfileRef::default(),
            std::path::PathBuf::from("salida"),
            std::path::PathBuf::from("auditoria"),
            "test",
        );
        crate::repository::create_project(db, &project, &[], Actor::Test).unwrap();
        project.id
    }

    #[test]
    fn an_approved_policy_comes_back_with_who_approved_it() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        approve(
            &db,
            id,
            &PolicyApproval {
                id: "legal",
                version: 1,
                schema: "dataforge.ai-disclosure-policy/0.1.0",
                policy: r#"{"budget":{"calls":10}}"#,
                digest: &digest(DIGEST),
            },
            Actor::Cli,
        )
        .unwrap();

        let stored = latest(&db, id, "legal").unwrap().expect("approved");
        assert_eq!(stored.version, 1);
        assert_eq!(stored.policy, r#"{"budget":{"calls":10}}"#);
        // A policy is a human act, and the record says whose.
        assert_eq!(stored.approved_by, Actor::Cli.as_str());
    }

    #[test]
    fn consumption_is_summed_from_charges_and_never_stored() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        let terms = digest(DIGEST);

        assert_eq!(consumption(&db, id, &terms).unwrap(), Spent::default());

        charge(&db, id, &terms, 1_000, 25).unwrap();
        charge(&db, id, &terms, 2_500, 40).unwrap();

        let spent = consumption(&db, id, &terms).unwrap();
        assert_eq!(spent.calls, 2);
        assert_eq!(spent.disclosed_bytes, 3_500);
        assert_eq!(spent.spend_cents, 65);
    }

    #[test]
    fn a_charge_cannot_be_edited_or_removed() {
        // The property that makes the budget mean something: a spent budget
        // cannot be quietly rewound. Enforced in the schema, so it holds for
        // anything holding the file and not only for callers of this module.
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        let terms = digest(DIGEST);
        charge(&db, id, &terms, 1_000, 25).unwrap();

        let updated = db
            .conn()
            .execute("UPDATE disclosure_charges SET spend_cents = 0", []);
        assert!(updated.is_err(), "charges are append-only");
        let deleted = db.conn().execute("DELETE FROM disclosure_charges", []);
        assert!(deleted.is_err(), "charges are append-only");

        assert_eq!(consumption(&db, id, &terms).unwrap().spend_cents, 25);
    }

    #[test]
    fn superseding_a_policy_leaves_old_charges_under_the_old_terms() {
        // A charge names the bytes that were approved, so what a past
        // disclosure was permitted by does not move when the policy is
        // renewed under the same name.
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        let old = digest("aa");
        let new = digest("bb");

        approve(&db, id, &terms("legal", 1, "{}", &old), Actor::Cli).unwrap();
        charge(&db, id, &old, 500, 10).unwrap();
        approve(&db, id, &terms("legal", 2, r#"{"x":1}"#, &new), Actor::Cli).unwrap();

        assert_eq!(latest(&db, id, "legal").unwrap().expect("v2").version, 2);
        assert_eq!(consumption(&db, id, &old).unwrap().calls, 1);
        // The new terms start unspent, which is the point of approving them.
        assert_eq!(consumption(&db, id, &new).unwrap(), Spent::default());
    }

    #[test]
    fn re_approving_a_version_with_different_terms_is_refused() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        let aa = digest("aa");
        approve(&db, id, &terms("legal", 1, "{}", &aa), Actor::Cli).unwrap();
        approve(&db, id, &terms("legal", 1, "{}", &aa), Actor::Cli)
            .expect("identical terms are idempotent");

        let cc = digest("cc");
        let error = approve(
            &db,
            id,
            &terms("legal", 1, r#"{"budget":{"calls":9999}}"#, &cc),
            Actor::Cli,
        )
        .expect_err("the same version cannot mean two things");
        assert!(matches!(error, DfError::Conflict(_)));
    }
}
