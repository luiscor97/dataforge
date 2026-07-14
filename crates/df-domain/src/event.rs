//! Audit events (RFC-0001 §29).
//!
//! Events are append-only and hash-chained. The chain arithmetic lives in
//! `df-ledger`; this module only defines the shape of an event.

use serde::{Deserialize, Serialize};

use crate::{
    ids::{EventId, ProjectId},
    Timestamp,
};

/// Who caused an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    /// The engine itself (migrations, automatic transitions).
    System,
    /// A human driving the CLI.
    Cli,
    /// A human driving the desktop app.
    Desktop,
    /// Test code.
    Test,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Cli => "cli",
            Self::Desktop => "desktop",
            Self::Test => "test",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "system" => Ok(Self::System),
            "cli" => Ok(Self::Cli),
            "desktop" => Ok(Self::Desktop),
            "test" => Ok(Self::Test),
            other => Err(df_error::DfError::Validation(format!(
                "unknown actor `{other}`"
            ))),
        }
    }
}

/// One link of the audit ledger (RFC-0001 §29.1).
///
/// `payload_json` stores the raw payload so the chain can be re-verified;
/// `event_hash = SHA-256(previous_hash ‖ canonical_envelope)` where the
/// canonical envelope covers type, timestamp, actor, sequence and payload
/// (see `df-ledger`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: EventId,
    pub project_id: ProjectId,
    /// 1-based, contiguous per project.
    pub sequence: u64,
    pub timestamp: Timestamp,
    /// Hex SHA-256 of the previous event (64 zeros for the first event).
    pub previous_hash: String,
    pub event_type: String,
    /// Canonical JSON payload (sorted keys).
    pub payload_json: String,
    /// Hex SHA-256 of `payload_json`.
    pub payload_hash: String,
    pub actor: Actor,
    /// Hex SHA-256 chaining hash of this event.
    pub event_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_round_trips() {
        for actor in [Actor::System, Actor::Cli, Actor::Desktop, Actor::Test] {
            assert_eq!(Actor::parse(actor.as_str()).unwrap(), actor);
        }
        assert!(Actor::parse("robot").is_err());
    }
}
