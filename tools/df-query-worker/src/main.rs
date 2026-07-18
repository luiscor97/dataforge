#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::path::PathBuf;

use df_db::extraction::AnalyticalSnapshotRecord;
use df_domain::{AnalyticalSnapshotId, ExtractionRunId, SnapshotId};
use df_query::worker_protocol::{
    WorkerRequest, WorkerResponse, MAX_REQUEST_BYTES, PROTOCOL_VERSION,
};

fn response_for(request: WorkerRequest) -> WorkerResponse {
    if request.protocol_version != PROTOCOL_VERSION {
        return WorkerResponse::error(format!(
            "unsupported request protocol {}",
            request.protocol_version
        ));
    }
    let artifact = AnalyticalSnapshotRecord {
        id: AnalyticalSnapshotId::new(),
        run_id: ExtractionRunId::new(),
        snapshot_id: SnapshotId::new(),
        schema_version: request.schema_version,
        relative_path: request.relative_path,
        sha256: request.sha256,
        rows: 0,
        created_at: chrono::Utc::now(),
    };
    let root = PathBuf::from(request.artifact_root.to_os_string());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return WorkerResponse::error(format!("runtime creation failed: {error}")),
    };
    match runtime.block_on(df_query::query_snapshot(
        &root,
        &artifact,
        &request.sql,
        request.options,
    )) {
        Ok(result) => WorkerResponse::Ok {
            protocol_version: PROTOCOL_VERSION,
            result,
        },
        Err(error) => WorkerResponse::error(error.to_string()),
    }
}

fn run() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES.saturating_add(1))
        .read_to_end(&mut input)
        .map_err(|error| format!("cannot read worker request: {error}"))?;
    let response = if u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_REQUEST_BYTES {
        WorkerResponse::error("request exceeds the worker byte limit")
    } else {
        match serde_json::from_slice::<WorkerRequest>(&input) {
            Ok(request) => response_for(request),
            Err(error) => WorkerResponse::error(format!("invalid request: {error}")),
        }
    };
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, &response)
        .map_err(|error| format!("cannot serialize worker response: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("cannot flush worker response: {error}"))
}

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(std::io::stderr(), "{error}");
        std::process::exit(1);
    }
}
