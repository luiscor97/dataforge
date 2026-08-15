//! Who is running a long stage, and when it last said anything.
//!
//! # This module reports evidence and never a verdict
//!
//! There is no `is_alive()` here, and its absence is the design. Whether a run
//! is alive is not something this database can know:
//!
//! - a pid means nothing on a different host, and pids are reused;
//! - a laptop that slept for a day has a stale heartbeat and a live run;
//! - a process killed one second ago has a fresh heartbeat and no run at all.
//!
//! So what is recorded is who claimed the stage, from where, when it started
//! and when it last reported progress. [`RunLiveness`] adds the one comparison
//! that *is* factual — whether the claim belongs to this very process — and
//! leaves the judgement to whoever is reading, which for the decisions that
//! matter is a person asserting that no other run is active (the same shape
//! `HashOptions::resume_interrupted` already uses).
//!
//! The alternative — a freshness threshold that declares a run dead — would
//! turn an unknowable into a number in a config file, and be wrong in exactly
//! the two cases above.

use df_domain::{Actor, ProjectId};
use df_error::DfResult;
use rusqlite::{params, OptionalExtension};

use crate::repository::{parse_stored_timestamp, to_stored_timestamp};
use crate::{db_err, Db};

/// A long stage that can claim a project while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStage {
    Scan,
    Hash,
    Analyze,
    Execute,
    Verify,
}

impl RunStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "SCAN",
            Self::Hash => "HASH",
            Self::Analyze => "ANALYZE",
            Self::Execute => "EXECUTE",
            Self::Verify => "VERIFY",
        }
    }

    /// The variant's own spelling, for tests that have to look for
    /// `RunStage::Verify` in source rather than for the stored `"VERIFY"`.
    pub fn as_str_pascal(self) -> &'static str {
        match self {
            Self::Scan => "Scan",
            Self::Hash => "Hash",
            Self::Analyze => "Analyze",
            Self::Execute => "Execute",
            Self::Verify => "Verify",
        }
    }
}

/// What the database knows about the run holding this project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunLiveness {
    pub stage: String,
    pub pid: i64,
    pub host: String,
    pub started_at: String,
    pub heartbeat_at: String,
    /// Seconds since the last heartbeat. Negative is possible and is left as
    /// it is: a clock that moved backwards is evidence too, and clamping it
    /// to zero would hide the one thing that explains a nonsensical age.
    pub heartbeat_age_seconds: i64,
    /// Whether this claim was made by the process asking. The only alive/dead
    /// question that has a factual answer: if this is true the run is *this*
    /// one, and no assertion is needed.
    pub is_this_process: bool,
    /// Whether the claim was made on the machine asking. A pid from elsewhere
    /// cannot be checked locally, so this is what says "you cannot look".
    pub is_this_host: bool,
}

fn host() -> String {
    // No dependency for this: the environment already carries it on both
    // platforms, and a wrong-but-stable name is more useful than a missing one
    // because it still tells two machines apart.
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string())
}

/// Record that this process is running `stage` on this project.
///
/// Replaces any previous claim. Taking over is not this function's decision to
/// refuse: the state machine already rejects an illegal transition, and the
/// operator's explicit resume is what authorises continuing a stage someone
/// else started.
pub fn claim(db: &mut Db, project_id: ProjectId, stage: RunStage, _actor: Actor) -> DfResult<()> {
    let now = to_stored_timestamp(chrono::Utc::now());
    db.conn()
        .execute(
            "INSERT INTO run_liveness
                (project_id, stage, pid, host, started_at, heartbeat_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(project_id) DO UPDATE SET
                stage = excluded.stage,
                pid = excluded.pid,
                host = excluded.host,
                started_at = excluded.started_at,
                heartbeat_at = excluded.heartbeat_at",
            params![
                project_id.to_string(),
                stage.as_str(),
                std::process::id() as i64,
                host(),
                now,
            ],
        )
        .map_err(db_err)?;
    Ok(())
}

/// Say that this process is still working.
///
/// Only refreshes a claim this process actually holds. A run that beat for
/// somebody else's claim would keep a dead run looking fresh forever, which is
/// worse than no heartbeat at all.
pub fn beat(db: &Db, project_id: ProjectId) -> DfResult<()> {
    db.conn()
        .execute(
            "UPDATE run_liveness
             SET heartbeat_at = ?2
             WHERE project_id = ?1 AND pid = ?3 AND host = ?4",
            params![
                project_id.to_string(),
                to_stored_timestamp(chrono::Utc::now()),
                std::process::id() as i64,
                host(),
            ],
        )
        .map_err(db_err)?;
    Ok(())
}

/// Give up the claim. Same restriction as [`beat`]: only one's own.
pub fn release(db: &Db, project_id: ProjectId) -> DfResult<()> {
    db.conn()
        .execute(
            "DELETE FROM run_liveness
             WHERE project_id = ?1 AND pid = ?2 AND host = ?3",
            params![project_id.to_string(), std::process::id() as i64, host(),],
        )
        .map_err(db_err)?;
    Ok(())
}

/// What is known about the run holding this project, if any.
pub fn liveness(db: &Db, project_id: ProjectId) -> DfResult<Option<RunLiveness>> {
    let row: Option<(String, i64, String, String, String)> = db
        .conn()
        .query_row(
            "SELECT stage, pid, host, started_at, heartbeat_at
             FROM run_liveness WHERE project_id = ?1",
            params![project_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?;

    let Some((stage, pid, claim_host, started_at, heartbeat_at)) = row else {
        return Ok(None);
    };
    let age = parse_stored_timestamp(&heartbeat_at)
        .map(|beat| (chrono::Utc::now() - beat).num_seconds())
        .unwrap_or(0);
    let this_host = host();
    Ok(Some(RunLiveness {
        stage,
        pid,
        is_this_process: pid == std::process::id() as i64 && claim_host == this_host,
        is_this_host: claim_host == this_host,
        host: claim_host,
        started_at,
        heartbeat_at,
        heartbeat_age_seconds: age,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{ProfileRef, Project};

    fn project(db: &mut Db) -> ProjectId {
        let project = Project::new(
            "Vitalidad",
            ProfileRef::default(),
            std::path::PathBuf::from("salida"),
            std::path::PathBuf::from("auditoria"),
            "test",
        );
        crate::repository::create_project(db, &project, &[], Actor::Test).unwrap();
        project.id
    }

    #[test]
    fn an_unclaimed_project_reports_nothing_rather_than_a_guess() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        assert_eq!(liveness(&db, id).unwrap(), None);
    }

    #[test]
    fn a_claim_records_who_and_where_and_says_it_is_this_process() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        claim(&mut db, id, RunStage::Hash, Actor::Test).unwrap();

        let found = liveness(&db, id).unwrap().expect("a claim");
        assert_eq!(found.stage, "HASH");
        assert_eq!(found.pid, std::process::id() as i64);
        // The one alive/dead question with a factual answer.
        assert!(found.is_this_process);
        assert!(found.is_this_host);
        assert!(found.heartbeat_age_seconds < 5);
    }

    #[test]
    fn a_claim_from_another_process_is_not_mistaken_for_ours() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        claim(&mut db, id, RunStage::Execute, Actor::Test).unwrap();
        // Somebody else's run, on this machine.
        db.conn()
            .execute("UPDATE run_liveness SET pid = pid + 1", [])
            .unwrap();

        let found = liveness(&db, id).unwrap().expect("a claim");
        assert!(!found.is_this_process, "that pid is not ours");
        assert!(found.is_this_host, "but it is checkable here");
    }

    #[test]
    fn a_heartbeat_never_refreshes_someone_elses_claim() {
        // The property that keeps the record honest. A run that could beat for
        // another claim would keep a dead one looking fresh for ever, which is
        // worse than recording no heartbeat at all.
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        claim(&mut db, id, RunStage::Execute, Actor::Test).unwrap();
        db.conn()
            .execute(
                "UPDATE run_liveness SET pid = pid + 1, heartbeat_at = '2020-01-01T00:00:00Z'",
                [],
            )
            .unwrap();

        beat(&db, id).unwrap();

        let found = liveness(&db, id).unwrap().expect("a claim");
        assert_eq!(
            found.heartbeat_at, "2020-01-01T00:00:00Z",
            "the other run's heartbeat must not move"
        );
        assert!(found.heartbeat_age_seconds > 1_000_000);
    }

    #[test]
    fn releasing_only_gives_up_our_own_claim() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        claim(&mut db, id, RunStage::Scan, Actor::Test).unwrap();
        db.conn()
            .execute("UPDATE run_liveness SET pid = pid + 1", [])
            .unwrap();

        release(&db, id).unwrap();
        assert!(
            liveness(&db, id).unwrap().is_some(),
            "another process's claim is not ours to drop"
        );
    }

    #[test]
    fn a_claim_replaces_the_previous_one_for_the_same_project() {
        let mut db = Db::open_in_memory().unwrap();
        let id = project(&mut db);
        claim(&mut db, id, RunStage::Hash, Actor::Test).unwrap();
        claim(&mut db, id, RunStage::Execute, Actor::Test).unwrap();

        let found = liveness(&db, id).unwrap().expect("a claim");
        assert_eq!(found.stage, "EXECUTE", "one long stage at a time");
    }
}
