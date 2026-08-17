//! Choosing the wrong profile used to be silent (RFC-0001 rule 9, ADR-0026).
//!
//! A 444 GB legal archive ran fifteen hours under `generic`. It reported
//! `Protected bounds: 0`, deduplicated across `expediente` folders — which
//! rule 9 exists to forbid, because each expediente must stand alone as an
//! evidentiary unit — and verified clean. Nothing complained, because nothing
//! was broken: `legal` declares the markers, `classify()` consumes them and
//! `m02_legal_profile` proves the boundaries survive `CONSOLIDATE_ALL`. The
//! capability was there. `--profile legal` was not, and no error says so.
//!
//! These two tests pin the signal that ends that silence: the same folders,
//! classified under each profile, and what the run says about its own fit.
//! It is a warning and never a refusal — `generic` may be the right choice,
//! and rule 9 leaves that decision with the human or the calling agent.

use std::path::Path;

use df_db::{analysis, context, inventory, repository, Db};
use df_domain::{Actor, ProfileRef, Project, SnapshotId, SourceRoot};
use df_hash::{hash_project, HashOptions};
use df_planner::analyze_project;
use df_scan::{scan_project, ScanOptions};

/// The shape the field run actually had: a container of case folders, one
/// spelled out and one abbreviated, and a container of expert reports.
///
/// Every path is composed with [`Path::join`] and no separator appears in a
/// literal, so the fixture is the same tree on Windows and on Linux.
fn field_shape_project(tmp: &Path, profile: &str) -> Db {
    let origin = tmp.join("origen");
    let expedientes = origin.join("EXPEDIENTES");
    let long_form = expedientes.join("Expediente 1234-2020");
    let short_form = expedientes.join("Exp 1234-2020");
    let periciales = origin.join("PERICIALES");
    for directory in [&expedientes, &long_form, &short_form, &periciales] {
        std::fs::create_dir_all(directory).expect("create fixture directory");
    }
    // Distinct contents: this fixture is about how folders are classified, and
    // duplicate evidence would only add noise to the counters it asserts.
    for (index, directory) in [&expedientes, &long_form, &short_form, &periciales]
        .iter()
        .enumerate()
    {
        std::fs::write(
            directory.join("escrito.txt"),
            format!("documento {index}").as_bytes(),
        )
        .expect("write fixture document");
    }

    let mut db = Db::open(&tmp.join("state.sqlite")).expect("open project database");
    let project = Project::new(
        "Archivo jurídico real",
        ProfileRef::new(profile),
        tmp.join("salida"),
        tmp.join("auditoria"),
        "test",
    );
    let roots = vec![SourceRoot::new(project.id, origin)];
    repository::create_project(&mut db, &project, &roots, Actor::Test).expect("persist project");
    let scanned =
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).expect("scan fixture");
    assert_eq!(scanned.errors, 0);
    let hashed =
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).expect("hash fixture");
    assert_eq!(hashed.failed, 0);
    db
}

fn snapshot_of(db: &Db) -> SnapshotId {
    let project = repository::load_project(db).expect("load project");
    inventory::latest_complete_snapshot(db, project.id)
        .expect("read snapshot")
        .expect("the fixture was scanned")
        .id
}

/// The last path segment, split on either separator.
///
/// The stored path is built with the platform separator, so a comparison that
/// assumed one of them would pass on Windows and fail on Linux.
fn folder_name(path: &str) -> &str {
    path.split(['/', '\\']).next_back().unwrap_or(path)
}

#[test]
fn a_legal_archive_analysed_as_generic_names_the_profile_it_should_have_used() {
    let tmp = tempfile::tempdir().expect("temporary project directory");
    let mut db = field_shape_project(tmp.path(), "generic");
    let project = repository::load_project(&db).expect("load project");
    let analyzed = analyze_project(&mut db, Actor::Test).expect("analyze under generic");
    let snapshot = snapshot_of(&db);

    // The field run's own reading, reproduced: not one boundary, and nothing
    // in the rest of the outcome that would make anyone look twice.
    assert_eq!(analyzed.protected_boundaries, 0);
    assert!(context::protected_folders(&db, snapshot)
        .expect("protected evidence")
        .is_empty());

    // What is new: the run says which profile would have protected what it
    // did not, and how much of it. Specific enough to act on.
    let signal = analyzed
        .profile_fitness
        .as_ref()
        .expect("another shipped profile protects these folders");
    assert_eq!(signal.profile_id, "legal");
    assert_eq!(
        signal.unprotected_folders, 4,
        "EXPEDIENTES, both case folders and PERICIALES"
    );
    assert_eq!(
        signal.message(),
        "profile \"legal\" would protect 4 folders this run leaves unprotected"
    );

    // It reaches the surfaces an operator and an agent actually read: the
    // diagnostic behind `status` / `project_status`, and the sealed marker,
    // which recomputes it from the persisted classification and would refuse
    // the snapshot if the two disagreed.
    let diagnostics = analysis::diagnostics(&db, snapshot).expect("structural diagnostics");
    assert_eq!(diagnostics.protected_boundaries, 0);
    assert_eq!(diagnostics.profile_fitness.as_ref(), Some(signal));
    let sealed = analysis::sealed_analysis_summary(&db, project.id, snapshot, "generic")
        .expect("sealed summary")
        .expect("analysis completed");
    assert_eq!(sealed.profile_fitness.as_ref(), Some(signal));

    // And it is a warning, not a refusal: the project finished analysing.
    assert_eq!(analyzed.state, "ANALYZED");
}

#[test]
fn the_same_archive_analysed_as_legal_produces_a_silent_signal() {
    let tmp = tempfile::tempdir().expect("temporary project directory");
    let mut db = field_shape_project(tmp.path(), "legal");
    let analyzed = analyze_project(&mut db, Actor::Test).expect("analyze under legal");
    let snapshot = snapshot_of(&db);

    assert_eq!(analyzed.protected_boundaries, 4);
    assert!(
        analyzed.profile_fitness.is_none(),
        "the right profile must not accuse itself: {:?}",
        analyzed.profile_fitness
    );
    assert!(analysis::diagnostics(&db, snapshot)
        .expect("structural diagnostics")
        .profile_fitness
        .is_none());

    // The four folders the generic run left open, named.
    let mut protected: Vec<&str> = Vec::new();
    let boundaries = context::protected_folders(&db, snapshot).expect("protected evidence");
    for boundary in &boundaries {
        assert!(!boundary.reason.is_empty(), "a boundary explains itself");
        protected.push(folder_name(&boundary.path));
    }
    protected.sort_unstable();
    assert_eq!(
        protected,
        [
            "EXPEDIENTES",
            "Exp 1234-2020",
            "Expediente 1234-2020",
            "PERICIALES",
        ]
    );
}
