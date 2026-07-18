use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use ed25519_dalek::{Signature, VerifyingKey};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contract::{
    output_schema_sha256, PluginManifest, HOST_ABI_VERSION, MANIFEST_SCHEMA_VERSION,
    OUTPUT_SCHEMA_ID,
};
use crate::error::{LimitKind, PluginError, PluginResult};

const SIGNING_DOMAIN: &[u8] = b"DataForge signed plugin package\0v1\0";

/// Immutable registry key.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginKey {
    pub plugin_id: String,
    pub plugin_version: String,
}

impl fmt::Display for PluginKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.plugin_id, self.plugin_version)
    }
}

/// Signed transport envelope. The declared hash and manifest are covered by
/// the Ed25519 signature; the component bytes are verified against the hash
/// before the entry can be inserted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedPluginPackage {
    pub manifest: PluginManifest,
    pub component_sha256: String,
    pub component_bytes: Vec<u8>,
    pub publisher_public_key_hex: String,
    pub signature_hex: String,
}

/// Serializable view suitable for persistence by a higher layer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredPluginMetadata {
    pub key: PluginKey,
    pub manifest: PluginManifest,
    pub component_sha256: String,
    pub component_bytes: u64,
    pub publisher_public_key_hex: String,
    pub publisher_key_sha256: String,
    pub signature_hex: String,
}

/// An immutable, content-addressed component and its verified metadata.
#[derive(Clone, Debug)]
pub struct RegisteredPlugin {
    metadata: RegisteredPluginMetadata,
    component_bytes: Arc<[u8]>,
}

impl RegisteredPlugin {
    pub fn metadata(&self) -> &RegisteredPluginMetadata {
        &self.metadata
    }

    pub fn component_bytes(&self) -> &[u8] {
        &self.component_bytes
    }
}

/// Append-only in-memory registry. There is deliberately no update, replace,
/// or remove method; a plugin version is a permanent content identity.
#[derive(Clone, Debug, Default)]
pub struct PluginRegistry {
    entries: BTreeMap<PluginKey, RegisteredPlugin>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &PluginKey) -> Option<&RegisteredPlugin> {
        self.entries.get(key)
    }

    pub fn metadata(&self) -> Vec<RegisteredPluginMetadata> {
        self.entries
            .values()
            .map(|entry| entry.metadata.clone())
            .collect()
    }

    /// Verify and append a package. A host should validate Component Model
    /// structure before calling this method; [`crate::PluginHost::register`]
    /// performs both checks in that fail-closed order.
    pub fn register(
        &mut self,
        package: SignedPluginPackage,
        max_component_bytes: usize,
    ) -> PluginResult<PluginKey> {
        let (key, plugin) = verify_package(package, max_component_bytes)?;
        if self.entries.contains_key(&key) {
            return Err(PluginError::AlreadyRegistered(key.to_string()));
        }
        self.entries.insert(key.clone(), plugin);
        Ok(key)
    }
}

/// Domain-separated bytes publishers sign. The encoding is a fixed prefix,
/// the big-endian manifest length, the compact deterministic manifest JSON,
/// and the lowercase component SHA-256.
pub fn registration_signing_bytes(
    manifest: &PluginManifest,
    component_sha256: &str,
) -> PluginResult<Vec<u8>> {
    if !is_lower_sha256(component_sha256) {
        return Err(PluginError::InvalidManifest(
            "component_sha256 must be 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    let manifest_json = serde_json::to_vec(manifest)
        .map_err(|error| PluginError::InvalidManifest(error.to_string()))?;
    let length = u64::try_from(manifest_json.len()).map_err(|_| {
        PluginError::InvalidManifest("manifest serialization is too large".to_string())
    })?;
    let mut message = Vec::with_capacity(
        SIGNING_DOMAIN.len() + std::mem::size_of::<u64>() + manifest_json.len() + 64,
    );
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(&manifest_json);
    message.extend_from_slice(component_sha256.as_bytes());
    Ok(message)
}

fn verify_package(
    package: SignedPluginPackage,
    max_component_bytes: usize,
) -> PluginResult<(PluginKey, RegisteredPlugin)> {
    if package.component_bytes.len() > max_component_bytes {
        return Err(PluginError::LimitExceeded {
            kind: LimitKind::ComponentBytes,
        });
    }
    validate_manifest(&package.manifest)?;

    if !is_lower_sha256(&package.component_sha256) {
        return Err(PluginError::InvalidManifest(
            "component_sha256 must be 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    let observed_hash = hex::encode(Sha256::digest(&package.component_bytes));
    if observed_hash != package.component_sha256 {
        return Err(PluginError::HashMismatch);
    }

    let public_key_bytes = decode_exact::<32>(&package.publisher_public_key_hex)
        .ok_or(PluginError::MalformedSignature)?;
    let signature_bytes =
        decode_exact::<64>(&package.signature_hex).ok_or(PluginError::MalformedSignature)?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_key_bytes).map_err(|_| PluginError::MalformedSignature)?;
    let signature = Signature::from_bytes(&signature_bytes);
    let message = registration_signing_bytes(&package.manifest, &package.component_sha256)?;
    verifying_key
        .verify_strict(&message, &signature)
        .map_err(|_| PluginError::SignatureInvalid)?;

    let key = PluginKey {
        plugin_id: package.manifest.plugin_id.clone(),
        plugin_version: package.manifest.plugin_version.clone(),
    };
    let metadata = RegisteredPluginMetadata {
        key: key.clone(),
        manifest: package.manifest,
        component_sha256: package.component_sha256,
        component_bytes: u64::try_from(package.component_bytes.len()).map_err(|_| {
            PluginError::InvalidManifest("component length is not representable".to_string())
        })?,
        publisher_key_sha256: hex::encode(Sha256::digest(public_key_bytes)),
        publisher_public_key_hex: package.publisher_public_key_hex,
        signature_hex: package.signature_hex,
    };
    let plugin = RegisteredPlugin {
        metadata,
        component_bytes: Arc::from(package.component_bytes),
    };
    Ok((key, plugin))
}

fn validate_manifest(manifest: &PluginManifest) -> PluginResult<()> {
    if manifest.manifest_schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(PluginError::Incompatible(format!(
            "manifest schema `{}` is not `{MANIFEST_SCHEMA_VERSION}`",
            manifest.manifest_schema_version
        )));
    }
    if !valid_plugin_id(&manifest.plugin_id) {
        return Err(PluginError::InvalidManifest(
            "plugin_id must be 3..=128 lowercase ASCII letters, digits, dots or hyphens"
                .to_string(),
        ));
    }
    Version::parse(&manifest.plugin_version).map_err(|error| {
        PluginError::InvalidManifest(format!(
            "plugin_version is not semantic versioning: {error}"
        ))
    })?;
    let abi = Version::parse(&manifest.abi_version).map_err(|error| {
        PluginError::InvalidManifest(format!("abi_version must be an exact version: {error}"))
    })?;
    let host_abi = Version::parse(HOST_ABI_VERSION)
        .expect("the compiled host ABI constant is valid semantic versioning");
    if abi != host_abi {
        return Err(PluginError::Incompatible(format!(
            "component ABI `{abi}` is not the exact supported ABI `{host_abi}`"
        )));
    }
    let host_requirement = VersionReq::parse(&manifest.host_compatibility).map_err(|error| {
        PluginError::InvalidManifest(format!(
            "host_compatibility is not a semantic-version range: {error}"
        ))
    })?;
    if !host_requirement.matches(&host_abi) {
        return Err(PluginError::Incompatible(format!(
            "host ABI `{host_abi}` is outside `{host_requirement}`"
        )));
    }
    if manifest.publisher.trim().is_empty() || manifest.publisher.len() > 256 {
        return Err(PluginError::InvalidManifest(
            "publisher must contain 1..=256 bytes".to_string(),
        ));
    }
    if manifest.capabilities.len() > 16 {
        return Err(PluginError::InvalidManifest(
            "too many declared capabilities".to_string(),
        ));
    }
    if manifest.output_schema.id != OUTPUT_SCHEMA_ID
        || manifest.output_schema.sha256 != output_schema_sha256()
    {
        return Err(PluginError::Incompatible(
            "plugin output schema is not the exact host-owned ABI schema".to_string(),
        ));
    }
    if manifest.automatic_action {
        return Err(PluginError::AutomaticActionForbidden);
    }
    Ok(())
}

fn valid_plugin_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
        })
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_exact<const N: usize>(value: &str) -> Option<[u8; N]> {
    let decoded = hex::decode(value).ok()?;
    decoded.try_into().ok()
}
