//! Rebuildable full-text search over immutable M0.4 extraction evidence.
//!
//! SQLite remains the source of truth. Tantivy indexes are disposable
//! artifacts and are accepted only after their complete directory has been
//! written and registered transactionally by `df-db`.

#![forbid(unsafe_code)]

use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use df_db::extraction::{self, IndexSubjectRow, SearchIndexRecord, EVENT_SEARCH_INDEX_BUILT};
use df_db::Db;
use df_domain::{Actor, ContentId, ExtractionRunId, ExtractionRunStatus, TextSubjectId};
use df_error::{DfError, DfResult};
use df_fs_safety::{metadata_is_reparse, ReadLease, SafeOutputRoot, SafeRelativePath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, TantivyDocument, Value, STORED, STRING, TEXT};
use tantivy::snippet::SnippetGenerator;
use tantivy::{Index, IndexWriter, ReloadPolicy};

/// Persisted schema contract. A field or analyzer change must use a new value.
pub const SEARCH_SCHEMA_VERSION: &str = "m0.4-tantivy-v1";

const MIN_WRITER_MEMORY_BYTES: usize = 15_000_000;
const MAX_WRITER_MEMORY_BYTES: usize = 1_073_741_824;
const MAX_PAGE_SIZE: u32 = 4_096;
const MAX_QUERY_BYTES: usize = 4_096;
const MAX_QUERY_TERMS: usize = 256;
const MAX_RESULTS: usize = 100;
const MAX_OFFSET: usize = 10_000;
const MAX_SNIPPET_CHARS: usize = 1_000;
const ENTITY_TEXT_LIMIT: usize = 65_536;
const MAX_INDEX_ENTRIES: usize = 10_000;
const MAX_INDEX_DIRECTORIES: usize = 1_024;
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_INDEX_RELATIVE_PATH_BYTES: usize = 4_096;

#[derive(Debug)]
struct LockedIndexFile {
    relative_path: String,
    lease: ReadLease,
}

#[derive(Debug)]
struct LockedIndexArtifact {
    path: PathBuf,
    _directories: Vec<ReadLease>,
    _operational_files: Vec<ReadLease>,
    files: Vec<LockedIndexFile>,
}

fn is_tantivy_operational_lock(relative: &Path) -> bool {
    relative.components().count() == 1
        && matches!(
            relative.file_name().and_then(|name| name.to_str()),
            Some(".tantivy-meta.lock" | ".tantivy-writer.lock")
        )
}

/// Resource bounds for a single, rebuildable index build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchBuildOptions {
    pub page_size: u32,
    pub writer_memory_bytes: usize,
}

impl Default for SearchBuildOptions {
    fn default() -> Self {
        Self {
            page_size: 512,
            writer_memory_bytes: 50_000_000,
        }
    }
}

impl SearchBuildOptions {
    fn validate(self) -> DfResult<Self> {
        if self.page_size == 0 || self.page_size > MAX_PAGE_SIZE {
            return Err(DfError::Validation(format!(
                "search page_size must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        if !(MIN_WRITER_MEMORY_BYTES..=MAX_WRITER_MEMORY_BYTES).contains(&self.writer_memory_bytes)
        {
            return Err(DfError::Validation(format!(
                "search writer_memory_bytes must be between {MIN_WRITER_MEMORY_BYTES} and {MAX_WRITER_MEMORY_BYTES}"
            )));
        }
        Ok(self)
    }
}

/// A bounded user query over one immutable index artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub limit: usize,
    pub offset: usize,
    pub snippet_chars: usize,
}

impl SearchRequest {
    fn validate(&self) -> DfResult<()> {
        let query = self.query.trim();
        if query.is_empty() {
            return Err(DfError::Validation("search query is empty".to_string()));
        }
        if self.query.len() > MAX_QUERY_BYTES {
            return Err(DfError::Validation(format!(
                "search query exceeds {MAX_QUERY_BYTES} UTF-8 bytes"
            )));
        }
        if query.split_whitespace().count() > MAX_QUERY_TERMS {
            return Err(DfError::Validation(format!(
                "search query exceeds {MAX_QUERY_TERMS} terms"
            )));
        }
        if self.limit == 0 || self.limit > MAX_RESULTS {
            return Err(DfError::Validation(format!(
                "search limit must be between 1 and {MAX_RESULTS}"
            )));
        }
        if self.offset > MAX_OFFSET {
            return Err(DfError::Validation(format!(
                "search offset cannot exceed {MAX_OFFSET}"
            )));
        }
        if self.snippet_chars == 0 || self.snippet_chars > MAX_SNIPPET_CHARS {
            return Err(DfError::Validation(format!(
                "snippet_chars must be between 1 and {MAX_SNIPPET_CHARS}"
            )));
        }
        Ok(())
    }
}

/// Search evidence returned to CLI, facade and desktop clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub subject_id: TextSubjectId,
    pub content_id: ContentId,
    pub score: f32,
    pub file_name: String,
    pub relative_path: String,
    pub representative_path: String,
    pub virtual_path: Option<String>,
    pub subject: String,
    pub context: String,
    pub mime: String,
    /// Plain text only. Highlight markup is deliberately not returned.
    pub snippet: String,
}

#[derive(Debug, Clone, Copy)]
struct SearchFields {
    subject_id: Field,
    content_id: Field,
    kind: Field,
    file_name: Field,
    relative_path: Field,
    representative_path: Field,
    virtual_path: Field,
    text: Field,
    subject: Field,
    from: Field,
    to: Field,
    entities: Field,
    context: Field,
    mime: Field,
}

fn search_schema() -> (Schema, SearchFields) {
    let mut builder = Schema::builder();
    let fields = SearchFields {
        subject_id: builder.add_text_field("subject_id", STRING | STORED),
        content_id: builder.add_text_field("content_id", STRING | STORED),
        kind: builder.add_text_field("kind", STRING | STORED),
        file_name: builder.add_text_field("file_name", TEXT | STORED),
        relative_path: builder.add_text_field("relative_path", TEXT | STORED),
        representative_path: builder.add_text_field("display_path", TEXT | STORED),
        virtual_path: builder.add_text_field("virtual_path", TEXT | STORED),
        text: builder.add_text_field("text", TEXT | STORED),
        subject: builder.add_text_field("subject", TEXT | STORED),
        from: builder.add_text_field("from", TEXT),
        to: builder.add_text_field("to", TEXT),
        entities: builder.add_text_field("entities", TEXT),
        context: builder.add_text_field("context", TEXT | STORED),
        mime: builder.add_text_field("mime", STRING | STORED),
    };
    (builder.build(), fields)
}

fn tantivy_error(context: &str, error: impl std::fmt::Display) -> DfError {
    DfError::Serialization(format!("Tantivy {context}: {error}"))
}

fn subject_heading(row: &IndexSubjectRow) -> &str {
    row.mail_subject
        .as_deref()
        .or(row.title.as_deref())
        .unwrap_or(&row.display_name)
}

fn entity_text(metadata: &serde_json::Value) -> String {
    let Some(entities) = metadata.get("entities") else {
        return String::new();
    };
    let mut value = match entities {
        serde_json::Value::Array(values) => values
            .iter()
            .take(128)
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::String(value) => value.clone(),
        _ => String::new(),
    };
    if value.len() > ENTITY_TEXT_LIMIT {
        let mut end = ENTITY_TEXT_LIMIT;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

fn add_row(
    writer: &mut IndexWriter,
    fields: SearchFields,
    row: &IndexSubjectRow,
    text: &str,
) -> DfResult<()> {
    let mut document = TantivyDocument::default();
    document.add_text(fields.subject_id, row.subject_id.to_string());
    document.add_text(fields.content_id, row.content_id.to_string());
    document.add_text(fields.kind, row.kind.as_str());
    document.add_text(fields.file_name, &row.file_name);
    document.add_text(fields.relative_path, &row.relative_path);
    document.add_text(fields.representative_path, &row.representative_path);
    if let Some(virtual_path) = row.virtual_path.as_deref() {
        document.add_text(fields.virtual_path, virtual_path);
    }
    document.add_text(fields.text, text);
    document.add_text(fields.subject, subject_heading(row));
    for address in &row.mail_from {
        document.add_text(fields.from, address);
    }
    for address in &row.mail_to {
        document.add_text(fields.to, address);
    }
    document.add_text(fields.entities, entity_text(&row.metadata));
    document.add_text(fields.context, &row.context);
    document.add_text(fields.mime, &row.mime);
    writer
        .add_document(document)
        .map_err(|error| tantivy_error("document write", error))?;
    Ok(())
}

fn artifact_relative_path(run_id: ExtractionRunId) -> DfResult<SafeRelativePath> {
    SafeRelativePath::parse(
        Path::new("indexes")
            .join("tantivy")
            .join(run_id.to_string())
            .join(uuid::Uuid::new_v4().to_string())
            .as_path(),
    )
    .map_err(Into::into)
}

/// Build and register a new immutable index. Existing artifacts are never
/// updated or replaced; rebuilding produces another registry record.
pub fn build_index(
    db: &mut Db,
    run_id: ExtractionRunId,
    artifact_root: &Path,
    options: SearchBuildOptions,
    actor: Actor,
) -> DfResult<SearchIndexRecord> {
    let options = options.validate()?;
    let run = extraction::load_run(db, run_id)?;
    if run.status != ExtractionRunStatus::Completed {
        return Err(DfError::InvalidTransition {
            from: run.status.as_str().to_string(),
            to: EVENT_SEARCH_INDEX_BUILT.to_string(),
        });
    }

    let output = SafeOutputRoot::validate(artifact_root)?;
    let relative = artifact_relative_path(run_id)?;
    output.create_directory_secure(&relative)?;
    // Acquire the directory lease before giving Tantivy a path. This pins the
    // newly-created directory and every ancestor, so a concurrent junction or
    // directory swap cannot redirect Tantivy's writes outside artifact_root.
    let root_lease = output.lease_existing_directory(&relative)?;
    let index_path = root_lease.path().to_path_buf();
    let (schema, fields) = search_schema();
    let index = Index::create_in_dir(&index_path, schema)
        .map_err(|error| tantivy_error("index creation", error))?;
    let mut writer = index
        .writer_with_num_threads(1, options.writer_memory_bytes)
        .map_err(|error| tantivy_error("writer creation", error))?;

    let mut after: Option<String> = None;
    let mut documents = 0_u64;
    loop {
        let rows =
            extraction::index_subjects_after(db, run_id, after.as_deref(), options.page_size)?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let text = extraction::load_subject_text(db, run_id, row.subject_id)?;
            add_row(&mut writer, fields, row, &text)?;
            documents = documents
                .checked_add(1)
                .ok_or_else(|| DfError::Validation("search document count overflow".to_string()))?;
        }
        after = rows.last().map(|row| row.subject_id.to_string());
        if rows.len() < options.page_size as usize {
            break;
        }
    }
    writer
        .commit()
        .map_err(|error| tantivy_error("commit", error))?;
    writer
        .wait_merging_threads()
        .map_err(|error| tantivy_error("merge completion", error))?;
    drop(index);

    let locked = lock_index_artifact_with_root(&output, &relative, root_lease)?;
    let content_digest = locked_directory_digest(&locked)?;
    extraction::register_search_index(
        db,
        run_id,
        SEARCH_SCHEMA_VERSION,
        &relative.to_path().to_string_lossy(),
        &content_digest,
        documents,
        actor,
    )
}

fn stored_text(document: &TantivyDocument, field: Field, name: &str) -> DfResult<String> {
    document
        .get_first(field)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| DfError::Serialization(format!("Tantivy document lacks `{name}`")))
}

fn optional_stored_text(document: &TantivyDocument, field: Field) -> Option<String> {
    document
        .get_first(field)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn registered_index_artifact(
    artifact_root: &Path,
    artifact: &SearchIndexRecord,
) -> DfResult<LockedIndexArtifact> {
    if artifact.schema_version != SEARCH_SCHEMA_VERSION {
        return Err(DfError::Validation(format!(
            "unsupported search schema `{}`",
            artifact.schema_version
        )));
    }
    let output = SafeOutputRoot::validate(artifact_root)?;
    let relative = SafeRelativePath::parse(Path::new(&artifact.relative_path))?;
    let locked = lock_index_artifact(&output, &relative)?;
    let digest = locked_directory_digest(&locked)?;
    if digest != artifact.content_digest {
        return Err(DfError::LedgerIntegrity(format!(
            "search artifact digest mismatch: expected {}, got {digest}",
            artifact.content_digest
        )));
    }
    Ok(locked)
}

/// Search a registered artifact after verifying its schema, safe path and
/// complete directory digest.
pub fn search_index(
    artifact_root: &Path,
    artifact: &SearchIndexRecord,
    request: &SearchRequest,
) -> DfResult<Vec<SearchHit>> {
    request.validate()?;
    // The directory and every indexed file stay leased for this whole scope.
    // Tantivy may reopen by path, but Windows sharing rules prevent a writer
    // or deleter from changing the verified objects in the interim.
    let locked = registered_index_artifact(artifact_root, artifact)?;
    let index =
        Index::open_in_dir(&locked.path).map_err(|error| tantivy_error("index open", error))?;
    let schema = index.schema();
    let fields = fields_from_schema(&schema)?;
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|error| tantivy_error("reader creation", error))?;
    let searcher = reader.searcher();
    let searchable = vec![
        fields.file_name,
        fields.relative_path,
        fields.text,
        fields.subject,
        fields.from,
        fields.to,
        fields.entities,
        fields.context,
        fields.mime,
    ];
    let parser = QueryParser::for_index(&index, searchable);
    let query = parser
        .parse_query(request.query.trim())
        .map_err(|error| DfError::Validation(format!("invalid search query: {error}")))?;
    let top_docs = searcher
        .search(
            &query,
            &TopDocs::with_limit(request.limit)
                .and_offset(request.offset)
                .order_by_score(),
        )
        .map_err(|error| tantivy_error("query execution", error))?;
    let mut snippet_generator = SnippetGenerator::create(&searcher, &*query, fields.text)
        .map_err(|error| tantivy_error("snippet creation", error))?;
    snippet_generator.set_max_num_chars(request.snippet_chars);

    let hits = top_docs
        .into_iter()
        .map(|(score, address)| {
            let document = searcher
                .doc::<TantivyDocument>(address)
                .map_err(|error| tantivy_error("stored document read", error))?;
            let subject_id =
                TextSubjectId::from_str(&stored_text(&document, fields.subject_id, "subject_id")?)?;
            let content_id =
                ContentId::from_str(&stored_text(&document, fields.content_id, "content_id")?)?;
            let snippet = snippet_generator.snippet_from_doc(&document);
            Ok(SearchHit {
                subject_id,
                content_id,
                score,
                file_name: stored_text(&document, fields.file_name, "file_name")?,
                relative_path: stored_text(&document, fields.relative_path, "relative_path")?,
                representative_path: stored_text(
                    &document,
                    fields.representative_path,
                    "display_path",
                )?,
                virtual_path: optional_stored_text(&document, fields.virtual_path),
                subject: stored_text(&document, fields.subject, "subject")?,
                context: stored_text(&document, fields.context, "context")?,
                mime: stored_text(&document, fields.mime, "mime")?,
                snippet: snippet.fragment().to_string(),
            })
        })
        .collect();
    drop(locked);
    hits
}

fn fields_from_schema(schema: &Schema) -> DfResult<SearchFields> {
    let field = |name: &str| {
        schema.get_field(name).map_err(|error| {
            DfError::Serialization(format!("Tantivy schema field `{name}`: {error}"))
        })
    };
    Ok(SearchFields {
        subject_id: field("subject_id")?,
        content_id: field("content_id")?,
        kind: field("kind")?,
        file_name: field("file_name")?,
        relative_path: field("relative_path")?,
        representative_path: field("display_path")?,
        virtual_path: field("virtual_path")?,
        text: field("text")?,
        subject: field("subject")?,
        from: field("from")?,
        to: field("to")?,
        entities: field("entities")?,
        context: field("context")?,
        mime: field("mime")?,
    })
}

/// Lock every directory and regular file under one registered index. Leases
/// close the check/use gap while the explicit ceilings keep a hostile artifact
/// tree from turning integrity verification into unbounded work.
fn lock_index_artifact(
    output: &SafeOutputRoot,
    relative: &SafeRelativePath,
) -> DfResult<LockedIndexArtifact> {
    let root = output.lease_existing_directory(relative)?;
    lock_index_artifact_with_root(output, relative, root)
}

fn lock_index_artifact_with_root(
    output: &SafeOutputRoot,
    relative: &SafeRelativePath,
    root: ReadLease,
) -> DfResult<LockedIndexArtifact> {
    let path = root.path().to_path_buf();
    let mut directories = vec![root];
    let mut operational_files = Vec::new();
    let mut files = Vec::new();
    let mut entries = 0_usize;
    let mut total_bytes = 0_u64;
    collect_locked_entries(
        output,
        relative,
        &path,
        &path,
        &mut directories,
        &mut operational_files,
        &mut files,
        &mut entries,
        &mut total_bytes,
    )?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(LockedIndexArtifact {
        path,
        _directories: directories,
        _operational_files: operational_files,
        files,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_locked_entries(
    output: &SafeOutputRoot,
    artifact_relative: &SafeRelativePath,
    artifact_path: &Path,
    directory: &Path,
    directories: &mut Vec<ReadLease>,
    operational_files: &mut Vec<ReadLease>,
    files: &mut Vec<LockedIndexFile>,
    entries: &mut usize,
    total_bytes: &mut u64,
) -> DfResult<()> {
    let listed = fs::read_dir(directory).map_err(|error| DfError::io(directory, error))?;
    for entry in listed {
        *entries = entries.checked_add(1).ok_or_else(|| {
            DfError::Validation("search artifact entry count overflow".to_string())
        })?;
        if *entries > MAX_INDEX_ENTRIES {
            return Err(DfError::Validation(format!(
                "search artifact exceeds {MAX_INDEX_ENTRIES} filesystem entries"
            )));
        }
        let entry = entry.map_err(|error| DfError::io(directory, error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| DfError::io(&path, error))?;
        if metadata_is_reparse(&metadata) {
            return Err(DfError::Validation(format!(
                "reparse point inside search artifact: {}",
                path.display()
            )));
        }
        let child = path
            .strip_prefix(artifact_path)
            .map_err(|_| DfError::Validation("search artifact escaped its root".to_string()))?;
        let name = child
            .to_str()
            .ok_or_else(|| DfError::Validation("search artifact path is not Unicode".to_string()))?
            .replace('\\', "/");
        if name.len() > MAX_INDEX_RELATIVE_PATH_BYTES {
            return Err(DfError::Validation(format!(
                "search artifact path exceeds {MAX_INDEX_RELATIVE_PATH_BYTES} bytes"
            )));
        }
        let full_relative =
            SafeRelativePath::parse(artifact_relative.to_path().join(child).as_path())?;
        if metadata.is_dir() {
            if directories.len() >= MAX_INDEX_DIRECTORIES {
                return Err(DfError::Validation(format!(
                    "search artifact exceeds {MAX_INDEX_DIRECTORIES} directories"
                )));
            }
            let lease = output.lease_existing_directory(&full_relative)?;
            let leased_path = lease.path().to_path_buf();
            directories.push(lease);
            collect_locked_entries(
                output,
                artifact_relative,
                artifact_path,
                &leased_path,
                directories,
                operational_files,
                files,
                entries,
                total_bytes,
            )?;
        } else if metadata.is_file() {
            if is_tantivy_operational_lock(child) {
                // Tantivy requires a writable lockfile even for reader setup.
                // It is operational state, not index evidence: pin its object
                // and path, allow cooperative writes, and exclude its mutable
                // bytes from the registered content digest.
                operational_files.push(output.lease_existing_mutable_file(&full_relative)?);
                continue;
            }
            let lease = output.lease_existing_file(&full_relative)?;
            let length = lease
                .file()
                .metadata()
                .map_err(|error| DfError::io(lease.path(), error))?
                .len();
            *total_bytes = total_bytes.checked_add(length).ok_or_else(|| {
                DfError::Validation("search artifact byte count overflow".to_string())
            })?;
            if *total_bytes > MAX_INDEX_BYTES {
                return Err(DfError::Validation(format!(
                    "search artifact exceeds {MAX_INDEX_BYTES} bytes"
                )));
            }
            files.push(LockedIndexFile {
                relative_path: name,
                lease,
            });
        } else {
            return Err(DfError::Validation(format!(
                "unsupported object inside search artifact: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Stable digest over locked relative file names, lengths and bytes.
fn locked_directory_digest(artifact: &LockedIndexArtifact) -> DfResult<String> {
    let mut digest = Sha256::new();
    for file in &artifact.files {
        let metadata = file
            .lease
            .file()
            .metadata()
            .map_err(|error| DfError::io(file.lease.path(), error))?;
        digest.update((file.relative_path.len() as u64).to_le_bytes());
        digest.update(file.relative_path.as_bytes());
        digest.update(metadata.len().to_le_bytes());
        let mut reader = file
            .lease
            .file()
            .try_clone()
            .map_err(|error| DfError::io(file.lease.path(), error))?;
        reader
            .rewind()
            .map_err(|error| DfError::io(file.lease.path(), error))?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| DfError::io(file.lease.path(), error))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_query_limits_fail_closed() {
        assert!(SearchBuildOptions {
            page_size: 0,
            ..SearchBuildOptions::default()
        }
        .validate()
        .is_err());
        assert!(SearchRequest {
            query: "".to_string(),
            limit: 10,
            offset: 0,
            snippet_chars: 200,
        }
        .validate()
        .is_err());
        assert!(SearchRequest {
            query: "valid".to_string(),
            limit: MAX_RESULTS + 1,
            offset: 0,
            snippet_chars: 200,
        }
        .validate()
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn locked_directory_digest_covers_names_and_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let output = SafeOutputRoot::validate(temp.path()).unwrap();
        let relative = SafeRelativePath::parse(Path::new("index")).unwrap();
        let path = output.create_directory_secure(&relative).unwrap();
        fs::write(path.join("a"), b"one").unwrap();
        let first =
            locked_directory_digest(&lock_index_artifact(&output, &relative).unwrap()).unwrap();
        fs::write(path.join("a"), b"two").unwrap();
        let second =
            locked_directory_digest(&lock_index_artifact(&output, &relative).unwrap()).unwrap();
        fs::rename(path.join("a"), path.join("b")).unwrap();
        let third =
            locked_directory_digest(&lock_index_artifact(&output, &relative).unwrap()).unwrap();
        assert_ne!(first, second);
        assert_ne!(second, third);
    }
}
