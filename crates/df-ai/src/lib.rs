//! Optional, non-executing assisted intelligence for M0.7.
//!
//! This crate deliberately stops at explanations and suggestions. It has no
//! filesystem mutation, shell, SQL, planning, approval, or execution API. A
//! request is prepared in two phases so a caller can inspect the exact
//! [`DisclosureManifest`] before granting consent for one cloud disclosure.

#![forbid(unsafe_code)]

mod engine;
mod provider;
mod redaction;
mod schema;
mod types;

pub use engine::{AssistanceEngine, PrepareError, PreparedAssistance};
pub use provider::{
    CloudProvider, CloudTransport, LocalProcessProvider, Provider, ProviderFailure,
};
pub use redaction::redact_text;
pub use schema::{output_schema, OUTPUT_SCHEMA_ID};
pub use types::{
    AiMode, AssistanceOutcome, AssistancePurpose, AssistanceRequest, AssistanceResult, AuditRecord,
    CloudConsentToken, ConfidenceScore, DisclosedField, DisclosureManifest, EvidenceInput,
    ExecutionStatus, FailureCode, LocalRisk, ProviderDescriptor, ProviderKind, RedactionConfig,
    RedactionKind, RedactionRecord, RiskScore, ValidatedSuggestion, AUDIT_SCHEMA_VERSION,
    DISCLOSURE_SCHEMA_VERSION, PROMPT_VERSION,
};

#[cfg(test)]
mod tests;
