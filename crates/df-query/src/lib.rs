//! Bounded analytical snapshots and read-only SQL for M0.4.
//!
//! Parquet files are derived from immutable SQLite evidence. DataFusion is
//! exposed through a deliberately narrow API that does not register arbitrary
//! user paths or remote object stores.

#![forbid(unsafe_code)]

mod isolated;
#[doc(hidden)]
pub mod worker_protocol;

use std::fs;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use arrow_array::{Array, ArrayRef, BooleanArray, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use datafusion::arrow::util::display::array_value_to_string;
use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{ParquetReadOptions, SQLOptions, SessionConfig, SessionContext};
use df_db::extraction::{
    self, AnalyticalSnapshotRecord, IndexSubjectRow, EVENT_ANALYTICAL_SNAPSHOT_BUILT,
};
use df_db::Db;
use df_domain::{Actor, ExtractionRunId, ExtractionRunStatus};
use df_error::{DfError, DfResult};
use df_fs_safety::{identity_of_open_file, ReadLease, SafeOutputRoot, SafeRelativePath};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use isolated::{query_snapshot_isolated, QueryWorkerConfig};

/// Persisted Parquet column contract. Any column/type change gets a new value.
pub const ANALYTICAL_SCHEMA_VERSION: &str = "m0.4-parquet-v1";

const MAX_PAGE_SIZE: u32 = 16_384;
const MIN_QUERY_MEMORY_BYTES: usize = 32 * 1024 * 1024;
const MAX_QUERY_MEMORY_BYTES: usize = 1024 * 1024 * 1024;
const MAX_SQL_BYTES: usize = 16_384;
const MAX_QUERY_ROWS: usize = 10_000;
const MAX_QUERY_SECONDS: u64 = 300;
const MAX_CELL_CHARS: usize = 262_144;
const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;

/// Bounded settings for materialising one analytical snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotBuildOptions {
    pub page_size: u32,
    pub zstd_level: i32,
}

impl Default for SnapshotBuildOptions {
    fn default() -> Self {
        Self {
            page_size: 2_048,
            zstd_level: 3,
        }
    }
}

impl SnapshotBuildOptions {
    fn validate(self) -> DfResult<Self> {
        if self.page_size == 0 || self.page_size > MAX_PAGE_SIZE {
            return Err(DfError::Validation(format!(
                "analytical page_size must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        ZstdLevel::try_new(self.zstd_level)
            .map_err(|error| DfError::Validation(format!("invalid Parquet zstd level: {error}")))?;
        Ok(self)
    }
}

/// Runtime limits for one read-only DataFusion query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryOptions {
    pub max_rows: usize,
    pub max_result_bytes: usize,
    pub max_cell_chars: usize,
    pub memory_limit_bytes: usize,
    pub timeout_seconds: u64,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            max_rows: 1_000,
            max_result_bytes: 8 * 1024 * 1024,
            max_cell_chars: 65_536,
            memory_limit_bytes: 256 * 1024 * 1024,
            timeout_seconds: 30,
        }
    }
}

impl QueryOptions {
    pub(crate) fn validate(self) -> DfResult<Self> {
        if self.max_rows == 0 || self.max_rows > MAX_QUERY_ROWS {
            return Err(DfError::Validation(format!(
                "query max_rows must be between 1 and {MAX_QUERY_ROWS}"
            )));
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > MAX_RESULT_BYTES {
            return Err(DfError::Validation(format!(
                "query max_result_bytes must be between 1 and {MAX_RESULT_BYTES}"
            )));
        }
        if self.max_cell_chars == 0 || self.max_cell_chars > MAX_CELL_CHARS {
            return Err(DfError::Validation(format!(
                "query max_cell_chars must be between 1 and {MAX_CELL_CHARS}"
            )));
        }
        if !(MIN_QUERY_MEMORY_BYTES..=MAX_QUERY_MEMORY_BYTES).contains(&self.memory_limit_bytes) {
            return Err(DfError::Validation(format!(
                "query memory_limit_bytes must be between {MIN_QUERY_MEMORY_BYTES} and {MAX_QUERY_MEMORY_BYTES}"
            )));
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > MAX_QUERY_SECONDS {
            return Err(DfError::Validation(format!(
                "query timeout_seconds must be between 1 and {MAX_QUERY_SECONDS}"
            )));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryColumn {
    pub name: String,
    pub data_type: String,
}

/// Stable, frontend-neutral representation of a bounded SQL result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<Vec<Option<String>>>,
}

fn parquet_error(context: &str, error: impl std::fmt::Display) -> DfError {
    DfError::Serialization(format!("Parquet {context}: {error}"))
}

fn datafusion_error(context: &str, error: impl std::fmt::Display) -> DfError {
    DfError::Validation(format!("analytical SQL {context}: {error}"))
}

fn analytical_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("subject_id", DataType::Utf8, false),
        Field::new("content_id", DataType::Utf8, false),
        Field::new("subject_kind", DataType::Utf8, false),
        Field::new("file_name", DataType::Utf8, false),
        Field::new("extension", DataType::Utf8, true),
        Field::new("relative_path", DataType::Utf8, false),
        Field::new("representative_path", DataType::Utf8, false),
        Field::new("virtual_path", DataType::Utf8, true),
        Field::new("mime", DataType::Utf8, false),
        Field::new("context", DataType::Utf8, false),
        Field::new("size_bytes", DataType::UInt64, false),
        Field::new("normalized_chars", DataType::UInt64, false),
        Field::new("text_truncated", DataType::Boolean, false),
        Field::new("document_format", DataType::Utf8, false),
        Field::new("extraction_status", DataType::Utf8, false),
        Field::new("representation_error", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
        Field::new("mail_subject", DataType::Utf8, true),
        Field::new("mail_from_json", DataType::Utf8, false),
        Field::new("mail_to_json", DataType::Utf8, false),
        Field::new("metadata_json", DataType::Utf8, false),
    ]))
}

fn extension(file_name: &str) -> Option<String> {
    Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_lowercase())
}

fn rows_to_batch(rows: &[IndexSubjectRow], schema: Arc<Schema>) -> DfResult<RecordBatch> {
    let strings = |values: Vec<String>| Arc::new(StringArray::from(values)) as ArrayRef;
    let optional_strings =
        |values: Vec<Option<String>>| Arc::new(StringArray::from(values)) as ArrayRef;
    let arrays: Vec<ArrayRef> = vec![
        strings(rows.iter().map(|row| row.subject_id.to_string()).collect()),
        strings(rows.iter().map(|row| row.content_id.to_string()).collect()),
        strings(
            rows.iter()
                .map(|row| row.kind.as_str().to_string())
                .collect(),
        ),
        strings(rows.iter().map(|row| row.file_name.clone()).collect()),
        optional_strings(rows.iter().map(|row| extension(&row.file_name)).collect()),
        strings(rows.iter().map(|row| row.relative_path.clone()).collect()),
        strings(
            rows.iter()
                .map(|row| row.representative_path.clone())
                .collect(),
        ),
        optional_strings(rows.iter().map(|row| row.virtual_path.clone()).collect()),
        strings(rows.iter().map(|row| row.mime.clone()).collect()),
        strings(rows.iter().map(|row| row.context.clone()).collect()),
        Arc::new(UInt64Array::from(
            rows.iter().map(|row| row.size_bytes).collect::<Vec<_>>(),
        )),
        Arc::new(UInt64Array::from(
            rows.iter()
                .map(|row| row.normalized_chars)
                .collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter()
                .map(|row| row.text_truncated)
                .collect::<Vec<_>>(),
        )),
        strings(
            rows.iter()
                .map(|row| row.document_format.as_str().to_string())
                .collect(),
        ),
        strings(
            rows.iter()
                .map(|row| row.extraction_status.as_str().to_string())
                .collect(),
        ),
        optional_strings(
            rows.iter()
                .map(|row| row.representation_error.clone())
                .collect(),
        ),
        optional_strings(rows.iter().map(|row| row.title.clone()).collect()),
        optional_strings(rows.iter().map(|row| row.mail_subject.clone()).collect()),
        strings(
            rows.iter()
                .map(|row| serde_json::to_string(&row.mail_from))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    DfError::Serialization(format!("mail_from analytical JSON: {error}"))
                })?,
        ),
        strings(
            rows.iter()
                .map(|row| serde_json::to_string(&row.mail_to))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    DfError::Serialization(format!("mail_to analytical JSON: {error}"))
                })?,
        ),
        strings(
            rows.iter()
                .map(|row| serde_json::to_string(&row.metadata))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    DfError::Serialization(format!("metadata analytical JSON: {error}"))
                })?,
        ),
    ];
    RecordBatch::try_new(schema, arrays)
        .map_err(|error| DfError::Serialization(format!("analytical Arrow batch: {error}")))
}

fn artifact_relative_path(run_id: ExtractionRunId) -> DfResult<SafeRelativePath> {
    SafeRelativePath::parse(
        Path::new("snapshots")
            .join("analytical")
            .join(run_id.to_string())
            .join(format!("{}.parquet", uuid::Uuid::new_v4()))
            .as_path(),
    )
    .map_err(Into::into)
}

/// Materialise and register a compact Parquet view. Normalized text remains in
/// SQLite/Tantivy; analytical rows contain metadata and bounded JSON only.
pub fn build_analytical_snapshot(
    db: &mut Db,
    run_id: ExtractionRunId,
    artifact_root: &Path,
    options: SnapshotBuildOptions,
    actor: Actor,
) -> DfResult<AnalyticalSnapshotRecord> {
    let options = options.validate()?;
    let run = extraction::load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::InvalidTransition {
            from: run.status.as_str().to_string(),
            to: EVENT_ANALYTICAL_SNAPSHOT_BUILT.to_string(),
        });
    }
    let output = SafeOutputRoot::validate(artifact_root)?;
    let destination = artifact_relative_path(run_id)?;
    if let Some(parent) = destination.parent() {
        output.create_directory_secure(&parent)?;
    }
    let partial_name = format!(
        "{}.partial-{}",
        destination.file_name(),
        uuid::Uuid::new_v4()
    );
    let partial = destination.with_file_name(&partial_name)?;
    let file = output.create_partial_secure(&partial)?;
    let identity = identity_of_open_file(&file, artifact_root)?.ok_or_else(|| {
        DfError::Validation(
            "filesystem cannot provide strong identity for analytical partial".to_string(),
        )
    })?;
    let sync_file = file
        .try_clone()
        .map_err(|error| DfError::io(artifact_root.join(partial.to_path()), error))?;
    let schema = analytical_schema();
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(options.zstd_level).map_err(|error| {
                DfError::Validation(format!("invalid Parquet zstd level: {error}"))
            })?,
        ))
        .set_created_by(format!("dataforge/{ANALYTICAL_SCHEMA_VERSION}"))
        .build();
    let mut writer = ArrowWriter::try_new(file, Arc::clone(&schema), Some(properties))
        .map_err(|error| parquet_error("writer creation", error))?;

    let mut after: Option<String> = None;
    let mut row_count = 0_u64;
    loop {
        let rows =
            extraction::index_subjects_after(db, run_id, after.as_deref(), options.page_size)?;
        if rows.is_empty() {
            break;
        }
        writer
            .write(&rows_to_batch(&rows, Arc::clone(&schema))?)
            .map_err(|error| parquet_error("batch write", error))?;
        row_count = row_count
            .checked_add(rows.len() as u64)
            .ok_or_else(|| DfError::Validation("analytical row count overflow".to_string()))?;
        after = rows.last().map(|row| row.subject_id.to_string());
        if rows.len() < options.page_size as usize {
            break;
        }
    }
    writer
        .close()
        .map_err(|error| parquet_error("file close", error))?;
    sync_file
        .sync_all()
        .map_err(|error| DfError::io(artifact_root.join(partial.to_path()), error))?;

    let partial_path = artifact_root.join(partial.to_path());
    let sha256 = file_sha256_handle(&sync_file, &partial_path)?;
    // The digest came from the exact claimed handle. Release that read/write
    // handle before the handle-identity checked no-replace rename on Windows.
    drop(sync_file);
    output.finalize_claimed_partial_no_replace(&partial, &destination, identity)?;
    extraction::register_analytical_snapshot(
        db,
        run_id,
        ANALYTICAL_SCHEMA_VERSION,
        &destination.to_path().to_string_lossy(),
        &sha256,
        row_count,
        actor,
    )
}

fn registered_snapshot_lease(
    artifact_root: &Path,
    artifact: &AnalyticalSnapshotRecord,
) -> DfResult<ReadLease> {
    if artifact.schema_version != ANALYTICAL_SCHEMA_VERSION {
        return Err(DfError::Validation(format!(
            "unsupported analytical schema `{}`",
            artifact.schema_version
        )));
    }
    let output = SafeOutputRoot::validate(artifact_root)?;
    let relative = SafeRelativePath::parse(Path::new(&artifact.relative_path))?;
    let lease = output.lease_existing_file(&relative)?;
    let digest = file_sha256_handle(lease.file(), lease.path())?;
    if digest != artifact.sha256 {
        return Err(DfError::LedgerIntegrity(format!(
            "analytical artifact digest mismatch: expected {}, got {digest}",
            artifact.sha256
        )));
    }
    Ok(lease)
}

/// Execute one read-only statement against the sole registered `content`
/// table. DDL, DML, session statements, arbitrary paths, spill files and
/// unbounded result materialisation are disabled.
pub async fn query_snapshot(
    artifact_root: &Path,
    artifact: &AnalyticalSnapshotRecord,
    sql: &str,
    options: QueryOptions,
) -> DfResult<QueryResult> {
    let options = options.validate()?;
    let sql = validate_query_text(sql)?;
    execute_query_snapshot(artifact_root, artifact, sql, options).await
}

fn validate_query_text(sql: &str) -> DfResult<&str> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err(DfError::Validation("analytical SQL is empty".to_string()));
    }
    if sql.len() > MAX_SQL_BYTES {
        return Err(DfError::Validation(format!(
            "analytical SQL exceeds {MAX_SQL_BYTES} UTF-8 bytes"
        )));
    }
    // Keep the lease alive through registration, planning and execution. The
    // verified Parquet object and every path component therefore cannot be
    // modified, renamed or replaced between the digest check and DataFusion's
    // path-based reopen.
    Ok(sql)
}

async fn execute_query_snapshot(
    artifact_root: &Path,
    artifact: &AnalyticalSnapshotRecord,
    sql: &str,
    options: QueryOptions,
) -> DfResult<QueryResult> {
    // Keep the lease alive through registration, planning and execution. The
    // verified Parquet object and every path component therefore cannot be
    // modified, renamed or replaced between the digest check and DataFusion's
    // path-based reopen.
    let artifact_lease = registered_snapshot_lease(artifact_root, artifact)?;
    let runtime = RuntimeEnvBuilder::new()
        .with_memory_limit(options.memory_limit_bytes, 1.0)
        .with_disk_manager_builder(
            DiskManagerBuilder::default().with_mode(DiskManagerMode::Disabled),
        )
        .build_arc()
        .map_err(|error| datafusion_error("runtime creation", error))?;
    let config = SessionConfig::new()
        .with_target_partitions(1)
        .with_batch_size(options.max_rows.clamp(1, 8_192));
    let context = SessionContext::new_with_config_rt(config, runtime);
    let path_text = artifact_lease
        .path()
        .to_str()
        .ok_or_else(|| DfError::Validation("analytical path is not Unicode".to_string()))?;
    let execute = async {
        context
            .register_parquet("content", path_text, ParquetReadOptions::default())
            .await
            .map_err(|error| datafusion_error("snapshot registration", error))?;
        let sql_options = SQLOptions::new()
            .with_allow_ddl(false)
            .with_allow_dml(false)
            .with_allow_statements(false);
        let frame = context
            .sql_with_options(sql, sql_options)
            .await
            .map_err(|error| datafusion_error("planning", error))?
            .limit(0, Some(options.max_rows + 1))
            .map_err(|error| datafusion_error("result limit", error))?;
        frame
            .collect()
            .await
            .map_err(|error| datafusion_error("execution", error))
    };
    let batches = tokio::time::timeout(Duration::from_secs(options.timeout_seconds), execute)
        .await
        .map_err(|_| {
            DfError::Validation(format!(
                "analytical SQL exceeded {} seconds",
                options.timeout_seconds
            ))
        })??;
    batches_to_result(&batches, options)
}

fn batches_to_result(batches: &[RecordBatch], options: QueryOptions) -> DfResult<QueryResult> {
    let schema = batches
        .first()
        .map(RecordBatch::schema)
        .unwrap_or_else(|| Arc::new(Schema::empty()));
    let columns = schema
        .fields()
        .iter()
        .map(|field| QueryColumn {
            name: field.name().clone(),
            data_type: field.data_type().to_string(),
        })
        .collect();
    let mut rows = Vec::new();
    let mut result_bytes = 0_usize;
    for batch in batches {
        for row_index in 0..batch.num_rows() {
            if rows.len() == options.max_rows {
                return Err(DfError::Validation(format!(
                    "analytical result exceeds the configured {} row limit",
                    options.max_rows
                )));
            }
            let mut row = Vec::with_capacity(batch.num_columns());
            for array in batch.columns() {
                if array.is_null(row_index) {
                    row.push(None);
                    continue;
                }
                let value = array_value_to_string(array.as_ref(), row_index)
                    .map_err(|error| datafusion_error("result formatting", error))?;
                if value.chars().count() > options.max_cell_chars {
                    return Err(DfError::Validation(format!(
                        "analytical cell exceeds the configured {} character limit",
                        options.max_cell_chars
                    )));
                }
                result_bytes = result_bytes.checked_add(value.len()).ok_or_else(|| {
                    DfError::Validation("analytical result byte count overflow".to_string())
                })?;
                if result_bytes > options.max_result_bytes {
                    return Err(DfError::Validation(format!(
                        "analytical result exceeds the configured {} byte limit",
                        options.max_result_bytes
                    )));
                }
                row.push(Some(value));
            }
            rows.push(row);
        }
    }
    Ok(QueryResult { columns, rows })
}

#[cfg(test)]
fn file_sha256(path: &Path) -> DfResult<String> {
    let file = fs::File::open(path).map_err(|error| DfError::io(path, error))?;
    file_sha256_handle(&file, path)
}

fn file_sha256_handle(file: &fs::File, path: &Path) -> DfResult<String> {
    let mut file = file.try_clone().map_err(|error| DfError::io(path, error))?;
    file.rewind().map_err(|error| DfError::io(path, error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DfError::io(path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{
        AnalyticalSnapshotId, ContentId, DocumentFormat, ExtractionStatus, SnapshotId,
        TextSubjectId, TextSubjectKind,
    };
    use std::io::Write;

    fn test_artifact(root: &Path) -> AnalyticalSnapshotRecord {
        let row = IndexSubjectRow {
            run_id: ExtractionRunId::new(),
            subject_id: TextSubjectId::new(),
            content_id: ContentId::new(),
            kind: TextSubjectKind::Document,
            display_name: "report.txt".to_string(),
            virtual_path: None,
            mime: "text/plain".to_string(),
            metadata: serde_json::json!({"source": "test"}),
            size_bytes: 12,
            normalized_chars: 12,
            text_truncated: false,
            document_format: DocumentFormat::Text,
            extraction_status: ExtractionStatus::Extracted,
            representation_error: None,
            file_name: "report.txt".to_string(),
            relative_path: "case/report.txt".to_string(),
            representative_path: "C:/source/case/report.txt".to_string(),
            context: "case".to_string(),
            title: Some("Report".to_string()),
            mail_subject: None,
            mail_from: Vec::new(),
            mail_to: Vec::new(),
        };
        let relative_path = "fixture.parquet";
        let path = root.join(relative_path);
        let file = fs::File::create(&path).unwrap();
        let mut writer = ArrowWriter::try_new(file, analytical_schema(), None).unwrap();
        writer
            .write(&rows_to_batch(std::slice::from_ref(&row), analytical_schema()).unwrap())
            .unwrap();
        writer.close().unwrap();
        AnalyticalSnapshotRecord {
            id: AnalyticalSnapshotId::new(),
            run_id: row.run_id,
            snapshot_id: SnapshotId::new(),
            schema_version: ANALYTICAL_SCHEMA_VERSION.to_string(),
            relative_path: relative_path.to_string(),
            sha256: file_sha256(&path).unwrap(),
            rows: 1,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn options_reject_unbounded_work() {
        assert!(SnapshotBuildOptions {
            page_size: 0,
            ..SnapshotBuildOptions::default()
        }
        .validate()
        .is_err());
        assert!(QueryOptions {
            max_rows: MAX_QUERY_ROWS + 1,
            ..QueryOptions::default()
        }
        .validate()
        .is_err());
        assert!(QueryOptions {
            memory_limit_bytes: MIN_QUERY_MEMORY_BYTES - 1,
            ..QueryOptions::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn extension_is_normalized_without_guessing() {
        assert_eq!(extension("report.PDF").as_deref(), Some("pdf"));
        assert_eq!(extension("README"), None);
    }

    #[test]
    fn sql_is_read_only_bounded_and_integrity_checked() {
        let temp = tempfile::tempdir().unwrap();
        let artifact = test_artifact(temp.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result = runtime
            .block_on(query_snapshot(
                temp.path(),
                &artifact,
                "SELECT extension, size_bytes, context FROM content",
                QueryOptions::default(),
            ))
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0].as_deref(), Some("txt"));
        assert_eq!(result.rows[0][2].as_deref(), Some("case"));

        let ddl = runtime.block_on(query_snapshot(
            temp.path(),
            &artifact,
            "CREATE EXTERNAL TABLE escaped STORED AS PARQUET LOCATION 'C:/Windows/win.ini'",
            QueryOptions::default(),
        ));
        assert!(ddl.is_err(), "DDL must not register caller-selected paths");

        let allocating_function = runtime.block_on(query_snapshot(
            temp.path(),
            &artifact,
            "SELECT repeat('x', 1000000000) FROM content",
            QueryOptions::default(),
        ));
        assert!(
            allocating_function.is_err(),
            "optional allocation-amplifying string functions stay disabled"
        );

        fs::OpenOptions::new()
            .append(true)
            .open(temp.path().join(&artifact.relative_path))
            .unwrap()
            .write_all(b"tamper")
            .unwrap();
        let tampered = runtime.block_on(query_snapshot(
            temp.path(),
            &artifact,
            "SELECT * FROM content",
            QueryOptions::default(),
        ));
        assert!(matches!(tampered, Err(DfError::LedgerIntegrity(_))));
    }
}
