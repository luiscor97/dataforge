use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PROMPT_VERSION: &str = "dataforge.assisted-intelligence-prompt/0.7.0";
pub const DISCLOSURE_SCHEMA_VERSION: &str = "dataforge.ai-disclosure/0.7.0";
pub const AUDIT_SCHEMA_VERSION: &str = "dataforge.ai-audit/0.7.0";
pub(crate) const REQUEST_SCHEMA_VERSION: &str = "dataforge.ai-request/0.7.0";
pub(crate) const CONSENT_DOMAIN: &[u8] = b"DataForge cloud disclosure consent\0v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AiMode {
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProviderKind {
    LocalProcess,
    Cloud,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDescriptor {
    pub kind: ProviderKind,
    pub provider: String,
    pub model: String,
    /// The cloud HTTPS endpoint or the explicitly selected local sidecar.
    pub endpoint: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssistancePurpose {
    Explain,
    SummarizeContext,
    SuggestLabels,
    ClarifyAmbiguity,
    DraftReportText,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LocalRisk {
    None,
    Low,
    Medium,
    High,
    Critical,
}

impl LocalRisk {
    pub(crate) const fn points(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Low => 20,
            Self::Medium => 50,
            Self::High => 75,
            Self::Critical => 100,
        }
    }
}

/// Host-owned evidence. `local_risk` and `reliability_basis_points` are never
/// sent to a provider: they are used only to score a validated suggestion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceInput {
    pub id: String,
    pub field_name: String,
    pub text: String,
    pub local_risk: LocalRisk,
    pub reliability_basis_points: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RedactionKind {
    ExplicitIdentifier,
    Email,
    Phone,
    Path,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionConfig {
    pub redact_paths: bool,
    pub redact_emails: bool,
    pub redact_phone_numbers: bool,
    #[serde(default)]
    pub identifiers: BTreeSet<String>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            redact_paths: true,
            redact_emails: true,
            redact_phone_numbers: true,
            identifiers: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssistanceRequest {
    pub request_id: String,
    pub purpose: AssistancePurpose,
    pub evidence: Vec<EvidenceInput>,
    pub redaction: RedactionConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionRecord {
    pub kind: RedactionKind,
    /// Offset in the text as it existed immediately before this replacement.
    pub byte_start: usize,
    pub original_bytes: usize,
    pub original_sha256: String,
    pub replacement: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosedField {
    pub evidence_id: String,
    pub field_name: String,
    /// The exact UTF-8 text sent to the provider after redaction.
    pub visible_text: String,
    pub visible_bytes: usize,
    pub source_sha256: String,
    pub visible_sha256: String,
    pub redactions: Vec<RedactionRecord>,
}

/// Exact, serializable preview of a single disclosure. Its digest binds cloud
/// consent to destination, purpose, prompt version, and transport bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureManifest {
    pub schema_version: String,
    pub request_id: String,
    pub purpose: AssistancePurpose,
    pub provider: ProviderDescriptor,
    pub prompt_version: String,
    pub fields: Vec<DisclosedField>,
    pub visible_content_bytes: usize,
    pub transport_bytes: usize,
    pub transport_sha256: String,
}

impl DisclosureManifest {
    /// SHA-256 over deterministic canonical JSON, excluding no fields.
    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self)
            .expect("DisclosureManifest has an infallible serde representation");
        sha256_hex(canonical_json(&value).as_bytes())
    }
}

/// Explicit one-request cloud opt-in. This is a consent marker, not an
/// authentication secret; credentials remain wholly inside CloudTransport.
#[derive(Debug, Eq, PartialEq)]
pub struct CloudConsentToken {
    disclosure_sha256: String,
    token_sha256: String,
}

impl CloudConsentToken {
    /// Create a token only after the caller has shown and accepted `manifest`.
    #[must_use]
    pub fn grant_for(manifest: &DisclosureManifest) -> Self {
        let disclosure_sha256 = manifest.digest();
        let mut binding = Vec::with_capacity(CONSENT_DOMAIN.len() + disclosure_sha256.len());
        binding.extend_from_slice(CONSENT_DOMAIN);
        binding.extend_from_slice(disclosure_sha256.as_bytes());
        Self {
            disclosure_sha256,
            token_sha256: sha256_hex(&binding),
        }
    }

    pub fn disclosure_sha256(&self) -> &str {
        &self.disclosure_sha256
    }

    pub(crate) fn token_sha256(&self) -> &str {
        &self.token_sha256
    }

    pub(crate) fn validates(&self, manifest: &DisclosureManifest) -> bool {
        let expected = Self::grant_for(manifest);
        constant_time_eq(
            self.disclosure_sha256.as_bytes(),
            expected.disclosure_sha256.as_bytes(),
        ) && constant_time_eq(
            self.token_sha256.as_bytes(),
            expected.token_sha256.as_bytes(),
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStatus {
    Disabled,
    ConsentRequired,
    ProviderFailed,
    Rejected,
    Accepted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureCode {
    ConsentMissingOrInvalid,
    ProviderMismatch,
    ProviderUnavailable,
    ResponseTooLarge,
    ResponseTooDeep,
    ResponseNotUtf8,
    MalformedJson,
    DuplicateJsonKey,
    SchemaViolation,
    ForbiddenContent,
    InvalidEvidenceReference,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RiskScore(pub u8);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ConfidenceScore(pub u16);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedSuggestion {
    pub id: String,
    pub label: String,
    pub explanation: String,
    pub evidence_ids: Vec<String>,
    /// Always false; no assisted-intelligence type carries an executable act.
    pub automatic_action: bool,
    /// Recomputed locally solely from the referenced host evidence.
    pub local_risk: RiskScore,
    /// 0..=10_000 basis points, recomputed locally.
    pub local_confidence: ConfidenceScore,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssistanceResult {
    pub explanation: String,
    pub suggestions: Vec<ValidatedSuggestion>,
    pub automatic_action: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    pub schema_version: String,
    pub request_id_sha256: String,
    pub purpose: AssistancePurpose,
    pub provider_kind: ProviderKind,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub prompt_sha256: String,
    /// Hash of the complete caller request, including host-only scoring inputs.
    pub source_request_sha256: String,
    /// Hash of the exact bytes supplied to the provider.
    pub transport_request_sha256: String,
    pub response_sha256: Option<String>,
    pub disclosure_sha256: String,
    pub opt_in_sha256: Option<String>,
    pub status: ExecutionStatus,
    pub failure: Option<FailureCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssistanceOutcome {
    pub audit: AuditRecord,
    pub result: Option<AssistanceResult>,
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn canonical_json(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, output: &mut String) {
        match value {
            serde_json::Value::Null => output.push_str("null"),
            serde_json::Value::Bool(value) => {
                output.push_str(if *value { "true" } else { "false" })
            }
            serde_json::Value::Number(value) => output.push_str(&value.to_string()),
            serde_json::Value::String(value) => output.push_str(
                &serde_json::to_string(value).expect("a JSON string is always serializable"),
            ),
            serde_json::Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write(value, output);
                }
                output.push(']');
            }
            serde_json::Value::Object(values) => {
                output.push('{');
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key)
                            .expect("a JSON object key is always serializable"),
                    );
                    output.push(':');
                    write(&values[key], output);
                }
                output.push('}');
            }
        }
    }

    let mut output = String::new();
    write(value, &mut output);
    output
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (&left, &right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}
