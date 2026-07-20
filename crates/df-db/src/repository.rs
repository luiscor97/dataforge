//! Repositories: the only write paths into the database.
//!
//! Every mutation runs in a transaction and appends its audit event inside
//! that same transaction, so state and ledger can never diverge.

use std::path::PathBuf;
use std::str::FromStr;

use df_domain::{
    Actor, AuditEvent, EventId, FileSystemKind, ProfileRef, Project, ProjectId, ProjectState,
    SourceRoot, SourceRootId, Timestamp,
};
use df_error::{DfError, DfResult};
use rusqlite::{params, Transaction};

use crate::{db_err, Db};

pub(crate) fn to_stored_timestamp(ts: Timestamp) -> String {
    df_ledger::canonical_timestamp(ts)
}

pub(crate) fn parse_stored_timestamp(value: &str) -> DfResult<Timestamp> {
    value
        .parse::<Timestamp>()
        .map_err(|e| DfError::Serialization(format!("invalid stored timestamp `{value}`: {e}")))
}

/// Event types emitted by this crate.
pub const EVENT_PROJECT_CREATED: &str = "PROJECT_CREATED";
pub const EVENT_STATE_CHANGED: &str = "STATE_CHANGED";

fn last_chain_link(tx: &Transaction<'_>, project_id: ProjectId) -> DfResult<(u64, String)> {
    let result = tx
        .query_row(
            "SELECT sequence, event_hash FROM audit_events
             WHERE project_id = ?1 ORDER BY sequence DESC LIMIT 1",
            [project_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(db_err(other)),
        })?;
    Ok(match result {
        Some((sequence, hash)) => (sequence as u64, hash),
        None => (0, df_ledger::GENESIS_HASH.to_string()),
    })
}

fn insert_event(tx: &Transaction<'_>, event: &AuditEvent) -> DfResult<()> {
    tx.execute(
        "INSERT INTO audit_events
            (id, project_id, sequence, timestamp, previous_hash, event_type,
             payload_json, payload_hash, actor, event_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event.id.to_string(),
            event.project_id.to_string(),
            event.sequence as i64,
            to_stored_timestamp(event.timestamp),
            event.previous_hash,
            event.event_type,
            event.payload_json,
            event.payload_hash,
            event.actor.as_str(),
            event.event_hash,
            to_stored_timestamp(chrono::Utc::now()),
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Append an audit event as part of an ongoing transaction.
pub fn append_event(
    tx: &Transaction<'_>,
    project_id: ProjectId,
    event_type: &str,
    payload: &serde_json::Value,
    actor: Actor,
) -> DfResult<AuditEvent> {
    let (last_sequence, previous_hash) = last_chain_link(tx, project_id)?;
    let event = df_ledger::build_event(
        project_id,
        last_sequence + 1,
        &previous_hash,
        event_type,
        payload,
        actor,
    )?;
    insert_event(tx, &event)?;
    Ok(event)
}

/// Persist a freshly created project with its source roots, emitting the
/// `PROJECT_CREATED` genesis event — all in one transaction.
pub fn create_project(
    db: &mut Db,
    project: &Project,
    roots: &[SourceRoot],
    actor: Actor,
) -> DfResult<()> {
    if project.state != ProjectState::Created {
        return Err(DfError::Validation(format!(
            "new projects must be persisted in CREATED state, got {}",
            project.state
        )));
    }
    let existing: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .map_err(db_err)?;
    if existing > 0 {
        return Err(DfError::Conflict(
            "this database already contains a project".to_string(),
        ));
    }

    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO projects
            (id, name, state, profile, output_root, audit_root, app_version,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            project.id.to_string(),
            project.name,
            project.state.as_str(),
            project.profile.as_str(),
            project.output_root.to_string_lossy(),
            project.audit_root.to_string_lossy(),
            project.app_version,
            to_stored_timestamp(project.created_at),
            to_stored_timestamp(project.updated_at),
        ],
    )
    .map_err(db_err)?;

    for root in roots {
        tx.execute(
            "INSERT INTO source_roots
                (id, project_id, absolute_path, volume_id, filesystem,
                 is_network, is_removable, read_only_policy, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                root.id.to_string(),
                root.project_id.to_string(),
                root.absolute_path.to_string_lossy(),
                root.volume_id,
                root.filesystem.as_str(),
                root.is_network as i64,
                root.is_removable as i64,
                root.read_only_policy as i64,
                to_stored_timestamp(chrono::Utc::now()),
            ],
        )
        .map_err(db_err)?;
    }

    let payload = serde_json::json!({
        "project_id": project.id.to_string(),
        "name": project.name,
        "profile": project.profile.as_str(),
        "state": project.state.as_str(),
        "output_root": project.output_root.to_string_lossy(),
        "audit_root": project.audit_root.to_string_lossy(),
        "source_roots": roots
            .iter()
            .map(|r| r.absolute_path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "app_version": project.app_version,
    });
    append_event(&tx, project.id, EVENT_PROJECT_CREATED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// Load the single project stored in this database.
pub fn load_project(db: &Db) -> DfResult<Project> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, name, state, profile, output_root, audit_root,
                    app_version, created_at, updated_at
             FROM projects",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<Project>> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (id, name, state, profile, output_root, audit_root, app_version, created, updated) =
                raw.map_err(db_err)?;
            Ok(Project {
                id: ProjectId::from_str(&id)?,
                name,
                state: ProjectState::parse(&state)?,
                profile: ProfileRef::new(profile),
                source_roots: Vec::new(),
                output_root: PathBuf::from(output_root),
                audit_root: PathBuf::from(audit_root),
                created_at: parse_stored_timestamp(&created)?,
                updated_at: parse_stored_timestamp(&updated)?,
                app_version,
            })
        })
        .collect();

    let mut projects = rows.into_iter().collect::<DfResult<Vec<_>>>()?;
    match projects.len() {
        0 => Err(DfError::NotFound("no project in this database".to_string())),
        1 => {
            let mut project = projects.remove(0);
            project.source_roots = load_source_roots(db, project.id)?
                .into_iter()
                .map(|r| r.id)
                .collect();
            Ok(project)
        }
        n => Err(DfError::Conflict(format!(
            "expected a single project per database, found {n}"
        ))),
    }
}

/// Load the source roots of a project.
/// Persist the filesystem classification captured during validation
/// (ADR-0036). The column is operational metadata of the root, not
/// snapshot evidence, so re-validation may refresh it.
pub fn update_source_root_filesystem(
    db: &mut Db,
    root_id: df_domain::SourceRootId,
    kind: df_domain::FileSystemKind,
) -> DfResult<()> {
    db.conn()
        .execute(
            "UPDATE source_roots SET filesystem = ?1 WHERE id = ?2",
            rusqlite::params![kind.as_str(), root_id.to_string()],
        )
        .map_err(db_err)?;
    Ok(())
}

pub fn load_source_roots(db: &Db, project_id: ProjectId) -> DfResult<Vec<SourceRoot>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, project_id, absolute_path, volume_id, filesystem,
                    is_network, is_removable, read_only_policy
             FROM source_roots WHERE project_id = ?1 ORDER BY created_at, id",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<SourceRoot>> = stmt
        .query_map([project_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (id, project, path, volume_id, fs, is_network, is_removable, read_only) =
                raw.map_err(db_err)?;
            Ok(SourceRoot {
                id: SourceRootId::from_str(&id)?,
                project_id: ProjectId::from_str(&project)?,
                absolute_path: PathBuf::from(path),
                volume_id,
                filesystem: FileSystemKind::parse(&fs)?,
                is_network: is_network != 0,
                is_removable: is_removable != 0,
                read_only_policy: read_only != 0,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// Apply a validated state machine transition and record it in the ledger,
/// atomically.
pub fn update_project_state(db: &mut Db, next: ProjectState, actor: Actor) -> DfResult<Project> {
    let mut project = load_project(db)?;
    let from = project.state;
    project.transition_to(next)?;

    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "UPDATE projects SET state = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            project.state.as_str(),
            to_stored_timestamp(project.updated_at),
            project.id.to_string(),
        ],
    )
    .map_err(db_err)?;
    let payload = serde_json::json!({
        "project_id": project.id.to_string(),
        "from": from.as_str(),
        "to": project.state.as_str(),
    });
    append_event(&tx, project.id, EVENT_STATE_CHANGED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(project)
}

/// All events of a project, ordered by sequence.
pub fn list_events(db: &Db, project_id: ProjectId) -> DfResult<Vec<AuditEvent>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, project_id, sequence, timestamp, previous_hash, event_type,
                    payload_json, payload_hash, actor, event_hash
             FROM audit_events WHERE project_id = ?1 ORDER BY sequence",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<AuditEvent>> = stmt
        .query_map([project_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (
                id,
                project,
                sequence,
                timestamp,
                prev,
                event_type,
                payload,
                payload_hash,
                actor,
                hash,
            ) = raw.map_err(db_err)?;
            Ok(AuditEvent {
                id: EventId::from_str(&id)?,
                project_id: ProjectId::from_str(&project)?,
                sequence: sequence as u64,
                timestamp: parse_stored_timestamp(&timestamp)?,
                previous_hash: prev,
                event_type,
                payload_json: payload,
                payload_hash,
                actor: Actor::parse(&actor)?,
                event_hash: hash,
            })
        })
        .collect();
    rows.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project() -> Project {
        Project::new(
            "Proyecto de prueba",
            ProfileRef::default(),
            PathBuf::from("D:/salida"),
            PathBuf::from("D:/auditoria"),
            "0.0.1-dev",
        )
    }

    fn create_sample(db: &mut Db) -> Project {
        let project = sample_project();
        let roots = vec![SourceRoot::new(project.id, PathBuf::from("D:/origen"))];
        create_project(db, &project, &roots, Actor::Test).expect("create");
        load_project(db).expect("load")
    }

    #[test]
    fn create_and_load_round_trip() {
        let mut db = Db::open_in_memory().unwrap();
        let loaded = create_sample(&mut db);
        assert_eq!(loaded.name, "Proyecto de prueba");
        assert_eq!(loaded.state, ProjectState::Created);
        assert_eq!(loaded.source_roots.len(), 1);

        let roots = load_source_roots(&db, loaded.id).unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].read_only_policy);

        let events = list_events(&db, loaded.id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EVENT_PROJECT_CREATED);
        df_ledger::verify_chain(&events).expect("genesis chain verifies");
    }

    #[test]
    fn a_database_holds_exactly_one_project() {
        let mut db = Db::open_in_memory().unwrap();
        create_sample(&mut db);
        let another = sample_project();
        let err = create_project(&mut db, &another, &[], Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::Conflict(_)));
    }

    #[test]
    fn create_project_is_atomic_when_a_root_violates_constraints() {
        let mut db = Db::open_in_memory().unwrap();
        let project = sample_project();
        // Root pointing at a project id that does not exist → FK violation.
        let bad_root = SourceRoot::new(ProjectId::new(), PathBuf::from("D:/origen"));
        let err = create_project(&mut db, &project, &[bad_root], Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::Database(_)), "{err}");
        // Nothing must have been committed.
        assert!(matches!(load_project(&db), Err(DfError::NotFound(_))));
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM audit_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn state_updates_append_chained_events() {
        let mut db = Db::open_in_memory().unwrap();
        let project = create_sample(&mut db);

        update_project_state(&mut db, ProjectState::Validating, Actor::Test).unwrap();
        let updated = update_project_state(&mut db, ProjectState::Ready, Actor::Test).unwrap();
        assert_eq!(updated.state, ProjectState::Ready);

        let events = list_events(&db, project.id).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].event_type, EVENT_STATE_CHANGED);
        assert_eq!(events[2].sequence, 3);
        df_ledger::verify_chain(&events).expect("chain verifies after updates");
    }

    #[test]
    fn invalid_transition_changes_nothing() {
        let mut db = Db::open_in_memory().unwrap();
        let project = create_sample(&mut db);
        let err = update_project_state(&mut db, ProjectState::Executing, Actor::Test).unwrap_err();
        assert!(matches!(err, DfError::InvalidTransition { .. }));
        let reloaded = load_project(&db).unwrap();
        assert_eq!(reloaded.state, ProjectState::Created);
        assert_eq!(list_events(&db, project.id).unwrap().len(), 1);
    }

    #[test]
    fn audit_events_reject_update_and_delete() {
        let mut db = Db::open_in_memory().unwrap();
        let project = create_sample(&mut db);
        let update = db.conn().execute(
            "UPDATE audit_events SET event_type = 'FORGED' WHERE project_id = ?1",
            [project.id.to_string()],
        );
        assert!(update.is_err(), "append-only trigger must block UPDATE");
        let delete = db.conn().execute("DELETE FROM audit_events", []);
        assert!(delete.is_err(), "append-only trigger must block DELETE");
    }

    #[test]
    fn events_survive_reopen_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("dataforge.sqlite");
        let project_id;
        {
            let mut db = Db::open(&db_path).unwrap();
            let project = create_sample(&mut db);
            project_id = project.id;
            update_project_state(&mut db, ProjectState::Validating, Actor::Test).unwrap();
        }
        let db = Db::open(&db_path).unwrap();
        let events = list_events(&db, project_id).unwrap();
        assert_eq!(events.len(), 2);
        df_ledger::verify_chain(&events).expect("chain verifies after reopen");
    }
}
