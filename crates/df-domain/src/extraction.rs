//! Deterministic document-intelligence evidence (Milestone 0.4).
//!
//! Physical contents, embedded mail attachments and virtual archive entries
//! remain distinct subjects. Extracted text is evidence for search and
//! analytics; it never changes SHA-256 identity or authorises a file action.

use serde::{Deserialize, Serialize};

use crate::{
    AnalyticalSnapshotId, ArchiveEntryId, ContentId, ExtractionRunId, MailAttachmentId,
    MailThreadId, ProjectId, RepresentationId, SearchIndexId, SnapshotId, TextSubjectId, Timestamp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExtractionRunStatus {
    Running,
    Completed,
    Failed,
}

impl ExtractionRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "RUNNING" => Ok(Self::Running),
            "COMPLETED" => Ok(Self::Completed),
            "FAILED" => Ok(Self::Failed),
            other => Err(df_error::DfError::Validation(format!(
                "unknown extraction run status `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExtractionStatus {
    Extracted,
    Unsupported,
    Limited,
    Failed,
}

impl ExtractionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extracted => "EXTRACTED",
            Self::Unsupported => "UNSUPPORTED",
            Self::Limited => "LIMITED",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "EXTRACTED" => Ok(Self::Extracted),
            "UNSUPPORTED" => Ok(Self::Unsupported),
            "LIMITED" => Ok(Self::Limited),
            "FAILED" => Ok(Self::Failed),
            other => Err(df_error::DfError::Validation(format!(
                "unknown extraction status `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DocumentFormat {
    Pdf,
    Docx,
    Text,
    Html,
    Eml,
    Zip,
    Unsupported,
}

impl DocumentFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Docx => "DOCX",
            Self::Text => "TXT",
            Self::Html => "HTML",
            Self::Eml => "EML",
            Self::Zip => "ZIP",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "PDF" => Ok(Self::Pdf),
            "DOCX" => Ok(Self::Docx),
            "TXT" => Ok(Self::Text),
            "HTML" => Ok(Self::Html),
            "EML" => Ok(Self::Eml),
            "ZIP" => Ok(Self::Zip),
            "UNSUPPORTED" => Ok(Self::Unsupported),
            other => Err(df_error::DfError::Validation(format!(
                "unknown document format `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TextSubjectKind {
    Document,
    MailAttachment,
    ArchiveEntry,
}

impl TextSubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "DOCUMENT",
            Self::MailAttachment => "MAIL_ATTACHMENT",
            Self::ArchiveEntry => "ARCHIVE_ENTRY",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "DOCUMENT" => Ok(Self::Document),
            "MAIL_ATTACHMENT" => Ok(Self::MailAttachment),
            "ARCHIVE_ENTRY" => Ok(Self::ArchiveEntry),
            other => Err(df_error::DfError::Validation(format!(
                "unknown text subject kind `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionRunCounters {
    pub contents_total: u64,
    pub extracted: u64,
    pub unsupported: u64,
    pub limited: u64,
    pub failed: u64,
    pub text_subjects: u64,
    pub text_segments: u64,
    pub mail_messages: u64,
    pub mail_threads: u64,
    pub mail_attachments: u64,
    pub archive_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionRun {
    pub id: ExtractionRunId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub status: ExtractionRunStatus,
    pub extractor_version: String,
    pub config_digest: String,
    pub config: serde_json::Value,
    pub counters: ExtractionRunCounters,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRepresentation {
    pub id: RepresentationId,
    pub content_id: ContentId,
    pub extractor_version: String,
    pub config_digest: String,
    pub format: DocumentFormat,
    pub mime: String,
    pub status: ExtractionStatus,
    pub title: Option<String>,
    pub normalized_text_sha256: Option<String>,
    pub normalized_chars: u64,
    pub text_truncated: bool,
    pub metadata: serde_json::Value,
    pub error: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSubject {
    pub id: TextSubjectId,
    pub representation_id: RepresentationId,
    pub kind: TextSubjectKind,
    pub parent_subject_id: Option<TextSubjectId>,
    pub display_name: String,
    pub virtual_path: Option<String>,
    pub mime: String,
    pub size_bytes: u64,
    pub normalized_text_sha256: Option<String>,
    pub normalized_chars: u64,
    pub text_truncated: bool,
    pub metadata: serde_json::Value,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailMessage {
    pub representation_id: RepresentationId,
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub sent_at: Option<String>,
    pub subject: Option<String>,
    pub normalized_subject: Option<String>,
    pub body_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAttachment {
    pub id: MailAttachmentId,
    pub representation_id: RepresentationId,
    pub subject_id: TextSubjectId,
    pub ordinal: u32,
    pub file_name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub extraction_status: ExtractionStatus,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub id: ArchiveEntryId,
    pub representation_id: RepresentationId,
    pub subject_id: Option<TextSubjectId>,
    pub ordinal: u32,
    pub virtual_path: String,
    pub compressed_bytes: u64,
    pub size_bytes: u64,
    pub crc32: u32,
    pub encrypted: bool,
    pub directory: bool,
    pub sha256: Option<String>,
    pub extraction_status: ExtractionStatus,
    pub created_at: Timestamp,
}

/// A deterministic, run-local grouping of related EML representations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailThread {
    pub id: MailThreadId,
    pub run_id: ExtractionRunId,
    pub snapshot_id: SnapshotId,
    pub root_message_id: Option<String>,
    pub normalized_subject: Option<String>,
    pub message_count: u64,
    pub created_at: Timestamp,
}

/// One ordered EML representation within a reconstructed thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailThreadMember {
    pub thread_id: MailThreadId,
    pub run_id: ExtractionRunId,
    pub representation_id: RepresentationId,
    pub parent_representation_id: Option<RepresentationId>,
    pub ordinal: u64,
    pub created_at: Timestamp,
}

/// Immutable registry record for a disposable Tantivy index directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexArtifact {
    pub id: SearchIndexId,
    pub run_id: ExtractionRunId,
    pub snapshot_id: SnapshotId,
    pub schema_version: String,
    pub relative_path: String,
    pub content_digest: String,
    pub documents: u64,
    pub created_at: Timestamp,
}

/// Immutable registry record for a disposable analytical Parquet file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticalSnapshotArtifact {
    pub id: AnalyticalSnapshotId,
    pub run_id: ExtractionRunId,
    pub snapshot_id: SnapshotId,
    pub schema_version: String,
    pub relative_path: String,
    pub sha256: String,
    pub rows: u64,
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_extraction_enums_round_trip() {
        for status in [
            ExtractionStatus::Extracted,
            ExtractionStatus::Unsupported,
            ExtractionStatus::Limited,
            ExtractionStatus::Failed,
        ] {
            assert_eq!(ExtractionStatus::parse(status.as_str()).unwrap(), status);
        }
        for format in [
            DocumentFormat::Pdf,
            DocumentFormat::Docx,
            DocumentFormat::Text,
            DocumentFormat::Html,
            DocumentFormat::Eml,
            DocumentFormat::Zip,
            DocumentFormat::Unsupported,
        ] {
            assert_eq!(DocumentFormat::parse(format.as_str()).unwrap(), format);
        }
    }
}
