use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Version of the signed manifest envelope understood by this crate.
pub const MANIFEST_SCHEMA_VERSION: &str = "dataforge.plugin-manifest/0.1.0";
/// Exact Component Model ABI implemented by [`crate::PluginHost`].
pub const HOST_ABI_VERSION: &str = "0.1.0";
/// Version of the immutable JSON snapshot passed to `analyze`.
pub const INPUT_SCHEMA_VERSION: &str = "dataforge.plugin-input/0.1.0";
/// Identifier of the only output schema accepted by ABI 0.1.
pub const OUTPUT_SCHEMA_ID: &str = "dataforge.plugin-findings/0.1.0";

/// A capability is explicit, signed and limited to data copied into the
/// invocation. ABI 0.1 intentionally has no ambient-resource capabilities.
/// Adding filesystem, network, environment or clock imports requires a new ABI
/// version and a separate security review.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Capability {
    /// Receive bounded key/value metadata supplied by DataForge.
    SubjectMetadata,
    /// Receive bounded normalized text supplied by DataForge.
    SubjectText,
}

/// A content-addressed reference to a host-owned schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaReference {
    pub id: String,
    pub sha256: String,
}

/// Signed declaration of identity, compatibility and least privilege.
///
/// Compatibility uses two independent checks:
///
/// * `abi_version` is an exact semantic version and must equal
///   [`HOST_ABI_VERSION`];
/// * `host_compatibility` is a `semver::VersionReq` range and must contain the
///   host ABI version (for example `>=0.1.0, <0.2.0` or `=0.1.0`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub manifest_schema_version: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub abi_version: String,
    pub host_compatibility: String,
    pub publisher: String,
    pub capabilities: BTreeSet<Capability>,
    pub output_schema: SchemaReference,
    /// Must always be `false`. Plugins can report and suggest, never act.
    pub automatic_action: bool,
}

/// Caller-owned evidence before capability filtering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRequest {
    pub request_id: String,
    pub subject: PluginSubject,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_text: Option<String>,
}

/// Minimal immutable identity that every plugin may see.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSubject {
    pub id: String,
    pub kind: String,
}

/// Versioned JSON supplied to the component after capability filtering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginInput {
    pub schema_version: String,
    pub request_id: String,
    pub subject: PluginSubject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_text: Option<String>,
}

/// Severity is intentionally informational: plugins do not make decisions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingSeverity {
    Info,
    Warning,
}

/// Human-readable suggestion without an executable action or command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    pub recommendation: String,
    pub rationale: String,
}

/// One bounded observation returned by a plugin.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub code: String,
    pub severity: FindingSeverity,
    pub message: String,
    pub subject_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

/// The only accepted result envelope. `automatic_action` is validated both by
/// the closed JSON Schema and by an explicit host check.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginOutput {
    pub schema_version: String,
    pub automatic_action: bool,
    pub findings: Vec<Finding>,
}

/// Host-owned Draft 2020-12 schema. Every object is closed with
/// `additionalProperties: false`, and every collection/string is bounded.
pub fn output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": OUTPUT_SCHEMA_ID,
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "automatic_action", "findings"],
        "properties": {
            "schema_version": { "const": OUTPUT_SCHEMA_ID },
            "automatic_action": { "const": false },
            "findings": {
                "type": "array",
                "maxItems": 256,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["code", "severity", "message", "subject_id"],
                    "properties": {
                        "code": {
                            "type": "string",
                            "pattern": "^[A-Z][A-Z0-9_]{0,63}$"
                        },
                        "severity": { "enum": ["INFO", "WARNING"] },
                        "message": { "type": "string", "minLength": 1, "maxLength": 4096 },
                        "subject_id": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "suggestions": {
                            "type": "array",
                            "maxItems": 32,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["recommendation", "rationale"],
                                "properties": {
                                    "recommendation": {
                                        "type": "string", "minLength": 1, "maxLength": 2048
                                    },
                                    "rationale": {
                                        "type": "string", "minLength": 1, "maxLength": 4096
                                    }
                                }
                            }
                        },
                        "evidence": {
                            "type": "object",
                            "maxProperties": 64,
                            "additionalProperties": {
                                "type": "string", "maxLength": 4096
                            },
                            "propertyNames": { "minLength": 1, "maxLength": 128 }
                        }
                    }
                }
            }
        }
    })
}

/// SHA-256 over the canonical compact serialization of [`output_schema`].
pub fn output_schema_sha256() -> String {
    let bytes = serde_json::to_vec(&output_schema())
        .expect("the statically constructed output schema is serializable");
    hex::encode(Sha256::digest(bytes))
}
