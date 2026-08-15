//! Deciding a review class has to change where the plan puts the files.
//!
//! This is the property the whole review workflow rests on, and nothing tested
//! it. `review classes` collapses a queue of thousands into a handful of
//! questions and `review decide-batch` answers one in a single call — but both
//! are pointless if the answer does not actually move the files out of
//! `90_DataForge_Review` in the resulting plan.
//!
//! Over a real archive that gap is the difference between a 443.9 GB output
//! with 82% of its files parked in a review bucket and an output that is
//! actually organised.

use std::path::{Path, PathBuf};

use df_db::{analysis, repository, Db};
use df_domain::{
    Actor, DuplicatePolicy, OperationType, ProfileRef, Project, RuleAction, SourceRoot,
};
use df_hash::{hash_project, HashOptions};
use df_planner::{analyze_project, create_plan};
use df_scan::{scan_project, ScanOptions};

/// A project whose analysis produces exactly one review item: the generic
/// profile's `review.backup-extension` rule routes `*.bak` to review.
fn analysed_project(tmp: &Path, name: &str) -> Db {
    let origin = tmp.join(name).join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::write(origin.join("datos.bak"), b"respaldo antiguo").unwrap();
    std::fs::write(origin.join("informe.txt"), b"contenido normal").unwrap();

    let mut db = Db::open(&tmp.join(name).join("state.sqlite")).unwrap();
    let project = Project::new(
        "Enrutado por clase",
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

/// Destination of the one occurrence whose name we control.
fn destination_of(db: &Db, file_name: &str) -> (OperationType, PathBuf) {
    let project = repository::load_project(db).unwrap();
    let plan = df_db::plans::current_plan(db, project.id)
        .unwrap()
        .expect("the project has a plan");
    let operation = df_db::plans::list_operations(db, plan.id)
        .unwrap()
        .into_iter()
        .find(|operation| {
            operation
                .destination_relative_path
                .as_deref()
                .is_some_and(|path| path.ends_with(file_name))
        })
        .unwrap_or_else(|| panic!("no operation lands `{file_name}`"));
    (
        operation.operation_type,
        PathBuf::from(operation.destination_relative_path.unwrap()),
    )
}

#[test]
fn deciding_a_review_class_moves_its_files_out_of_the_review_bucket() {
    let tmp = tempfile::tempdir().unwrap();

    // Two projects over an identical origin. The only difference between them
    // is whether the review class was decided before planning, so any change
    // in routing is attributable to the decision and to nothing else.
    let mut undecided = analysed_project(tmp.path(), "sin-decidir");
    let mut decided = analysed_project(tmp.path(), "decidido");

    // --- Undecided: the backup file sits in the review bucket -------------
    let outcome = create_plan(&mut undecided, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
    assert_eq!(
        outcome.review_copies, 1,
        "an undecided review item must route to review"
    );
    let (operation, path) = destination_of(&undecided, "datos.bak");
    assert_eq!(operation, OperationType::CopyReview);
    assert!(
        path.starts_with("90_DataForge_Review"),
        "expected the review bucket, got `{}`",
        path.display()
    );

    // --- Decided as a class, by an agent ---------------------------------
    let project = repository::load_project(&decided).unwrap();
    let snapshot = df_db::inventory::latest_complete_snapshot(&decided, project.id)
        .unwrap()
        .unwrap();
    let classes = analysis::review_class_summary(&decided, snapshot.id).unwrap();
    let class = classes
        .classes
        .iter()
        .find(|class| class.kind == "review.backup-extension")
        .expect("the backup rule forms a class");
    assert_eq!(class.pending, 1);

    // The class summary hands back an item id precisely so a caller can
    // decide the class without listing the queue.
    let item_id = class
        .sample_item_id
        .clone()
        .expect("a pending class exposes an item to act on");
    analysis::decide_review_items(
        &mut decided,
        project.id,
        &[analysis::ReviewDecisionInput {
            item_id,
            decision: RuleAction::CopyActive,
            rationale: "respaldo revisado: es material vivo".to_string(),
        }],
        Actor::Agent,
    )
    .unwrap();

    // --- Decided: the same file now lands in the working archive ----------
    let outcome = create_plan(&mut decided, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();
    assert_eq!(
        outcome.review_copies, 0,
        "a decided class must leave nothing in review"
    );
    let (operation, path) = destination_of(&decided, "datos.bak");
    assert_eq!(operation, OperationType::CopyActive);
    assert!(
        !path.starts_with("90_DataForge_Review"),
        "a decided item must not stay in the review bucket, got `{}`",
        path.display()
    );

    // The decision has to be attributable in the chained ledger, not just in
    // the decisions table: an organised output nobody can account for is not
    // what this engine promises.
    let decisions: Vec<Actor> = repository::list_events(&decided, project.id)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "REVIEW_DECIDED")
        .map(|event| event.actor)
        .collect();
    assert_eq!(decisions, vec![Actor::Agent]);
}

/// The undecided plan is the baseline the definition of done measures against,
/// so it must not drift silently: an unanswered question keeps the file, in
/// the review bucket, under its original relative path.
#[test]
fn an_undecided_class_still_copies_every_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = analysed_project(tmp.path(), "cobertura");
    let outcome = create_plan(&mut db, Actor::Test, DuplicatePolicy::ReportOnly).unwrap();

    // Two files in, two copies out: review is a routing decision, never a
    // reason to drop anything.
    assert_eq!(outcome.copies, 2);
    assert_eq!(outcome.blocked, 0);
    assert_eq!(outcome.no_action, 0);

    let (_, review_path) = destination_of(&db, "datos.bak");
    let (_, active_path) = destination_of(&db, "informe.txt");
    // Same relative path under both roots: accepting a review later is a move
    // between roots, not a re-derivation of the path.
    assert_eq!(
        review_path.strip_prefix("90_DataForge_Review").unwrap(),
        active_path.parent().unwrap().join("datos.bak")
    );
}
