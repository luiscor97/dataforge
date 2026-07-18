use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const OUTPUT_SCHEMA_ID: &str = "dataforge.ai-suggestions/0.7.0";

/// Closed Draft 2020-12 schema for the only provider response accepted by the
/// host. There is intentionally no action, command, path, SQL, tool-call,
/// risk, or confidence field.
pub fn output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": OUTPUT_SCHEMA_ID,
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "automatic_action", "explanation", "suggestions"],
        "properties": {
            "schema_version": { "const": OUTPUT_SCHEMA_ID },
            "automatic_action": { "const": false },
            "explanation": { "type": "string", "minLength": 1, "maxLength": 4096 },
            "suggestions": {
                "type": "array",
                "maxItems": 32,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "label", "explanation", "evidence_ids"],
                    "properties": {
                        "id": {
                            "type": "string",
                            "pattern": "^[A-Z][A-Z0-9_]{0,63}$"
                        },
                        "label": {
                            "type": "string",
                            "pattern": "^[A-Za-z0-9][A-Za-z0-9 _.-]{0,127}$"
                        },
                        "explanation": {
                            "type": "string", "minLength": 1, "maxLength": 4096
                        },
                        "evidence_ids": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 16,
                            "uniqueItems": true,
                            "items": {
                                "type": "string",
                                "pattern": "^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$"
                            }
                        }
                    }
                }
            }
        }
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelOutput {
    pub schema_version: String,
    pub automatic_action: bool,
    pub explanation: String,
    pub suggestions: Vec<ModelSuggestion>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelSuggestion {
    pub id: String,
    pub label: String,
    pub explanation: String,
    pub evidence_ids: Vec<String>,
}
