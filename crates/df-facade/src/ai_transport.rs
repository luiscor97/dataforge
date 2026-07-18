//! Cloud transports at the facade edge (Milestone 0.7).
//!
//! `df-ai` never links network code and never sees a credential: these
//! transports own authentication, map the provider-agnostic envelope to
//! each vendor's wire format and hand back only the model's JSON text for
//! the engine to validate against its closed schema. Errors are mapped to
//! the engine's `ProviderFailure` without reflecting response bodies, so a
//! hostile provider cannot smuggle text through error channels.

use std::time::Duration;

use df_ai::{CloudTransport, ProviderFailure};
use serde_json::{json, Value};

pub(crate) const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
pub(crate) const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

const TIMEOUT: Duration = Duration::from_secs(120);
/// Generous wire cap; the engine enforces its own 64 KiB response limit.
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_OUTPUT_TOKENS: u32 = 4096;

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .build(),
    )
}

fn system_prompt_of(envelope_json: &str) -> Result<String, ProviderFailure> {
    let value: Value =
        serde_json::from_str(envelope_json).map_err(|_| ProviderFailure::Protocol)?;
    value
        .get("system_prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(ProviderFailure::Protocol)
}

/// Build the Anthropic Messages API body for one envelope.
pub(crate) fn anthropic_body(envelope_json: &str, model: &str) -> Result<Value, ProviderFailure> {
    let system = system_prompt_of(envelope_json)?;
    Ok(json!({
        "model": model,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "system": system,
        "messages": [{ "role": "user", "content": envelope_json }],
    }))
}

/// Extract the model text from an Anthropic Messages API response.
pub(crate) fn anthropic_text(response: &[u8]) -> Result<Vec<u8>, ProviderFailure> {
    let value: Value = serde_json::from_slice(response).map_err(|_| ProviderFailure::Protocol)?;
    let text = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find_map(|block| {
                (block.get("type")? == "text").then(|| block.get("text")?.as_str())?
            })
        })
        .ok_or(ProviderFailure::Protocol)?;
    Ok(text.as_bytes().to_vec())
}

/// Build the OpenAI Chat Completions body for one envelope.
pub(crate) fn openai_body(envelope_json: &str, model: &str) -> Result<Value, ProviderFailure> {
    let system = system_prompt_of(envelope_json)?;
    Ok(json!({
        "model": model,
        "max_completion_tokens": MAX_OUTPUT_TOKENS,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": envelope_json },
        ],
    }))
}

/// Extract the model text from an OpenAI Chat Completions response.
pub(crate) fn openai_text(response: &[u8]) -> Result<Vec<u8>, ProviderFailure> {
    let value: Value = serde_json::from_slice(response).map_err(|_| ProviderFailure::Protocol)?;
    let text = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .ok_or(ProviderFailure::Protocol)?;
    Ok(text.as_bytes().to_vec())
}

fn post_json(
    endpoint: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<Vec<u8>, ProviderFailure> {
    let body_text = serde_json::to_string(body).map_err(|_| ProviderFailure::Protocol)?;
    let mut request = agent()
        .post(endpoint)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.send(&body_text).map_err(|error| match error {
        ureq::Error::Timeout(_) => ProviderFailure::Timeout,
        ureq::Error::StatusCode(status) if status == 429 || status >= 500 => {
            ProviderFailure::Unavailable
        }
        ureq::Error::StatusCode(_) => ProviderFailure::Protocol,
        _ => ProviderFailure::Unavailable,
    })?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_vec()
        .map_err(|_| ProviderFailure::LimitExceeded)
}

/// Anthropic Messages API transport. Owns the key; never logs it.
pub(crate) struct AnthropicTransport {
    pub api_key: String,
    pub model: String,
}

impl CloudTransport for AnthropicTransport {
    fn send(&self, endpoint: &str, request: &[u8]) -> Result<Vec<u8>, ProviderFailure> {
        let envelope = std::str::from_utf8(request).map_err(|_| ProviderFailure::Protocol)?;
        let body = anthropic_body(envelope, &self.model)?;
        let response = post_json(
            endpoint,
            &[
                ("x-api-key", self.api_key.as_str()),
                ("anthropic-version", "2023-06-01"),
            ],
            &body,
        )?;
        anthropic_text(&response)
    }
}

/// OpenAI Chat Completions transport. Owns the key; never logs it.
pub(crate) struct OpenAiTransport {
    pub api_key: String,
    pub model: String,
}

impl CloudTransport for OpenAiTransport {
    fn send(&self, endpoint: &str, request: &[u8]) -> Result<Vec<u8>, ProviderFailure> {
        let envelope = std::str::from_utf8(request).map_err(|_| ProviderFailure::Protocol)?;
        let body = openai_body(envelope, &self.model)?;
        let authorization = format!("Bearer {}", self.api_key);
        let response = post_json(
            endpoint,
            &[("authorization", authorization.as_str())],
            &body,
        )?;
        openai_text(&response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENVELOPE: &str = r#"{"schema_version":"v","prompt_version":"p","system_prompt":"You may only explain.","purpose":"EXPLAIN","evidence":[],"response_schema":{}}"#;

    #[test]
    fn vendor_bodies_carry_the_system_prompt_and_full_envelope() {
        let anthropic = anthropic_body(ENVELOPE, "claude-sonnet-5").unwrap();
        assert_eq!(anthropic["system"], "You may only explain.");
        assert_eq!(anthropic["messages"][0]["content"], ENVELOPE);

        let openai = openai_body(ENVELOPE, "gpt-test").unwrap();
        assert_eq!(openai["messages"][0]["content"], "You may only explain.");
        assert_eq!(openai["messages"][1]["content"], ENVELOPE);
        assert_eq!(openai["response_format"]["type"], "json_object");
    }

    #[test]
    fn model_text_is_extracted_and_garbage_is_a_protocol_failure() {
        let anthropic = br#"{"content":[{"type":"text","text":"{\"ok\":true}"}]}"#;
        assert_eq!(anthropic_text(anthropic).unwrap(), br#"{"ok":true}"#);
        assert_eq!(
            anthropic_text(b"not json").unwrap_err(),
            ProviderFailure::Protocol
        );

        let openai = br#"{"choices":[{"message":{"content":"{\"ok\":true}"}}]}"#;
        assert_eq!(openai_text(openai).unwrap(), br#"{"ok":true}"#);
        assert_eq!(
            openai_text(br#"{"choices":[]}"#).unwrap_err(),
            ProviderFailure::Protocol
        );
    }
}
