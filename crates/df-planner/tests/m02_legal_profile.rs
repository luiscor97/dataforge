//! Windows-only: this scenario drives planning through execution,
//! and execution refuses fail-closed off Windows until POSIX write
//! safety exists (the refusal is pinned by the CLI/corpus tests).
#![cfg(windows)]

use std::collections::HashMap;

use df_db::{analysis, context, inventory, plans, repository, Db};
use df_domain::{
    Actor, DuplicatePolicy, ExecutionState, OccurrenceId, OperationType, ProfileRef, Project,
    RuleAction, SnapshotId, SourceRoot,
};
use df_executor::{execute_plan, ExecuteOptions};
use df_hash::{hash_project, HashOptions};
use df_planner::{analyze_project, approve_plan, create_plan};
use df_scan::{scan_project, ScanOptions};

#[test]
fn legal_boundaries_survive_aggressive_consolidation_with_explainable_evidence() {
    let temporary = tempfile::tempdir().expect("temporary project directory");
    let origin = temporary.path().join("origen");
    let first_case = origin.join("Expediente 100-2026");
    let second_case = origin.join("Expediente 200-2026");
    let auxiliary = origin.join("Material auxiliar");
    let version_a = origin.join("Versión A");
    let version_b = origin.join("Versión B");
    for directory in [
        &first_case,
        &second_case,
        &auxiliary,
        &version_a,
        &version_b,
    ] {
        std::fs::create_dir_all(directory).expect("create fixture directory");
    }

    // The same exact evidence belongs to two independent legal matters. Even
    // CONSOLIDATE_ALL must materialise both occurrences.
    const LEGAL_EVIDENCE: &[u8] = b"same signed pleading in two protected matters";
    std::fs::write(first_case.join("escrito-firmado.pdf"), LEGAL_EVIDENCE)
        .expect("write first protected copy");
    std::fs::write(second_case.join("escrito-firmado.pdf"), LEGAL_EVIDENCE)
        .expect("write second protected copy");

    // The legal profile inherits the generic non-destructive rules.
    std::fs::write(auxiliary.join("~$Borrador.docx"), b"office lock file")
        .expect("write temporary rule fixture");
    std::fs::write(auxiliary.join("informe.bak"), b"backup requiring review")
        .expect("write review rule fixture");

    // A portable anomaly fixture: two related trees share two contents while
    // each retains one unique content. This produces reviewable preservation
    // evidence without relying on case-only names that cannot coexist on a
    // normal Windows volume.
    for (name, bytes) in [("comun-a.bin", b"shared A"), ("comun-b.bin", b"shared B")] {
        std::fs::write(version_a.join(name), bytes).expect("write shared file in version A");
        std::fs::write(version_b.join(name), bytes).expect("write shared file in version B");
    }
    std::fs::write(version_a.join("solo-a.bin"), b"unique to A").expect("write unique A file");
    std::fs::write(version_b.join("solo-b.bin"), b"unique to B").expect("write unique B file");

    let mut db = Db::open(&temporary.path().join("state.sqlite")).expect("open project database");
    let project = Project::new(
        "Integración perfil jurídico",
        ProfileRef::new("legal"),
        temporary.path().join("salida"),
        temporary.path().join("auditoria"),
        "test",
    );
    let roots = vec![SourceRoot::new(project.id, origin)];
    repository::create_project(&mut db, &project, &roots, Actor::Test)
        .expect("persist legal project");

    let scanned =
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).expect("scan fixture");
    assert_eq!(scanned.errors, 0);
    let hashed =
        hash_project(&mut db, Actor::Test, &HashOptions::default(), None).expect("hash fixture");
    assert_eq!(hashed.failed, 0);
    assert_eq!(hashed.pending, 0);

    let analyzed = analyze_project(&mut db, Actor::Test).expect("analyze legal fixture");
    let snapshot_id: SnapshotId = analyzed.snapshot_id.parse().expect("snapshot id");
    assert_eq!(analyzed.protected_boundaries, 2);
    assert_eq!(analyzed.rule_matches, 2, "~$ and .bak rules must match");
    assert_eq!(analyzed.partial_tree_clones, 1);
    assert_eq!(analyzed.anomalies, 1);
    assert_eq!(analyzed.high_anomalies, 0);
    assert_eq!(
        analyzed.review_items, 2,
        "one rule and one anomaly need review"
    );

    let boundaries = context::protected_folders(&db, snapshot_id).expect("protected evidence");
    assert_eq!(boundaries.len(), 2);
    assert!(boundaries.iter().all(|boundary| {
        boundary.marker == "expediente"
            && !boundary.reason.is_empty()
            && boundary.path.contains("Expediente")
    }));

    let anomalies = analysis::anomaly_report(&db, snapshot_id).expect("anomaly evidence");
    assert_eq!(anomalies.warnings, 1);
    assert!(anomalies.anomalies.iter().any(|anomaly| {
        anomaly.kind == "PARTIAL_TREE_UNIQUE_CONTENT"
            && anomaly.requires_review
            && anomaly.evidence["unique_a_files"].as_u64() == Some(1)
            && anomaly.evidence["unique_b_files"].as_u64() == Some(1)
    }));

    let review = analysis::review_queue(&db, snapshot_id).expect("review evidence");
    assert_eq!(review.pending, 2);
    assert!(review.items.iter().any(|item| {
        item.source == "RULE"
            && item.kind == "review.backup-extension"
            && item.recommended_action == "COPY_REVIEW"
            && item.reason.contains("backup")
    }));
    assert!(review.items.iter().any(|item| {
        item.source == "ANOMALY"
            && item.kind == "PARTIAL_TREE_UNIQUE_CONTENT"
            && item.status == "PENDING"
            && item.folder_a.is_some()
            && item.folder_b.is_some()
            && item
                .evidence
                .as_ref()
                .and_then(|value| value["shared_files"].as_u64())
                == Some(2)
    }));

    let occurrences = inventory::list_occurrences(&db, snapshot_id).expect("occurrences");
    let paths_by_id: HashMap<OccurrenceId, &str> = occurrences
        .iter()
        .map(|occurrence| (occurrence.id, occurrence.relative_path.as_str()))
        .collect();
    let ids_by_name: HashMap<&str, OccurrenceId> = occurrences
        .iter()
        .map(|occurrence| (occurrence.file_name.as_str(), occurrence.id))
        .collect();
    let guidance = analysis::occurrence_guidance(&db, snapshot_id).expect("rule guidance");
    let temporary_id = ids_by_name["~$Borrador.docx"];
    let backup_id = ids_by_name["informe.bak"];
    assert_eq!(
        guidance[&temporary_id].operation_type,
        OperationType::CopyTemporary
    );
    assert!(guidance[&temporary_id]
        .reason
        .contains("temporary.office-lock"));
    assert_eq!(
        guidance[&backup_id].operation_type,
        OperationType::CopyReview
    );
    assert!(guidance[&backup_id].reason.contains("pending human review"));

    let backup_review_id = review
        .items
        .iter()
        .find(|item| item.kind == "review.backup-extension")
        .expect("backup review item")
        .id
        .clone();
    analysis::decide_review_item(
        &mut db,
        project.id,
        &backup_review_id,
        RuleAction::CopySeparated,
        "Conservar el backup fuera del conjunto activo",
        Actor::Test,
    )
    .expect("append review decision");
    let decided_queue = analysis::review_queue(&db, snapshot_id).expect("decided review queue");
    assert_eq!(decided_queue.pending, 1);
    assert_eq!(decided_queue.decided, 1);
    let decided_guidance =
        analysis::occurrence_guidance(&db, snapshot_id).expect("decided rule guidance");
    assert_eq!(
        decided_guidance[&backup_id].operation_type,
        OperationType::CopySeparated
    );
    assert!(decided_guidance[&backup_id]
        .reason
        .contains("Conservar el backup"));

    let plan_outcome = create_plan(&mut db, Actor::Test, DuplicatePolicy::ConsolidateAll)
        .expect("plan with aggressive duplicate policy");
    assert_eq!(plan_outcome.skipped_represented, 0);
    assert_eq!(plan_outcome.preserved_across_context, 2);
    assert_eq!(plan_outcome.temporary_copies, 1);
    assert_eq!(plan_outcome.separated_copies, 1);
    assert_eq!(
        plan_outcome.review_copies, 6,
        "the pending tree decision conservatively routes every occurrence in both subtrees"
    );
    assert_eq!(plan_outcome.duplicate_policy, "CONSOLIDATE_ALL");

    let plan = plans::current_plan(&db, project.id)
        .expect("load current plan")
        .expect("plan exists");
    let operations = plans::list_operations(&db, plan.id).expect("plan operations");
    let preserved: Vec<_> = operations
        .iter()
        .filter(|operation| operation.operation_type == OperationType::PreserveAcrossContext)
        .collect();
    assert_eq!(preserved.len(), 2);
    for operation in &preserved {
        let occurrence_id = operation
            .source_occurrence
            .expect("preservation covers a source occurrence");
        let source = paths_by_id[&occurrence_id];
        assert!(source.contains("Expediente"), "unexpected source: {source}");
        assert!(source.ends_with("escrito-firmado.pdf"));
        assert!(operation.destination_relative_path.is_some());
        assert!(operation.operation_type.is_executable());
        assert_eq!(operation.execution_state, ExecutionState::Pending);
        assert!(operation.reason.contains("expediente"));
        assert!(operation.reason.contains("CONSOLIDATE_ALL"));
        assert!(operation.reason.contains("rule 9"));
    }
    assert!(preserved.iter().any(|operation| paths_by_id
        [&operation.source_occurrence.expect("source")]
        .contains("Expediente 100-2026")));
    assert!(preserved.iter().any(|operation| paths_by_id
        [&operation.source_occurrence.expect("source")]
        .contains("Expediente 200-2026")));

    let temporary_operation = operations
        .iter()
        .find(|operation| operation.source_occurrence == Some(temporary_id))
        .expect("temporary operation");
    assert_eq!(
        temporary_operation.operation_type,
        OperationType::CopyTemporary
    );
    assert!(temporary_operation.reason.contains("temporary.office-lock"));
    assert!(temporary_operation
        .destination_relative_path
        .as_deref()
        .is_some_and(|path| path.starts_with("98_DataForge_Temporary")));
    let backup_operation = operations
        .iter()
        .find(|operation| operation.source_occurrence == Some(backup_id))
        .expect("backup operation");
    assert_eq!(
        backup_operation.operation_type,
        OperationType::CopySeparated
    );
    assert!(backup_operation.reason.contains("Conservar el backup"));
    assert!(backup_operation
        .destination_relative_path
        .as_deref()
        .is_some_and(|path| path.starts_with("95_DataForge_Separated")));

    let tree_review_operations: Vec<_> = operations
        .iter()
        .filter(|operation| operation.operation_type == OperationType::CopyReview)
        .collect();
    assert_eq!(tree_review_operations.len(), 6);
    assert!(tree_review_operations.iter().all(|operation| operation
        .destination_relative_path
        .as_deref()
        .is_some_and(|path| path.starts_with("90_DataForge_Review"))));

    let approved = approve_plan(&mut db, Actor::Test).expect("approve complete M0.2 plan");
    assert_eq!(approved.operations_approved, operations.len() as u64);
    let executed = execute_plan(&mut db, Actor::Test, &ExecuteOptions::default(), None)
        .expect("execute routed plan");
    assert_eq!(executed.failed_final, 0);
    assert_eq!(executed.failed_retryable, 0);
    assert_eq!(executed.pending, 0);
    assert_eq!(executed.state, "EXECUTED");
    assert_eq!(
        std::fs::read(
            project.output_root.join(
                temporary_operation
                    .destination_relative_path
                    .as_ref()
                    .unwrap()
            )
        )
        .expect("temporary bucket copy"),
        b"office lock file"
    );
    assert_eq!(
        std::fs::read(
            project
                .output_root
                .join(backup_operation.destination_relative_path.as_ref().unwrap())
        )
        .expect("separated bucket copy"),
        b"backup requiring review"
    );
}
