//! Versioned internal protocol for the resource-isolated DataFusion sidecar.

use df_domain::RawPath;
use serde::{Deserialize, Serialize};

use crate::{QueryOptions, QueryResult};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_REQUEST_BYTES: u64 = 256 * 1024;
pub const MAX_ERROR_CHARS: usize = 4_096;

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerRequest {
    pub protocol_version: u16,
    pub artifact_root: RawPath,
    pub schema_version: String,
    pub relative_path: String,
    pub sha256: String,
    pub sql: String,
    pub options: QueryOptions,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkerResponse {
    Ok {
        protocol_version: u16,
        result: QueryResult,
    },
    Error {
        protocol_version: u16,
        message: String,
    },
}

impl WorkerResponse {
    pub fn error(message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.chars().count() > MAX_ERROR_CHARS {
            message = message.chars().take(MAX_ERROR_CHARS).collect();
        }
        Self::Error {
            protocol_version: PROTOCOL_VERSION,
            message,
        }
    }
}
