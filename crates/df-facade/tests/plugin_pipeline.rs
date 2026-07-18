//! End-to-end M0.6 drive: register a signed example plugin, execute it over
//! an analysed snapshot and read the sealed findings back.

use std::path::Path;

use df_domain::Actor;
use df_facade::CreateProjectRequest;
use df_plugin::{
    output_schema_sha256, registration_signing_bytes, Capability, PluginManifest, SchemaReference,
    HOST_ABI_VERSION, MANIFEST_SCHEMA_VERSION,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

const COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/metadata-reporter/component.wat");

fn analyzed_project(base: &Path) -> std::path::PathBuf {
    let origin = base.join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::write(origin.join("informe.txt"), b"contenido uno").unwrap();
    std::fs::write(origin.join("acta.txt"), b"contenido dos").unwrap();

    let project_dir = base.join("proyecto");
    df_facade::create_project(
        &CreateProjectRequest {
            name: "Prueba plugins".to_string(),
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
    project_dir
}

/// Write the signed package the CLI-facing facade API consumes: one JSON
/// envelope plus the component file it signs. The signing key is a fixed
/// test seed; verification only needs the public half.
fn write_signed_package(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let manifest = PluginManifest {
        manifest_schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        plugin_id: "org.dataforge.metadata-reporter".to_string(),
        plugin_version: "1.2.3".to_string(),
        abi_version: HOST_ABI_VERSION.to_string(),
        host_compatibility: ">=0.1.0, <0.2.0".to_string(),
        publisher: "DataForge example publisher".to_string(),
        capabilities: [Capability::SubjectMetadata].into_iter().collect(),
        output_schema: SchemaReference {
            id: "dataforge.plugin-findings/0.1.0".to_string(),
            sha256: output_schema_sha256(),
        },
        automatic_action: false,
    };
    let component_sha256 = hex::encode(Sha256::digest(COMPONENT));
    let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let message = registration_signing_bytes(&manifest, &component_sha256).unwrap();
    let signature = signing_key.sign(&message);

    let package = serde_json::json!({
        "manifest": manifest,
        "component_sha256": component_sha256,
        "publisher_public_key_hex": hex::encode(signing_key.verifying_key().to_bytes()),
        "signature_hex": hex::encode(signature.to_bytes()),
    });
    let package_path = base.join("metadata-reporter.package.json");
    let component_path = base.join("metadata-reporter.component.wat");
    std::fs::write(&package_path, serde_json::to_vec_pretty(&package).unwrap()).unwrap();
    std::fs::write(&component_path, COMPONENT).unwrap();
    (package_path, component_path)
}

#[test]
fn signed_plugin_registers_runs_and_seals_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = analyzed_project(tmp.path());
    let (package_path, component_path) = write_signed_package(tmp.path());

    let metadata =
        df_facade::register_plugin(&project_dir, &package_path, &component_path, Actor::Test)
            .expect("register");
    assert_eq!(
        metadata.key.to_string(),
        "org.dataforge.metadata-reporter@1.2.3"
    );

    let listed = df_facade::list_plugins(&project_dir).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].plugin, "org.dataforge.metadata-reporter@1.2.3");

    // A second registration of the same version is a conflict, not a swap.
    let duplicate =
        df_facade::register_plugin(&project_dir, &package_path, &component_path, Actor::Test);
    assert!(duplicate.is_err());

    let outcome = df_facade::run_plugins(&project_dir, Actor::Test).expect("run");
    assert!(!outcome.cancelled);
    assert!(outcome.evidence_only);
    assert_eq!(outcome.runs.len(), 1);
    let run = &outcome.runs[0];
    assert_eq!(run.status, "COMPLETED");
    assert_eq!(run.subjects_total, 2, "two unique contents");
    assert_eq!(run.subjects_analyzed, 2);
    assert_eq!(run.subjects_failed, 0);
    assert_eq!(
        run.findings, 2,
        "the example reports one finding per subject"
    );

    let report = df_facade::plugin_report(&project_dir).expect("report");
    assert!(report.evidence_only);
    assert_eq!(report.findings.len(), 2);
    assert!(report
        .findings
        .iter()
        .all(|finding| finding.code == "METADATA_REPORTED" && finding.severity == "INFO"));

    // Same configuration: the sealed run is reused, nothing re-executes.
    let again = df_facade::run_plugins(&project_dir, Actor::Test).expect("sealed reuse");
    assert_eq!(again.runs[0].run_id, run.run_id);
    assert_eq!(again.runs[0].findings, 2);

    let audit = df_facade::verify_audit(&project_dir).expect("audit");
    assert!(audit.ledger_ok);
}

#[test]
fn a_tampered_component_is_rejected_before_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = analyzed_project(tmp.path());
    let (package_path, component_path) = write_signed_package(tmp.path());

    // Flip one byte of the component after signing: registration must fail
    // and nothing may be stored.
    let mut bytes = std::fs::read(&component_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&component_path, bytes).unwrap();

    let rejected =
        df_facade::register_plugin(&project_dir, &package_path, &component_path, Actor::Test);
    assert!(rejected.is_err());
    assert!(df_facade::list_plugins(&project_dir)
        .expect("list")
        .is_empty());
}
