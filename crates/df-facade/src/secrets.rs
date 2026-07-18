//! BYOK secret store (Milestone 0.7).
//!
//! API keys live in the operating system's credential vault — Windows
//! Credential Manager via `keyring` — and nowhere else: never in SQLite,
//! never in configuration files, never in the ledger, never in logs or
//! error messages. This module hands the value only to the cloud transport
//! that authenticates with it.

use df_error::{DfError, DfResult};

const SERVICE: &str = "DataForge";

/// Cloud providers the facade can hold one key for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AiKeyProvider {
    Anthropic,
    OpenAi,
}

impl AiKeyProvider {
    pub fn parse(value: &str) -> DfResult<Self> {
        match value.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            other => Err(DfError::Validation(format!(
                "unknown AI provider `{other}` (expected `anthropic` or `openai`)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }

    fn account(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic-api-key",
            Self::OpenAi => "openai-api-key",
        }
    }
}

fn entry(provider: AiKeyProvider) -> DfResult<keyring::Entry> {
    keyring::Entry::new(SERVICE, provider.account())
        .map_err(|error| DfError::Validation(format!("credential store unavailable: {error}")))
}

/// Store (or replace) the key for one provider.
pub fn set_ai_key(provider: AiKeyProvider, key: &str) -> DfResult<()> {
    let key = key.trim();
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        return Err(DfError::Validation(
            "the API key must be a non-empty single line of at most 512 characters".to_string(),
        ));
    }
    entry(provider)?
        .set_password(key)
        .map_err(|error| DfError::Validation(format!("credential store rejected the key: {error}")))
}

/// Remove the stored key, failing if none exists.
pub fn remove_ai_key(provider: AiKeyProvider) -> DfResult<()> {
    match entry(provider)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Err(DfError::Validation(format!(
            "no API key is stored for `{}`",
            provider.as_str()
        ))),
        Err(error) => Err(DfError::Validation(format!(
            "credential store failed: {error}"
        ))),
    }
}

/// Whether a key is present; the value itself is never exposed.
pub fn ai_key_present(provider: AiKeyProvider) -> DfResult<bool> {
    match entry(provider)?.get_password() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(DfError::Validation(format!(
            "credential store failed: {error}"
        ))),
    }
}

/// The key for the transport. Crate-internal on purpose: the only consumer
/// is the cloud transport constructor.
pub(crate) fn ai_key(provider: AiKeyProvider) -> DfResult<String> {
    match entry(provider)?.get_password() {
        Ok(key) => Ok(key),
        Err(keyring::Error::NoEntry) => Err(DfError::Validation(format!(
            "no API key is stored for `{}`; run `dataforge ai key set --provider {}` first",
            provider.as_str(),
            provider.as_str()
        ))),
        Err(error) => Err(DfError::Validation(format!(
            "credential store failed: {error}"
        ))),
    }
}
