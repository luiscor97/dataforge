use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signer, SigningKey};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

use super::*;

const VALID_COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/metadata-reporter/component.wat");
const FILESYSTEM_COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/filesystem-attempt/component.wat");
const LOOP_COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/infinite-loop/component.wat");
const MEMORY_COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/memory-bomb/component.wat");
const MALFORMED_COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/malformed-output/component.wat");
const INCOMPATIBLE_COMPONENT: &[u8] =
    include_bytes!("../../../plugins/examples/abi-incompatible/component.wat");

fn manifest(plugin_id: &str, capabilities: impl IntoIterator<Item = Capability>) -> PluginManifest {
    PluginManifest {
        manifest_schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        plugin_id: plugin_id.to_string(),
        plugin_version: "1.2.3".to_string(),
        abi_version: HOST_ABI_VERSION.to_string(),
        host_compatibility: ">=0.1.0, <0.2.0".to_string(),
        publisher: "DataForge ephemeral test publisher".to_string(),
        capabilities: capabilities.into_iter().collect(),
        output_schema: SchemaReference {
            id: OUTPUT_SCHEMA_ID.to_string(),
            sha256: output_schema_sha256(),
        },
        automatic_action: false,
    }
}

fn signed_package(component: &[u8], manifest: PluginManifest) -> SignedPluginPackage {
    let component_sha256 = hex::encode(Sha256::digest(component));
    let mut secret = [0_u8; 32];
    OsRng.fill_bytes(&mut secret);
    let signing_key = SigningKey::from_bytes(&secret);
    let message = registration_signing_bytes(&manifest, &component_sha256).unwrap();
    let signature = signing_key.sign(&message);
    SignedPluginPackage {
        manifest,
        component_sha256,
        component_bytes: component.to_vec(),
        publisher_public_key_hex: hex::encode(signing_key.verifying_key().to_bytes()),
        signature_hex: hex::encode(signature.to_bytes()),
    }
}

fn request() -> AnalysisRequest {
    AnalysisRequest {
        request_id: "request-1".to_string(),
        subject: PluginSubject {
            id: "subject-1".to_string(),
            kind: "DOCUMENT".to_string(),
        },
        metadata: BTreeMap::from([("extension".to_string(), "txt".to_string())]),
        normalized_text: Some("bounded normalized text".to_string()),
    }
}

fn host_with(capabilities: impl IntoIterator<Item = Capability>) -> PluginHost {
    PluginHost::new(
        HostLimits::default(),
        HostPolicy {
            granted_capabilities: capabilities.into_iter().collect(),
        },
    )
    .unwrap()
}

#[test]
fn valid_component_is_signed_immutable_deterministic_and_suggestion_only() {
    let mut host = host_with([Capability::SubjectMetadata]);
    let package = signed_package(
        VALID_COMPONENT,
        manifest(
            "org.dataforge.metadata-reporter",
            [Capability::SubjectMetadata],
        ),
    );
    let duplicate = package.clone();
    let key = host.register(package).unwrap();

    assert_eq!(
        host.describe(&key).unwrap(),
        "Deterministic metadata reporter"
    );
    let first = host.analyze(&key, &request()).unwrap();
    let second = host.analyze(&key, &request()).unwrap();
    assert_eq!(first, second);
    assert!(!first.automatic_action);
    assert_eq!(first.findings.len(), 1);
    assert_eq!(first.findings[0].code, "METADATA_REPORTED");

    let metadata_json = serde_json::to_string(&host.registered_plugins()).unwrap();
    assert!(metadata_json.contains("org.dataforge.metadata-reporter"));
    assert!(matches!(
        host.register(duplicate),
        Err(PluginError::AlreadyRegistered(_))
    ));
    assert_eq!(host.registry().len(), 1);
}

#[test]
fn filesystem_import_is_rejected_before_registry_mutation() {
    let mut host = host_with([]);
    let package = signed_package(
        FILESYSTEM_COMPONENT,
        manifest("org.dataforge.filesystem-attempt", []),
    );
    assert!(matches!(
        host.register(package),
        Err(PluginError::CapabilityDenied(_))
    ));
    assert!(host.registry().is_empty());
}

#[test]
fn signed_capabilities_must_be_granted_and_unrequested_data_is_stripped() {
    let mut denied_host = host_with([]);
    let denied_key = denied_host
        .register(signed_package(
            VALID_COMPONENT,
            manifest(
                "org.dataforge.capability-denied",
                [Capability::SubjectMetadata],
            ),
        ))
        .unwrap();
    assert!(matches!(
        denied_host.filtered_input(&denied_key, &request()),
        Err(PluginError::CapabilityDenied(_))
    ));

    let mut no_data_host = host_with([]);
    let no_data_key = no_data_host
        .register(signed_package(
            VALID_COMPONENT,
            manifest("org.dataforge.no-data", []),
        ))
        .unwrap();
    let filtered = no_data_host
        .filtered_input(&no_data_key, &request())
        .unwrap();
    assert!(filtered.metadata.is_none());
    assert!(filtered.normalized_text.is_none());

    let mut metadata_host = host_with([Capability::SubjectMetadata]);
    let metadata_key = metadata_host
        .register(signed_package(
            VALID_COMPONENT,
            manifest("org.dataforge.metadata-only", [Capability::SubjectMetadata]),
        ))
        .unwrap();
    let filtered = metadata_host
        .filtered_input(&metadata_key, &request())
        .unwrap();
    assert_eq!(filtered.metadata, Some(request().metadata));
    assert!(filtered.normalized_text.is_none());
}

#[test]
fn infinite_loop_is_stopped_by_fuel_or_epoch_deadline() {
    let limits = HostLimits {
        fuel: 100_000_000,
        epoch_timeout_ms: 10,
        ..HostLimits::default()
    };
    let mut host = PluginHost::new(limits, HostPolicy::default()).unwrap();
    let key = host
        .register(signed_package(
            LOOP_COMPONENT,
            manifest("org.dataforge.infinite-loop", []),
        ))
        .unwrap();
    let result = host.analyze(&key, &request());
    assert!(
        matches!(
            &result,
            Err(PluginError::LimitExceeded {
                kind: LimitKind::FuelOrEpoch
            })
        ),
        "unexpected result: {result:?}"
    );
}

#[test]
fn memory_growth_is_stopped_by_store_limit() {
    let limits = HostLimits {
        max_memory_bytes: 1024 * 1024,
        ..HostLimits::default()
    };
    let mut host = PluginHost::new(limits, HostPolicy::default()).unwrap();
    let key = host
        .register(signed_package(
            MEMORY_COMPONENT,
            manifest("org.dataforge.memory-bomb", []),
        ))
        .unwrap();
    let result = host.analyze(&key, &request());
    assert!(
        matches!(
            &result,
            Err(PluginError::LimitExceeded {
                kind: LimitKind::Memory
            })
        ),
        "unexpected result: {result:?}"
    );
}

#[test]
fn malformed_and_non_closed_outputs_are_rejected() {
    let mut malformed_host = host_with([]);
    let malformed_key = malformed_host
        .register(signed_package(
            MALFORMED_COMPONENT,
            manifest("org.dataforge.malformed-output", []),
        ))
        .unwrap();
    assert!(matches!(
        malformed_host.analyze(&malformed_key, &request()),
        Err(PluginError::MalformedOutput(_))
    ));

    let extra_property = br#"{"schema_version":"dataforge.plugin-findings/0.1.0","automatic_action":false,"findings":[],"unexpected":true}"#;
    let component = constant_component(extra_property);
    let mut schema_host = host_with([]);
    let schema_key = schema_host
        .register(signed_package(
            component.as_bytes(),
            manifest("org.dataforge.schema-violation", []),
        ))
        .unwrap();
    assert!(matches!(
        schema_host.analyze(&schema_key, &request()),
        Err(PluginError::OutputSchema(_))
    ));
}

#[test]
fn automatic_actions_are_rejected_explicitly() {
    let forbidden = br#"{"schema_version":"dataforge.plugin-findings/0.1.0","automatic_action":true,"findings":[]}"#;
    let component = constant_component(forbidden);
    let mut host = host_with([]);
    let key = host
        .register(signed_package(
            component.as_bytes(),
            manifest("org.dataforge.automatic-action", []),
        ))
        .unwrap();
    assert!(matches!(
        host.analyze(&key, &request()),
        Err(PluginError::AutomaticActionForbidden)
    ));
}

#[test]
fn component_hash_and_signature_tampering_fail_closed() {
    let mut altered_bytes =
        signed_package(VALID_COMPONENT, manifest("org.dataforge.altered-bytes", []));
    altered_bytes.component_bytes.push(b' ');
    let mut host = host_with([]);
    assert!(matches!(
        host.register(altered_bytes),
        Err(PluginError::HashMismatch)
    ));

    let mut altered_signature = signed_package(
        VALID_COMPONENT,
        manifest("org.dataforge.altered-signature", []),
    );
    let mut signature = hex::decode(&altered_signature.signature_hex).unwrap();
    signature[0] ^= 0x80;
    altered_signature.signature_hex = hex::encode(signature);
    assert!(matches!(
        host.register(altered_signature),
        Err(PluginError::SignatureInvalid)
    ));
    assert!(host.registry().is_empty());
}

#[test]
fn exact_abi_and_semver_range_are_both_enforced() {
    let mut incompatible_abi = manifest("org.dataforge.incompatible-abi", []);
    incompatible_abi.abi_version = "0.2.0".to_string();
    let mut host = host_with([]);
    assert!(matches!(
        host.register(signed_package(INCOMPATIBLE_COMPONENT, incompatible_abi)),
        Err(PluginError::Incompatible(_))
    ));

    let mut incompatible_range = manifest("org.dataforge.incompatible-range", []);
    incompatible_range.host_compatibility = ">=0.2.0, <0.3.0".to_string();
    assert!(matches!(
        host.register(signed_package(VALID_COMPONENT, incompatible_range)),
        Err(PluginError::Incompatible(_))
    ));
    assert!(host.registry().is_empty());
}

#[test]
fn input_output_and_configuration_have_non_disableable_hard_limits() {
    let input_limits = HostLimits {
        max_input_bytes: 128,
        ..HostLimits::default()
    };
    let mut input_host = PluginHost::new(input_limits, HostPolicy::default()).unwrap();
    let input_key = input_host
        .register(signed_package(
            VALID_COMPONENT,
            manifest("org.dataforge.input-limit", []),
        ))
        .unwrap();
    let mut large_request = request();
    large_request.request_id = "x".repeat(512);
    assert!(matches!(
        input_host.analyze(&input_key, &large_request),
        Err(PluginError::LimitExceeded {
            kind: LimitKind::InputBytes
        })
    ));

    let output = format!(
        "{{\"schema_version\":\"{OUTPUT_SCHEMA_ID}\",\"automatic_action\":false,\"findings\":[]}}"
    );
    let component = constant_component(output.as_bytes());
    let output_limits = HostLimits {
        max_output_bytes: 32,
        ..HostLimits::default()
    };
    let mut output_host = PluginHost::new(output_limits, HostPolicy::default()).unwrap();
    let output_key = output_host
        .register(signed_package(
            component.as_bytes(),
            manifest("org.dataforge.output-limit", []),
        ))
        .unwrap();
    assert!(matches!(
        output_host.analyze(&output_key, &request()),
        Err(PluginError::LimitExceeded {
            kind: LimitKind::OutputBytes
        })
    ));

    let invalid = HostLimits {
        fuel: u64::MAX,
        ..HostLimits::default()
    };
    assert!(matches!(
        PluginHost::new(invalid, HostPolicy::default()),
        Err(PluginError::InvalidHostConfiguration(_))
    ));
}

#[test]
fn output_schema_is_closed_valid_and_content_addressed() {
    let schema = output_schema();
    assert!(jsonschema::meta::is_valid(&schema));
    assert_eq!(output_schema_sha256().len(), 64);
    assert_eq!(
        output_schema_sha256(),
        hex::encode(Sha256::digest(serde_json::to_vec(&schema).unwrap()))
    );
}

#[test]
fn manifest_capabilities_are_unique_and_serializable() {
    let manifest = manifest(
        "org.dataforge.serialization",
        [Capability::SubjectText, Capability::SubjectMetadata],
    );
    let json = serde_json::to_string(&manifest).unwrap();
    let decoded: PluginManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(manifest, decoded);
    assert_eq!(decoded.capabilities.len(), 2);
    assert_eq!(
        decoded.capabilities,
        BTreeSet::from([Capability::SubjectMetadata, Capability::SubjectText])
    );
}

fn constant_component(output: &[u8]) -> String {
    let output_len = u32::try_from(output.len()).unwrap();
    let description = b"test";
    let describe_pair = pair_bytes(1024, u32::try_from(description.len()).unwrap());
    let analyze_pair = pair_bytes(4096, output_len);
    format!(
        r#"(component
  (core module $guest
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 32768))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $result i32)
      global.get $heap local.tee $result
      local.get 3 i32.add global.set $heap
      local.get $result)
    (data (i32.const 0) "{}")
    (data (i32.const 8) "{}")
    (data (i32.const 1024) "{}")
    (data (i32.const 4096) "{}")
    (func (export "describe") (result i32) i32.const 0)
    (func (export "analyze") (param i32 i32) (result i32) i32.const 8))
  (core instance $guest (instantiate $guest))
  (func (export "describe") (result string)
    (canon lift (core func $guest "describe")
      (memory $guest "memory") (realloc (func $guest "realloc"))))
  (func (export "analyze") (param "input" string) (result string)
    (canon lift (core func $guest "analyze")
      (memory $guest "memory") (realloc (func $guest "realloc")))))"#,
        wat_bytes(&describe_pair),
        wat_bytes(&analyze_pair),
        wat_bytes(description),
        wat_bytes(output)
    )
}

fn pair_bytes(pointer: u32, length: u32) -> [u8; 8] {
    let mut pair = [0_u8; 8];
    pair[..4].copy_from_slice(&pointer.to_le_bytes());
    pair[4..].copy_from_slice(&length.to_le_bytes());
    pair
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}
