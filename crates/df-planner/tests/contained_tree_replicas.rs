//! Using a proof the engine already recorded (ADR-0045).
//!
//! `classify_duplicate_set` decided a set's kind from folder equality alone,
//! so a copy living in a subtree the engine had **proved** holds nothing of
//! its own still came out `UnknownContext` — and §15.2 forbids inferring
//! redundancy, so no policy touched it.
//!
//! That proof is a `TREE_EMBEDDED` relation, whose `CHECK` enforces
//! `unique_files = 0` on the contained side: every distinct content of that
//! subtree also exists in the outer one. Refusing to use it is not prudence,
//! it is declining to read the evidence already on file. On the archive this
//! was built against it is 708 folders and 67.648 files — around half of the
//! redundancy no 1.0 policy could reach.
//!
//! These tests pin what the classification guarantees — the relation is
//! recorded, the surviving copy is always outside the contained subtree, a
//! partial clone is never mistaken for one, consolidation stays opt-in — and
//! one of them, `a_recommendation_still_overrides_the_proof`, pins the reason
//! none of it takes effect yet. Read that one first: ADR-0045's decision 1,
//! implemented exactly as written, changes nothing observable, and the ADR now
//! records why.

use std::path::Path;

use df_db::{analysis, dedup, plans, repository, structure, Db};
use df_domain::{
    Actor, DuplicateKind, DuplicatePolicy, OperationType, ProfileRef, Project, RuleAction,
    SourceRoot,
};
use df_hash::{hash_project, HashOptions};
use df_planner::{analyze_project, create_plan};
use df_scan::{scan_project, ScanOptions};

/// `copia/` holds two contents, both of which also live in `base/`, which
/// holds a third. That is `TREE_EMBEDDED`: the contained side has nothing of
/// its own. Two contents is the detector's floor (`min_subtree_contents`).
fn embedded_project(tmp: &Path) -> Db {
    let origin = tmp.join("origen");
    std::fs::create_dir_all(origin.join("base")).unwrap();
    std::fs::create_dir_all(origin.join("copia")).unwrap();
    std::fs::write(origin.join("base").join("a.txt"), b"contenido A").unwrap();
    std::fs::write(origin.join("base").join("b.txt"), b"contenido B").unwrap();
    std::fs::write(origin.join("base").join("c.txt"), b"contenido C propio").unwrap();
    std::fs::write(origin.join("copia").join("a.txt"), b"contenido A").unwrap();
    std::fs::write(origin.join("copia").join("b.txt"), b"contenido B").unwrap();

    let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
    let project = Project::new(
        "Réplica en árbol contenido",
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
    db
}

/// Answer every pending structural review item as "this is live material".
///
/// An embedded relation raises a review item of its own, and while it is
/// pending the planner routes *every* file in both folders to the review
/// bucket — which is exactly the state ADR-0045 describes on the real archive:
/// 3.702 review items dragging 129.379 copies, 63% of the volume parked.
///
/// So ADR-0045 does not empty the review bucket by itself; it makes the
/// consolidation possible **once the class is decided**, which is what
/// `review decide-batch` exists to do in one call. These tests take that step
/// explicitly rather than hiding it, because it is the real workflow.
fn decide_pending_reviews(db: &mut Db) {
    let project = repository::load_project(db).unwrap();
    let snapshot_id = snapshot(db);
    let queue = analysis::review_queue(db, snapshot_id).unwrap();
    let decisions: Vec<_> = queue
        .items
        .iter()
        .filter(|item| item.status == "PENDING")
        .map(|item| analysis::ReviewDecisionInput {
            item_id: item.id.clone(),
            decision: RuleAction::CopyActive,
            rationale: "árbol embebido revisado: material vivo".to_string(),
        })
        .collect();
    if !decisions.is_empty() {
        analysis::decide_review_items(db, project.id, &decisions, Actor::Agent).unwrap();
    }
}

fn snapshot(db: &Db) -> df_domain::SnapshotId {
    let project = repository::load_project(db).unwrap();
    df_db::inventory::latest_complete_snapshot(db, project.id)
        .unwrap()
        .unwrap()
        .id
}

fn operations(db: &Db) -> Vec<df_domain::PlanOperation> {
    let project = repository::load_project(db).unwrap();
    let plan = plans::current_plan(db, project.id).unwrap().unwrap();
    plans::list_operations(db, plan.id).unwrap()
}

#[test]
fn the_engine_records_the_contained_side_of_an_embedded_relation() {
    // The premise of everything below. If the detector stops producing this
    // relation for this layout, the other tests would pass vacuously.
    let tmp = tempfile::tempdir().unwrap();
    let db = embedded_project(tmp.path());

    let contained = structure::contained_embedded_folders(&db, snapshot(&db)).unwrap();
    assert!(
        contained
            .iter()
            .any(|folder| folder.relative_path.ends_with("copia")),
        "`copia` holds nothing `base` does not; it must be recorded as contained: {contained:?}"
    );
    assert!(
        !contained
            .iter()
            .any(|folder| folder.relative_path.ends_with("base")),
        "`base` has content of its own and must never be the contained side"
    );
}

#[test]
fn a_recommendation_still_overrides_the_proof() {
    // **This is the finding, and it is why ADR-0045 as written changes
    // nothing on its own.**
    //
    // A `TREE_EMBEDDED` relation raises a structural review item covering
    // every file in both folders. While it is pending, everything routes to
    // the review bucket. Once decided, the decision itself becomes a
    // recommendation — and the planner deliberately lets a recommendation
    // override duplicate consolidation, so "an ambiguous occurrence is not
    // silently represented by another path before the user decides".
    //
    // The classification below is therefore computed correctly and never
    // consulted for these occurrences. Reaching the consolidation needs a
    // decision about that precedence which ADR-0045 does not make, so it is
    // not made here either. This test exists so the gap is recorded rather
    // than rediscovered.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = embedded_project(tmp.path());
    decide_pending_reviews(&mut db);
    create_plan(&mut db, Actor::Test, DuplicatePolicy::ConsolidateAll).unwrap();

    let copied: Vec<_> = operations(&db)
        .into_iter()
        .filter(|operation| operation.operation_type == OperationType::CopyActive)
        .collect();
    assert_eq!(
        copied.len(),
        5,
        "today every occurrence is copied: the decided review recommendation \
         wins over the duplicate disposition"
    );
    assert!(
        copied
            .iter()
            .all(|operation| operation.reason.contains("human review decided")),
        "and the reason shows why: the recommendation is what drove the routing"
    );
}

#[test]
fn the_representative_of_a_contained_replica_never_lives_inside_it() {
    // The property that makes the classification safe, checked on the data
    // rather than through the planner: the copy that would survive sits
    // outside the contained subtree. If it did not, dropping the replicas
    // would drop the only copy there is.
    let tmp = tempfile::tempdir().unwrap();
    let db = embedded_project(tmp.path());
    let snapshot_id = snapshot(&db);

    let contained = structure::contained_embedded_folders(&db, snapshot_id).unwrap();
    let members = dedup::duplicate_members(&db, snapshot_id).unwrap();
    assert!(!members.is_empty(), "a.txt and b.txt are duplicated");

    let inside = |member: &df_db::dedup::DuplicateMember| {
        contained.iter().any(|folder| {
            folder.source_root_id == member.source_root_id
                && member.parent_relative_path == folder.relative_path
        })
    };
    let representatives: Vec<_> = members.iter().filter(|m| m.is_representative).collect();
    assert!(
        !representatives.is_empty(),
        "every set has a representative"
    );
    for representative in representatives {
        assert!(
            !inside(representative),
            "the representative must live outside the contained subtree: {:?}",
            representative.parent_relative_path
        );
    }
    assert!(
        members.iter().filter(|m| !m.is_representative).all(inside),
        "and every replica inside it"
    );
}

#[test]
fn consolidation_stays_opt_in() {
    // ADR-0045 §4: what changes is that consolidating policies have something
    // to act on, not that anything acts unasked. REPORT_ONLY is the default
    // and must keep producing the same output, byte for byte.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = embedded_project(tmp.path());
    decide_pending_reviews(&mut db);
    create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();

    let skipped = operations(&db)
        .into_iter()
        .filter(|operation| operation.operation_type == OperationType::SkipRepresented)
        .count();
    assert_eq!(
        skipped, 0,
        "the default policy copies everything and only reports the evidence"
    );
}

#[test]
fn a_partial_clone_is_never_treated_as_contained() {
    // Both sides hold something the other does not, so neither may be
    // dropped for the other (§19.4). This is the distinction the whole ADR
    // rests on, and reading it wrong is how you lose a file.
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origen");
    std::fs::create_dir_all(origin.join("izquierda")).unwrap();
    std::fs::create_dir_all(origin.join("derecha")).unwrap();
    std::fs::write(origin.join("izquierda").join("comun1.txt"), b"comun uno").unwrap();
    std::fs::write(origin.join("izquierda").join("comun2.txt"), b"comun dos").unwrap();
    std::fs::write(
        origin.join("izquierda").join("solo-izq.txt"),
        b"solo izquierda",
    )
    .unwrap();
    std::fs::write(origin.join("derecha").join("comun1.txt"), b"comun uno").unwrap();
    std::fs::write(origin.join("derecha").join("comun2.txt"), b"comun dos").unwrap();
    std::fs::write(origin.join("derecha").join("solo-der.txt"), b"solo derecha").unwrap();

    let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
    let project = Project::new(
        "Clon parcial",
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

    let contained = structure::contained_embedded_folders(&db, snapshot(&db)).unwrap();
    assert!(
        contained.is_empty(),
        "neither side is contained in the other: {contained:?}"
    );

    decide_pending_reviews(&mut db);
    create_plan(&mut db, Actor::Test, DuplicatePolicy::ConsolidateAll).unwrap();
    let skipped = operations(&db)
        .into_iter()
        .filter(|operation| operation.operation_type == OperationType::SkipRepresented)
        .count();
    // Six files, four distinct contents, and nothing may be dropped: each
    // side holds something the other does not.
    assert_eq!(
        skipped, 0,
        "a partial clone must keep every occurrence on both sides"
    );
}

#[test]
fn the_kind_round_trips_through_its_wire_name() {
    // It is persisted, so a name that does not survive the round trip would
    // surface as a corrupt plan rather than as a compile error.
    assert_eq!(
        DuplicateKind::parse("CONTAINED_TREE_REPLICA").unwrap(),
        DuplicateKind::ContainedTreeReplica
    );
    assert_eq!(
        DuplicateKind::ContainedTreeReplica.as_str(),
        "CONTAINED_TREE_REPLICA"
    );
}
