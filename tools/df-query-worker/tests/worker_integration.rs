use std::fs;
use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use df_db::extraction::AnalyticalSnapshotRecord;
use df_domain::{AnalyticalSnapshotId, ExtractionRunId, SnapshotId};
use df_query::{
    query_snapshot_isolated, QueryOptions, QueryWorkerConfig, ANALYTICAL_SCHEMA_VERSION,
};
use parquet::arrow::ArrowWriter;
use sha2::{Digest, Sha256};

fn worker_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_df-query-worker"))
}

fn fixture(root: &std::path::Path) -> AnalyticalSnapshotRecord {
    let relative_path = "fixture.parquet";
    let path = root.join(relative_path);
    let schema = Arc::new(Schema::new(vec![Field::new(
        "value",
        DataType::Utf8,
        false,
    )]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(StringArray::from(vec!["isolated"]))],
    )
    .unwrap();
    let file = fs::File::create(&path).unwrap();
    let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let sha256 = hex::encode(Sha256::digest(fs::read(&path).unwrap()));
    AnalyticalSnapshotRecord {
        id: AnalyticalSnapshotId::new(),
        run_id: ExtractionRunId::new(),
        snapshot_id: SnapshotId::new(),
        schema_version: ANALYTICAL_SCHEMA_VERSION.to_string(),
        relative_path: relative_path.to_string(),
        sha256,
        rows: 1,
        created_at: chrono::Utc::now(),
    }
}

#[cfg(windows)]
#[test]
fn isolated_worker_executes_select_and_rejects_ddl() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = fixture(temp.path());
    let worker = QueryWorkerConfig::new(worker_path());
    let result = query_snapshot_isolated(
        temp.path(),
        &artifact,
        "SELECT value FROM content",
        QueryOptions::default(),
        &worker,
    )
    .unwrap();
    assert_eq!(result.rows, vec![vec![Some("isolated".to_string())]]);

    let ddl = query_snapshot_isolated(
        temp.path(),
        &artifact,
        "CREATE VIEW escaped AS SELECT * FROM content",
        QueryOptions::default(),
        &worker,
    );
    assert!(ddl.is_err());
}

#[cfg(windows)]
#[test]
fn isolated_worker_refuses_tampered_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = fixture(temp.path());
    fs::write(
        temp.path().join(&artifact.relative_path),
        b"not parquet anymore",
    )
    .unwrap();
    let result = query_snapshot_isolated(
        temp.path(),
        &artifact,
        "SELECT * FROM content",
        QueryOptions::default(),
        &QueryWorkerConfig::new(worker_path()),
    );
    assert!(result.is_err());
}
