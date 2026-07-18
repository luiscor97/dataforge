use thiserror::Error;

/// Result type for the isolated plugin boundary.
pub type PluginResult<T> = Result<T, PluginError>;

/// Stable class of a resource boundary enforced by the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitKind {
    ComponentBytes,
    InputBytes,
    OutputBytes,
    FuelOrEpoch,
    Memory,
    RuntimeResources,
}

/// Fail-closed errors produced before or during plugin execution.
#[derive(Debug, Error)]
pub enum PluginError {
    #[error("invalid plugin manifest: {0}")]
    InvalidManifest(String),

    #[error("plugin compatibility rejected: {0}")]
    Incompatible(String),

    #[error("plugin component hash does not match its signed package")]
    HashMismatch,

    #[error("plugin publisher key or signature is malformed")]
    MalformedSignature,

    #[error("plugin publisher signature verification failed")]
    SignatureInvalid,

    #[error("plugin `{0}` is already registered; registry entries are immutable")]
    AlreadyRegistered(String),

    #[error("plugin `{0}` is not registered")]
    NotRegistered(String),

    #[error("component is not a valid DataForge Component Model plugin: {0}")]
    InvalidComponent(String),

    #[error("component requested an unavailable host import or capability: {0}")]
    CapabilityDenied(String),

    #[error("plugin invocation exceeded the {kind:?} limit")]
    LimitExceeded { kind: LimitKind },

    #[error("plugin runtime trapped")]
    RuntimeTrap,

    #[error("plugin returned malformed UTF-8 or JSON: {0}")]
    MalformedOutput(String),

    #[error("plugin output does not conform to the closed findings schema: {0}")]
    OutputSchema(String),

    #[error("plugin violated the suggestion-only contract: automatic_action must be false")]
    AutomaticActionForbidden,

    #[error("plugin host configuration is invalid: {0}")]
    InvalidHostConfiguration(String),
}
