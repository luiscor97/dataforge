//! End-to-end M0.7 drive with an air-gapped local provider: disclosure
//! preview, digest consent, isolated execution, validated suggestions and
//! the immutable audit trail.

#![cfg(windows)]

use std::path::{Path, PathBuf};

use df_domain::Actor;
use df_facade::{AiProviderChoice, CreateProjectRequest};

/// A deterministic "model": a sidecar that ignores stdin and prints one
/// fixed, schema-conforming suggestion document.
fn local_model(base: &Path) -> PathBuf {
    let response = serde_json::json!({
        "schema_version": "dataforge.ai-suggestions/0.7.0",
        "automatic_action": false,
        "explanation": "El elemento proviene de una regla de respaldo; conviene revisarlo manualmente.",
        "suggestions": [{
            "id": "KEEP_FOR_REVIEW",
            "label": "Conservar para revision humana",
            "explanation": "La evidencia declarada no basta para consolidar de forma segura.",
            "evidence_ids": ["reason", "recommended"],
        }],
    });
    let response_path = base.join("ai-response.json");
    std::fs::write(&response_path, serde_json::to_vec(&response).unwrap()).unwrap();
    let script = base.join("ai-model.cmd");
    std::fs::write(&script, "@type \"%~dp0ai-response.json\"\r\n").unwrap();
    script
}

fn project_with_review_item(base: &Path) -> (PathBuf, String) {
    let origin = base.join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    // The generic rule `review.backup-extension` routes *.bak to review.
    std::fs::write(origin.join("datos.bak"), b"respaldo antiguo").unwrap();
    std::fs::write(origin.join("informe.txt"), b"contenido normal").unwrap();

    let project_dir = base.join("proyecto");
    df_facade::create_project(
        &CreateProjectRequest {
            name: "Prueba IA".to_string(),
            project_dir: project_dir.clone(),
            output_root: base.join("salida"),
            audit_root: None,
            source_roots: vec![origin],
            profile: Some("generic".to_string()),
        },
        Actor::Test,
    )
    .expect("create");
    df_facade::scan_project(&project_dir, Actor::Test).expect("scan");
    df_facade::hash_project(&project_dir, Actor::Test).expect("hash");
    df_facade::analyze_project(&project_dir, Actor::Test).expect("analyze");

    let queue = df_facade::structural_review_queue(&project_dir).expect("queue");
    let item = queue
        .items
        .iter()
        .find(|item| item.status == "PENDING")
        .expect("the .bak rule must queue one review item");
    (project_dir, item.id.clone())
}

#[test]
fn disclosure_consent_and_local_execution_leave_an_audit_trail() {
    let tmp = tempfile::tempdir().unwrap();
    let (project_dir, item_id) = project_with_review_item(tmp.path());
    let choice = AiProviderChoice::LocalProcess {
        executable: local_model(tmp.path()),
        model: "deterministic-fixture".to_string(),
    };

    // Phase 1 — preview: nothing executes, the manifest is complete.
    let preview = df_facade::ai_explain_review(&project_dir, &item_id, &choice, None, Actor::Test)
        .expect("preview");
    assert!(!preview.executed);
    assert!(preview.evidence_only);
    assert!(preview.suggestions.is_empty());
    assert_eq!(preview.disclosure.purpose, "EXPLAIN");
    assert!(preview
        .disclosure
        .fields
        .iter()
        .any(|field| field.evidence_id == "reason"));
    let digest = preview.disclosure.disclosure_sha256.clone();
    assert_eq!(digest.len(), 64);

    // A wrong digest is rejected before anything runs.
    let wrong = df_facade::ai_explain_review(
        &project_dir,
        &item_id,
        &choice,
        Some(&"0".repeat(64)),
        Actor::Test,
    );
    assert!(wrong.is_err());

    // Phase 2 — consent to exactly that disclosure and execute isolated.
    let outcome =
        df_facade::ai_explain_review(&project_dir, &item_id, &choice, Some(&digest), Actor::Test)
            .expect("execute");
    assert!(outcome.executed);
    assert_eq!(outcome.status.as_deref(), Some("ACCEPTED"));
    assert_eq!(outcome.suggestions.len(), 1);
    let suggestion = &outcome.suggestions[0];
    assert_eq!(suggestion.id, "KEEP_FOR_REVIEW");
    assert!(!suggestion.automatic_action);
    assert_eq!(suggestion.evidence_ids, vec!["reason", "recommended"]);

    // The audit trail is immutable evidence and the ledger stays valid.
    let audits = df_facade::ai_audit_report(&project_dir, 10).expect("audits");
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].status, "ACCEPTED");
    assert_eq!(audits[0].provider, "local-process");
    assert_eq!(audits[0].disclosure_sha256, digest);
    let audit = df_facade::verify_audit(&project_dir).expect("ledger");
    assert!(audit.ledger_ok);
}

#[test]
fn a_cloud_execution_without_a_stored_key_fails_closed_after_consent() {
    let tmp = tempfile::tempdir().unwrap();
    let (project_dir, item_id) = project_with_review_item(tmp.path());
    let choice = AiProviderChoice::Cloud {
        provider: df_facade::AiKeyProvider::Anthropic,
        model: "claude-sonnet-5".to_string(),
    };

    let preview = df_facade::ai_explain_review(&project_dir, &item_id, &choice, None, Actor::Test)
        .expect("preview needs no key");
    assert!(!preview.executed);
    assert!(preview
        .disclosure
        .endpoint
        .starts_with("https://api.anthropic.com"));

    // Consent given, but no key stored on this machine: the invocation must
    // fail with a clear validation error and never reach the network.
    if !df_facade::ai_key_present(df_facade::AiKeyProvider::Anthropic).unwrap_or(false) {
        let digest = preview.disclosure.disclosure_sha256.clone();
        let outcome = df_facade::ai_explain_review(
            &project_dir,
            &item_id,
            &choice,
            Some(&digest),
            Actor::Test,
        );
        assert!(outcome.is_err());
    }
}
