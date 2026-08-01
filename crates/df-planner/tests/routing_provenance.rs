//! Where every operation landed, and why — as data rather than as prose.
//!
//! Until migration 0020 the only record of a routing decision was the
//! free-text `reason` ("routed to operational bucket `90_DataForge_Review`").
//! That is readable and unqueryable, so reconstructing how a destination was
//! chosen meant parsing prose — and prose nobody declared is prose that
//! drifts.
//!
//! `destination_root_id` records the declared root by its stable id
//! (ADR-0040 §3). These tests pin the three properties that make it evidence
//! rather than decoration: it is recorded, it is recorded by id and not by
//! folder name, and it is absent exactly when there is nothing to record.

use std::path::Path;

use df_db::{plans, repository, Db};
use df_domain::{Actor, DuplicatePolicy, OperationType, ProfileRef, Project, SourceRoot};
use df_hash::{hash_project, HashOptions};
use df_planner::{analyze_project, approve_plan, create_plan, DestinationTaxonomy};
use df_scan::{scan_project, ScanOptions};

fn planned_project(tmp: &Path, layout: impl Fn(&Path)) -> Db {
    let origin = tmp.join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    layout(&origin);

    let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
    let project = Project::new(
        "Procedencia de enrutado",
        ProfileRef::default(),
        tmp.join("salida"),
        tmp.join("auditoria"),
        "test",
    );
    let roots = vec![SourceRoot::new(project.id, origin)];
    repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
    scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
    hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
    analyze_project(&mut db, Actor::Test).unwrap();
    create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
    db
}

fn operations(db: &Db) -> Vec<df_domain::PlanOperation> {
    let project = repository::load_project(db).unwrap();
    let plan = plans::current_plan(db, project.id).unwrap().unwrap();
    plans::list_operations(db, plan.id).unwrap()
}

#[test]
fn every_operation_with_a_destination_records_the_root_it_landed_in() {
    let tmp = tempfile::tempdir().unwrap();
    let db = planned_project(tmp.path(), |origin| {
        std::fs::write(origin.join("informe.txt"), b"contenido").unwrap();
        std::fs::create_dir_all(origin.join("carpeta")).unwrap();
        std::fs::write(origin.join("carpeta").join("otro.txt"), b"distinto").unwrap();
    });

    let operations = operations(&db);
    assert!(!operations.is_empty(), "the plan must hold operations");

    for operation in &operations {
        match operation.destination_relative_path {
            // Landed somewhere: the root that decided where is recorded.
            Some(_) => assert!(
                operation.destination_root_id.is_some(),
                "operation {} landed at a destination with no recorded root",
                operation.sequence
            ),
            // Landed nowhere: naming a root would be inventing provenance.
            None => assert!(
                operation.destination_root_id.is_none(),
                "operation {} records a root but has no destination",
                operation.sequence
            ),
        }
    }
}

#[test]
fn the_recorded_root_is_the_one_the_taxonomy_chose() {
    let tmp = tempfile::tempdir().unwrap();
    let db = planned_project(tmp.path(), |origin| {
        std::fs::write(origin.join("informe.txt"), b"contenido").unwrap();
    });

    let profile = df_domain::Profile::load(df_domain::DEFAULT_PROFILE_ID).unwrap();
    let taxonomy = DestinationTaxonomy::from_profile(&profile);
    for operation in operations(&db) {
        let Some(recorded) = operation.destination_root_id.as_deref() else {
            continue;
        };
        let expected = taxonomy
            .root_for(operation.operation_type)
            .expect("the operational taxonomy covers every 1.x operation type");
        assert_eq!(
            recorded,
            expected.id,
            "operation {} of type {} recorded the wrong root",
            operation.sequence,
            operation.operation_type.as_str()
        );
    }
}

#[test]
fn provenance_is_recorded_by_id_not_by_folder_name() {
    // The whole point of an id: renaming a folder in a profile must not
    // rewrite the provenance of plans made before the rename. If the folder
    // name leaked into this column, a rename would silently invalidate the
    // history of every plan that used it.
    let tmp = tempfile::tempdir().unwrap();
    let db = planned_project(tmp.path(), |origin| {
        std::fs::write(origin.join("informe.txt"), b"contenido").unwrap();
    });

    let profile = df_domain::Profile::load(df_domain::DEFAULT_PROFILE_ID).unwrap();
    let folders: Vec<&str> = DestinationTaxonomy::from_profile(&profile)
        .reserved_folders()
        .collect();
    for operation in operations(&db) {
        let Some(recorded) = operation.destination_root_id.as_deref() else {
            continue;
        };
        assert!(
            !folders.contains(&recorded),
            "operation {} recorded the folder name `{recorded}` instead of a root id",
            operation.sequence
        );
    }
}

#[test]
fn an_operation_that_copies_nothing_records_no_root() {
    // SKIP_REPRESENTED covers a duplicate that the set's canonical copy
    // already represents. It has no destination, so it has no root — and
    // `None` here has to keep meaning "not recorded" rather than defaulting
    // to the active archive.
    let tmp = tempfile::tempdir().unwrap();
    let origin_tmp = tempfile::tempdir().unwrap();
    let origin = origin_tmp.path().join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    // Two byte-identical files in one folder: provably the same context, so a
    // consolidating policy may represent one with the other.
    std::fs::write(origin.join("a.txt"), b"identico").unwrap();
    std::fs::write(origin.join("b.txt"), b"identico").unwrap();

    let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
    let project = Project::new(
        "Duplicado representado",
        ProfileRef::default(),
        tmp.path().join("salida"),
        tmp.path().join("auditoria"),
        "test",
    );
    let roots = vec![SourceRoot::new(project.id, origin)];
    repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
    scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
    hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
    analyze_project(&mut db, Actor::Test).unwrap();
    create_plan(
        &mut db,
        Actor::Test,
        DuplicatePolicy::ConsolidateWithinContext,
    )
    .unwrap();

    let skipped: Vec<_> = operations(&db)
        .into_iter()
        .filter(|operation| operation.operation_type == OperationType::SkipRepresented)
        .collect();
    assert!(
        !skipped.is_empty(),
        "two identical files in one folder must produce a represented duplicate"
    );
    for operation in skipped {
        assert!(
            operation.destination_root_id.is_none(),
            "an operation that copies nothing must not claim a destination root"
        );
    }
}

#[test]
fn provenance_does_not_reach_the_frozen_manifest() {
    // Routing provenance is evidence beside the operation, exactly like
    // `reason` — not part of the manifest. Keeping it out is what lets the
    // manifest digest stay what it was in 1.x, so approving a plan built
    // before and after this migration yields the same contract.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = planned_project(tmp.path(), |origin| {
        std::fs::write(origin.join("informe.txt"), b"contenido").unwrap();
    });

    let approved = approve_plan(&mut db, Actor::Test).unwrap();
    let project = repository::load_project(&db).unwrap();
    let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
    let entries = plans::manifest(&db, plan.id).unwrap();
    assert!(!entries.is_empty(), "an approved plan has manifest entries");

    let manifest = df_ledger::canonical_json(&serde_json::json!(entries
        .iter()
        .map(|entry| entry.canonical_value())
        .collect::<Vec<_>>()));
    assert!(
        !manifest.contains("destination_root_id"),
        "the frozen manifest must not carry routing provenance"
    );
    assert!(
        !approved.serialized_sha256.is_empty(),
        "approval produces a digest"
    );
}
