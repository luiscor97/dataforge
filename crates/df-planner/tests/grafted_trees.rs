//! Recovering the canonical path of a grafted subtree (ROADMAP-2.0 M2.3).
//!
//! A graft is a folder whose path ends with another folder's path: an
//! unrelated folder acquired a leading prefix when a whole tree was copied
//! under it. Stripping that prefix is what recovers where a file belongs.
//!
//! What these tests pin is the split. Two of the four cases are backed by a
//! hash that exists outside the graft, so placing them takes no judgement; the
//! other two are exactly the cases where a file is the only copy there is, or
//! where two files claim one destination. Measured on the archive this was
//! built against, that split is about 98% automatic — and the value of the
//! report is entirely in the 2%, so the test that matters most is the one
//! saying a unique content is never placed automatically.

use std::path::Path;

use df_db::{repository, structure::GraftMatch};
use df_db::{structure, Db};
use df_domain::{Actor, ProfileRef, Project, SourceRoot};
use df_hash::{hash_project, HashOptions};
use df_planner::analyze_project;
use df_scan::{scan_project, ScanOptions};

/// `curso/` swallowed a copy of `asunto/`, which is what makes `curso` a graft
/// prefix. Everything else under `curso/` is there to exercise one case each.
fn grafted_project(tmp: &Path) -> Db {
    let origin = tmp.join("origen");

    // The canonical tree.
    std::fs::create_dir_all(origin.join("asunto")).unwrap();
    std::fs::write(origin.join("asunto").join("a.txt"), b"contenido A").unwrap();
    std::fs::write(origin.join("asunto").join("b.txt"), b"contenido B").unwrap();
    // Something only the canonical tree has. Without it the two trees are
    // identical, which is an exact clone and not an embedded tree at all —
    // and the containment would be detected the other way round.
    std::fs::write(origin.join("asunto").join("c.txt"), b"contenido C propio").unwrap();

    // The graft: same tree, one level deeper, holding nothing of its own.
    std::fs::create_dir_all(origin.join("curso").join("asunto")).unwrap();
    std::fs::write(
        origin.join("curso").join("asunto").join("a.txt"),
        b"contenido A",
    )
    .unwrap();
    std::fs::write(
        origin.join("curso").join("asunto").join("b.txt"),
        b"contenido B",
    )
    .unwrap();

    // Canonical path free, but the content lives outside the graft.
    std::fs::create_dir_all(origin.join("curso").join("copia")).unwrap();
    std::fs::write(
        origin.join("curso").join("copia").join("a.txt"),
        b"contenido A",
    )
    .unwrap();

    // The only copy there is. Placing this one would be a guess.
    std::fs::create_dir_all(origin.join("curso").join("propio")).unwrap();
    std::fs::write(
        origin.join("curso").join("propio").join("u.txt"),
        b"solo aqui",
    )
    .unwrap();

    // Two files claiming one destination.
    std::fs::create_dir_all(origin.join("choque")).unwrap();
    std::fs::create_dir_all(origin.join("curso").join("choque")).unwrap();
    std::fs::write(origin.join("choque").join("x.txt"), b"version uno").unwrap();
    std::fs::write(
        origin.join("curso").join("choque").join("x.txt"),
        b"version dos",
    )
    .unwrap();

    let mut db = Db::open(&tmp.join("state.sqlite")).unwrap();
    let project = Project::new(
        "Árbol injertado",
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

fn snapshot(db: &Db) -> df_domain::SnapshotId {
    let project = repository::load_project(db).unwrap();
    df_db::inventory::latest_complete_snapshot(db, project.id)
        .unwrap()
        .unwrap()
        .id
}

#[test]
fn the_graft_prefix_is_the_accident_and_not_the_context() {
    let tmp = tempfile::tempdir().unwrap();
    let db = grafted_project(tmp.path());
    let report = structure::grafted_trees(&db, snapshot(&db)).unwrap();

    assert_eq!(report.prefixes, 1, "one folder acquired a prefix");
    let graft = &report.grafts[0];
    assert_eq!(
        graft.prefix, "curso",
        "the prefix is the leading part that is not in the canonical path"
    );
    // Everything under `curso/`, counted once each.
    assert_eq!(graft.files, 5);
    assert_eq!(report.files, 5);
}

#[test]
fn a_content_that_exists_only_inside_the_graft_is_never_automatic() {
    // The property the whole report exists for. If this ever passes by
    // accident, the 2% that needs a human silently becomes 0% and the engine
    // starts placing the only copy of something on a guess.
    let tmp = tempfile::tempdir().unwrap();
    let db = grafted_project(tmp.path());
    let report = structure::grafted_trees(&db, snapshot(&db)).unwrap();
    let graft = &report.grafts[0];

    assert_eq!(
        graft.unique_hash_not_elsewhere, 1,
        "`curso/propio/u.txt` is the only copy of its content"
    );
    assert!(!GraftMatch::UniqueHashNotElsewhere.is_automatic());
    assert!(!GraftMatch::CanonicalPathHashDiff.is_automatic());
}

#[test]
fn the_four_cases_are_told_apart() {
    let tmp = tempfile::tempdir().unwrap();
    let db = grafted_project(tmp.path());
    let report = structure::grafted_trees(&db, snapshot(&db)).unwrap();
    let graft = &report.grafts[0];

    // `curso/asunto/{a,b}.txt`: the canonical path holds this very content.
    assert_eq!(graft.canonical_path_same_hash, 2);
    // `curso/copia/a.txt`: canonical path free, content lives outside.
    assert_eq!(graft.hash_elsewhere_outside_prefix, 1);
    // `curso/propio/u.txt`: nowhere else.
    assert_eq!(graft.unique_hash_not_elsewhere, 1);
    // `curso/choque/x.txt`: `choque/x.txt` exists and differs.
    assert_eq!(graft.canonical_path_hash_diff, 1);

    assert_eq!(report.auto_placeable, 3);
    assert_eq!(report.needs_review, 2);
    assert_eq!(report.auto_placeable + report.needs_review, report.files);
}

#[test]
fn a_snapshot_without_grafts_reports_nothing_rather_than_failing() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origen");
    std::fs::create_dir_all(origin.join("uno")).unwrap();
    std::fs::write(origin.join("uno").join("a.txt"), b"a").unwrap();
    std::fs::write(origin.join("uno").join("b.txt"), b"b").unwrap();

    let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
    let project = Project::new(
        "Sin injertos",
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

    let report = structure::grafted_trees(&db, snapshot(&db)).unwrap();
    assert_eq!(report.prefixes, 0);
    assert_eq!(report.files, 0);
    assert!(report.grafts.is_empty());
}
