//! What a consolidating policy actually saves — and what it refuses to touch.
//!
//! The consolidating policies have shipped since 1.0 and nothing measured
//! them. Doing so turns up a result worth pinning down, because it is easy to
//! assume the opposite: **`CONSOLIDATE_ALL` does not consolidate duplicates
//! that live in different folders.**
//!
//! That is deliberate. `classify_duplicate_set` only claims
//! `WithinSameContext` when every copy provably shares one folder of one
//! source root; without the entity graph, that is the only "same context" the
//! engine can demonstrate. Everything else stays `UnknownContext`, and §15.2
//! forbids inferring redundancy — so `decide()` copies it whatever the policy
//! says.
//!
//! The consequence is measurable and it shapes the roadmap. On the archive
//! this was built against, of 28.537 duplicate sets only 625 keep all their
//! copies in one folder: 5,45 GB of the 239,7 GB of redundancy is reachable by
//! any 1.0 policy. The remaining 234,2 GB is not a policy choice, it is a
//! missing classification — which is why context classification is a
//! precondition for deduplication rather than a tidiness feature.

use std::path::Path;

use df_db::{plans, repository, Db};
use df_domain::{Actor, DuplicatePolicy, OperationType, ProfileRef, Project, SourceRoot};
use df_hash::{hash_project, HashOptions};
use df_planner::{analyze_project, create_plan, PlanOutcome};
use df_scan::{scan_project, ScanOptions};

const SHARED: &[u8] = b"exactly the same forty-eight bytes in every copy";
const UNIQUE: &[u8] = b"a different payload nobody duplicates at all";

/// Build and analyse a project over the files `layout` creates.
fn analysed_project(tmp: &Path, name: &str, layout: impl Fn(&Path)) -> Db {
    let origin = tmp.join(name).join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    layout(&origin);

    let mut db = Db::open(&tmp.join(name).join("state.sqlite")).unwrap();
    let project = Project::new(
        "Ahorro por consolidación",
        ProfileRef::default(),
        tmp.join(name).join("salida"),
        tmp.join(name).join("auditoria"),
        "test",
    );
    let roots = vec![SourceRoot::new(project.id, origin)];
    repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
    scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
    hash_project(&mut db, Actor::Test, &HashOptions::default(), None).unwrap();
    analyze_project(&mut db, Actor::Test).unwrap();
    db
}

/// Bytes the plan would write, from the same aggregation the CLI and the
/// desktop preview use, so the number asserted here is the number a user sees.
fn planned_bytes(db: &Db) -> u64 {
    let project = repository::load_project(db).unwrap();
    let plan = plans::current_plan(db, project.id).unwrap().unwrap();
    plans::destination_tree(db, plan.id, 1).unwrap().bytes
}

/// Every occurrence must be accounted for, whatever the policy.
fn covered(outcome: &PlanOutcome) -> u64 {
    outcome.copies + outcome.skipped_represented + outcome.no_action + outcome.blocked
}

/// Two copies in one folder are provably the same context, so consolidation
/// is allowed to represent one with the other.
#[test]
fn duplicates_in_one_folder_consolidate_and_the_saving_is_exact() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = |origin: &Path| {
        std::fs::write(origin.join("informe.txt"), SHARED).unwrap();
        std::fs::write(origin.join("informe-copia.txt"), SHARED).unwrap();
        std::fs::write(origin.join("otro.txt"), UNIQUE).unwrap();
    };
    let mut reporting = analysed_project(tmp.path(), "misma-informar", layout);
    let mut consolidating = analysed_project(tmp.path(), "misma-consolidar", layout);

    let report_only =
        create_plan(&mut reporting, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
    let consolidated = create_plan(
        &mut consolidating,
        Actor::Test,
        DuplicatePolicy::ConsolidateAll,
    )
    .unwrap();

    assert_eq!(covered(&report_only), 3);
    assert_eq!(covered(&consolidated), 3, "consolidation drops nothing");

    assert_eq!(report_only.copies, 3);
    assert_eq!(report_only.skipped_represented, 0);
    assert_eq!(consolidated.copies, 2);
    assert_eq!(consolidated.skipped_represented, 1);

    let saved = planned_bytes(&reporting) - planned_bytes(&consolidating);
    assert_eq!(
        saved,
        SHARED.len() as u64,
        "the saving is exactly the one redundant copy, nothing more"
    );
}

/// The result that is easy to get wrong: copies spread across folders are an
/// unknown context, and **no** policy consolidates those. This is the safety
/// rule of §15.2 — the engine will not infer that a copy somewhere else is
/// redundant — and it is why a consolidating policy alone cannot deduplicate a
/// real archive.
#[test]
fn duplicates_across_folders_are_preserved_even_by_consolidate_all() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = |origin: &Path| {
        std::fs::create_dir_all(origin.join("carpeta-a")).unwrap();
        std::fs::create_dir_all(origin.join("carpeta-b")).unwrap();
        std::fs::write(origin.join("carpeta-a").join("informe.txt"), SHARED).unwrap();
        std::fs::write(origin.join("carpeta-b").join("informe.txt"), SHARED).unwrap();
        std::fs::write(origin.join("otro.txt"), UNIQUE).unwrap();
    };
    let mut reporting = analysed_project(tmp.path(), "cruz-informar", layout);
    let mut consolidating = analysed_project(tmp.path(), "cruz-consolidar", layout);

    let report_only =
        create_plan(&mut reporting, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
    let consolidated = create_plan(
        &mut consolidating,
        Actor::Test,
        DuplicatePolicy::ConsolidateAll,
    )
    .unwrap();

    assert_eq!(
        consolidated.copies, report_only.copies,
        "an unknown context is copied under every policy"
    );
    assert_eq!(
        consolidated.skipped_represented, 0,
        "CONSOLIDATE_ALL consolidates nothing across folders"
    );
    assert_eq!(
        planned_bytes(&consolidating),
        planned_bytes(&reporting),
        "no bytes are saved, because no redundancy was proven"
    );
    assert_eq!(covered(&consolidated), 3);
}

/// The source is never the thing that gets shortened. "Not copied" and
/// "deleted" are the same word to a worried user, so the reason says which.
#[test]
fn a_represented_duplicate_is_not_copied_and_the_source_keeps_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = analysed_project(tmp.path(), "origen-intacto", |origin| {
        std::fs::write(origin.join("informe.txt"), SHARED).unwrap();
        std::fs::write(origin.join("informe-copia.txt"), SHARED).unwrap();
    });
    create_plan(&mut db, Actor::Test, DuplicatePolicy::ConsolidateAll).unwrap();

    let project = repository::load_project(&db).unwrap();
    let plan = plans::current_plan(&db, project.id).unwrap().unwrap();
    let represented: Vec<_> = plans::list_operations(&db, plan.id)
        .unwrap()
        .into_iter()
        .filter(|operation| operation.operation_type == OperationType::SkipRepresented)
        .collect();

    assert_eq!(represented.len(), 1);
    let operation = &represented[0];
    assert!(
        operation.destination_relative_path.is_none(),
        "a represented duplicate has no destination: nothing is written for it"
    );
    assert!(
        operation.reason.contains("the source keeps it"),
        "the reason must say the origin is untouched, got `{}`",
        operation.reason
    );

    let origin = tmp.path().join("origen-intacto").join("origen");
    for relative in ["informe.txt", "informe-copia.txt"] {
        assert!(
            origin.join(relative).exists(),
            "`{relative}` must still exist in the source"
        );
    }
}
