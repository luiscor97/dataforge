//! Capability-based WebAssembly Component Model plugin host for M0.6.
//!
//! The ABI is intentionally suggestion-only. Components receive a bounded
//! JSON snapshot and return findings that conform to a host-owned, closed JSON
//! schema. There is no operation in the WIT world that can execute, approve,
//! or mutate a DataForge plan. ABI 0.1 exposes no host imports and the linker
//! contains no WASI implementation, so ambient filesystem, network,
//! environment, clock and random access are unavailable by construction.

#![forbid(unsafe_code)]

mod contract;
mod error;
mod host;
mod registry;

pub use contract::{
    output_schema, output_schema_sha256, AnalysisRequest, Capability, Finding, FindingSeverity,
    PluginInput, PluginManifest, PluginOutput, PluginSubject, SchemaReference, Suggestion,
    HOST_ABI_VERSION, INPUT_SCHEMA_VERSION, MANIFEST_SCHEMA_VERSION, OUTPUT_SCHEMA_ID,
};
pub use error::{LimitKind, PluginError, PluginResult};
pub use host::{HostLimits, HostPolicy, PluginHost};
pub use registry::{
    registration_signing_bytes, PluginKey, PluginRegistry, RegisteredPlugin,
    RegisteredPluginMetadata, SignedPluginPackage,
};

#[cfg(test)]
mod tests;
