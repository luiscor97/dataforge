use std::path::{Path, PathBuf};
use std::time::Duration;

use df_db::extraction::AnalyticalSnapshotRecord;
use df_domain::RawPath;
use df_error::{DfError, DfResult};
use df_process_safety::{run_isolated, ProcessLimits};

use crate::worker_protocol::{WorkerRequest, WorkerResponse, MAX_REQUEST_BYTES, PROTOCOL_VERSION};
use crate::{QueryOptions, QueryResult, MAX_RESULT_BYTES};

const DEFAULT_WORKER_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_WORKER_MEMORY_BYTES: u64 = 384 * 1024 * 1024;
const MAX_WORKER_MEMORY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const WORKER_TIMEOUT_GRACE_SECONDS: u64 = 5;
const RESPONSE_OVERHEAD_BYTES: u64 = 4 * 1024 * 1024;

/// Explicit resource policy for the trusted DataFusion sidecar. The path is
/// never resolved through `PATH` or an environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryWorkerConfig {
    executable: PathBuf,
    memory_limit_bytes: u64,
}

impl QueryWorkerConfig {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            memory_limit_bytes: DEFAULT_WORKER_MEMORY_BYTES,
        }
    }

    #[must_use]
    pub fn with_memory_limit_bytes(mut self, bytes: u64) -> Self {
        self.memory_limit_bytes = bytes;
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn memory_limit_bytes(&self) -> u64 {
        self.memory_limit_bytes
    }

    fn validate(&self, options: QueryOptions) -> DfResult<()> {
        if !self.executable.is_absolute() {
            return Err(DfError::Validation(
                "analytical worker path must be absolute".to_string(),
            ));
        }
        if !(MIN_WORKER_MEMORY_BYTES..=MAX_WORKER_MEMORY_BYTES).contains(&self.memory_limit_bytes) {
            return Err(DfError::Validation(format!(
                "analytical worker memory must be between {MIN_WORKER_MEMORY_BYTES} and {MAX_WORKER_MEMORY_BYTES} bytes"
            )));
        }
        if self.memory_limit_bytes < options.memory_limit_bytes as u64 {
            return Err(DfError::Validation(
                "analytical worker memory must not be lower than the DataFusion memory limit"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Execute untrusted analytical SQL only inside the resource-isolated worker.
pub fn query_snapshot_isolated(
    artifact_root: &Path,
    artifact: &AnalyticalSnapshotRecord,
    sql: &str,
    options: QueryOptions,
    worker: &QueryWorkerConfig,
) -> DfResult<QueryResult> {
    let options = options.validate()?;
    worker.validate(options)?;
    super::validate_query_text(sql)?;
    let request = WorkerRequest {
        protocol_version: PROTOCOL_VERSION,
        artifact_root: RawPath::from_os_str(artifact_root.as_os_str()),
        schema_version: artifact.schema_version.clone(),
        relative_path: artifact.relative_path.clone(),
        sha256: artifact.sha256.clone(),
        sql: sql.to_string(),
        options,
    };
    let request = serde_json::to_vec(&request)
        .map_err(|error| DfError::Serialization(format!("analytical worker request: {error}")))?;
    if u64::try_from(request.len()).unwrap_or(u64::MAX) > MAX_REQUEST_BYTES {
        return Err(DfError::Validation(format!(
            "analytical worker request exceeds {MAX_REQUEST_BYTES} bytes"
        )));
    }
    // JSON escaping can expand a result by at most six bytes per input byte.
    let max_stdout_bytes = (options.max_result_bytes as u64)
        .saturating_mul(6)
        .saturating_add(RESPONSE_OVERHEAD_BYTES)
        .min((MAX_RESULT_BYTES as u64) * 6 + RESPONSE_OVERHEAD_BYTES);
    let timeout = Duration::from_secs(
        options
            .timeout_seconds
            .saturating_add(WORKER_TIMEOUT_GRACE_SECONDS),
    );
    let response = run_isolated(
        &worker.executable,
        &[],
        &request,
        ProcessLimits {
            timeout,
            memory_bytes: worker.memory_limit_bytes,
            max_stdin_bytes: MAX_REQUEST_BYTES,
            max_stdout_bytes,
        },
    )
    .map_err(|error| DfError::Validation(format!("analytical worker failed: {error}")))?;
    let response: WorkerResponse = serde_json::from_slice(&response).map_err(|error| {
        DfError::Serialization(format!("analytical worker response is invalid: {error}"))
    })?;
    match response {
        WorkerResponse::Ok {
            protocol_version,
            result,
        } if protocol_version == PROTOCOL_VERSION => Ok(result),
        WorkerResponse::Error {
            protocol_version,
            message,
        } if protocol_version == PROTOCOL_VERSION => Err(DfError::Validation(format!(
            "analytical SQL worker: {message}"
        ))),
        WorkerResponse::Ok {
            protocol_version, ..
        }
        | WorkerResponse::Error {
            protocol_version, ..
        } => Err(DfError::Validation(format!(
            "unsupported analytical worker protocol {protocol_version}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_policy_rejects_relative_path_and_memory_mismatch() {
        let options = QueryOptions::default();
        assert!(QueryWorkerConfig::new("df-query-worker.exe")
            .validate(options)
            .is_err());
        let executable = std::env::current_exe().unwrap();
        assert!(QueryWorkerConfig::new(executable)
            .with_memory_limit_bytes(MIN_WORKER_MEMORY_BYTES)
            .validate(QueryOptions {
                memory_limit_bytes: (MIN_WORKER_MEMORY_BYTES as usize) + 1,
                ..options
            })
            .is_err());
    }
}
