//! Bounded, deterministic content extraction for DataForge.
//!
//! This crate is deliberately side-effect free: callers supply immutable
//! bytes and receive domain evidence. ZIP/DOCX entries and EML attachments are
//! virtual subjects; no API here can materialize them on disk.

#![deny(unsafe_code)]

mod archive;
mod formats;
mod limits;
mod mail;
mod normalize;
mod pdf_worker;
#[doc(hidden)]
pub mod worker_protocol;
mod zip_safety;

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use df_domain::{
    ArchiveEntry, ArchiveEntryId, ContentId, DocumentFormat, DocumentRepresentation,
    ExtractionStatus, MailAttachment, MailAttachmentId, MailMessage, RepresentationId, TextSubject,
    TextSubjectId, TextSubjectKind,
};
use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use archive::scan_archive;
use formats::{canonical_mime, detect_format as detect, extract_text_payload};
use mail::parse_mail;
use normalize::{normalize_text, segment_text, sha256_hex};
use pdf_worker::PdfWorkerOutcome;

pub use formats::detect_format;
pub use limits::ExtractionLimits;
pub use normalize::{NormalizedText, TextSegmentDraft};
pub use pdf_worker::PdfWorkerConfig;

/// Semantic backend identifier persisted with each representation. Changing
/// normalization or dispatch semantics requires changing this identifier.
///
/// Frozen as a literal (ADR-0037): this is an algorithm identity sealed
/// into existing evidence, not the software version. Deriving it from the
/// crate version would silently re-key every stored representation on a
/// release bump with unchanged semantics. The leading "0.2.0" is the
/// historical token under which this identity was first sealed.
pub const EXTRACTOR_VERSION: &str = "0.2.0+content-v1";

/// Immutable input and caller-owned lineage for one content object.
#[derive(Debug, Clone, Copy)]
pub struct ExtractionRequest<'a> {
    pub content_id: ContentId,
    pub representation_id: RepresentationId,
    pub source_sha256: &'a str,
    pub source_size_bytes: u64,
    pub display_name: &'a str,
    pub mime_hint: Option<&'a str>,
    pub bytes: &'a [u8],
    pub extractor_version: &'a str,
    pub config_digest: &'a str,
    pub created_at: DateTime<Utc>,
}

/// Persistable segment tied to a domain text subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedTextSegment {
    pub subject_id: TextSubjectId,
    pub ordinal: u32,
    pub char_start: u64,
    pub char_end: u64,
    pub text: String,
    pub sha256: String,
}

/// Complete atomic persistence unit for one physical content representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionBundle {
    pub source_sha256: String,
    pub representation: DocumentRepresentation,
    pub text_subjects: Vec<TextSubject>,
    pub text_segments: Vec<ExtractedTextSegment>,
    pub mail_message: Option<MailMessage>,
    pub mail_attachments: Vec<MailAttachment>,
    pub archive_entries: Vec<ArchiveEntry>,
}

impl ExtractionBundle {
    /// Adapt this atomic extractor result to the database transaction input
    /// without re-normalizing or re-hashing any text.
    pub fn into_db_input(self) -> DfResult<df_db::extraction::ContentExtractionInput> {
        let mut by_subject: HashMap<TextSubjectId, Vec<df_db::extraction::TextSegmentInput>> =
            HashMap::new();
        for segment in self.text_segments {
            by_subject.entry(segment.subject_id).or_default().push(
                df_db::extraction::TextSegmentInput {
                    ordinal: segment.ordinal,
                    char_start: segment.char_start,
                    char_end: segment.char_end,
                    text: segment.text,
                    text_sha256: segment.sha256,
                },
            );
        }
        let subjects = self
            .text_subjects
            .into_iter()
            .map(|subject| df_db::extraction::TextSubjectInput {
                segments: by_subject.remove(&subject.id).unwrap_or_default(),
                subject,
            })
            .collect();
        if !by_subject.is_empty() {
            return Err(DfError::Validation(
                "text segment references a subject outside its bundle".to_string(),
            ));
        }
        Ok(df_db::extraction::ContentExtractionInput {
            representation: self.representation,
            source_sha256: self.source_sha256,
            subjects,
            mail_message: self.mail_message,
            mail_attachments: self.mail_attachments,
            archive_entries: self.archive_entries,
        })
    }
}

/// Bytes read through a hard `max_input_bytes + 1` boundary together with the
/// stable pre/post size observed on the open source handle.
#[derive(Debug)]
pub struct BoundedFileInput {
    pub bytes: Vec<u8>,
    pub source_size_bytes: u64,
}

/// Open a physical source without using a display path, cap the read before
/// allocation, and reject a concurrent size/mtime change. Callers should build
/// `path` from [`df_domain::RawPath`], never from a lossy display string.
pub fn read_bounded_file(path: &Path, limits: &ExtractionLimits) -> DfResult<BoundedFileInput> {
    limits.validate()?;
    let io_path = df_fs_safety::extended_for_io(path);
    let mut file = std::fs::File::open(&io_path).map_err(|error| DfError::io(path, error))?;
    let before = file.metadata().map_err(|error| DfError::io(path, error))?;
    if !before.is_file() {
        return Err(DfError::Validation(format!(
            "extraction source `{}` is not a regular file",
            path.display()
        )));
    }
    let source_size_bytes = before.len();
    let cap = limits
        .max_input_bytes
        .checked_add(1)
        .ok_or_else(|| DfError::Validation("input read cap overflow".to_string()))?;
    let capacity = usize::try_from(source_size_bytes.min(cap)).map_err(|_| {
        DfError::Validation("input read size does not fit this platform".to_string())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|error| DfError::io(path, error))?;
    let after = file.metadata().map_err(|error| DfError::io(path, error))?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || (source_size_bytes <= limits.max_input_bytes
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX) != source_size_bytes)
    {
        return Err(DfError::Conflict(format!(
            "extraction source `{}` changed while it was read",
            path.display()
        )));
    }
    Ok(BoundedFileInput {
        bytes,
        source_size_bytes,
    })
}

/// Extract one already bounded physical content object. Invalid configuration
/// or lineage is a caller error; malformed/untrusted content becomes a typed
/// representation outcome so a run can continue and audit it.
pub fn extract(
    request: ExtractionRequest<'_>,
    limits: &ExtractionLimits,
) -> DfResult<ExtractionBundle> {
    extract_impl(request, limits, None)
}

fn extract_impl(
    request: ExtractionRequest<'_>,
    limits: &ExtractionLimits,
    pdf_worker: Option<&PdfWorkerConfig>,
) -> DfResult<ExtractionBundle> {
    limits.validate()?;
    validate_request(&request, limits)?;
    let format = detect(request.display_name, request.mime_hint, request.bytes);
    let mime = canonical_mime(format, request.mime_hint, request.display_name);
    let mut builder = BundleBuilder::new(request, limits, format, mime);

    if request.source_size_bytes > limits.max_input_bytes {
        builder.status = ExtractionStatus::Limited;
        builder.error = Some("input exceeds the configured extraction byte limit".to_string());
        let mime = builder.mime.clone();
        let document = builder.add_subject(
            TextSubjectKind::Document,
            None,
            request.display_name,
            None,
            &mime,
            request.source_size_bytes,
            None,
            json!({"input_limit_reached": true}),
            false,
        );
        return Ok(builder.finish(Some(document)));
    }

    match format {
        DocumentFormat::Unsupported => {
            builder.status = ExtractionStatus::Unsupported;
        }
        DocumentFormat::Text | DocumentFormat::Html | DocumentFormat::Docx => {
            match extract_text_payload(
                format,
                request.display_name,
                request.mime_hint,
                request.bytes,
                limits,
            ) {
                Ok(payload) => {
                    builder.metadata = payload.metadata.clone();
                    let primary = builder.add_subject(
                        TextSubjectKind::Document,
                        None,
                        request.display_name,
                        None,
                        &payload.mime,
                        request.source_size_bytes,
                        Some(&payload.text),
                        payload.metadata,
                        false,
                    );
                    if primary.truncated {
                        builder.mark_limited("normalized text reached its configured limit");
                    }
                    return Ok(builder.finish(Some(primary)));
                }
                Err(error) => {
                    builder.status = ExtractionStatus::Failed;
                    builder.error = Some(bounded_error(&error));
                }
            }
        }
        DocumentFormat::Pdf => {
            let primary = extract_top_level_pdf(&mut builder, pdf_worker);
            return Ok(builder.finish(Some(primary)));
        }
        DocumentFormat::Eml => extract_eml(&mut builder, pdf_worker),
        DocumentFormat::Zip => extract_zip(&mut builder, pdf_worker),
    }
    Ok(builder.finish(None))
}

/// Extract content while allowing a top-level PDF to use an explicitly
/// configured, resource-isolated worker process.
///
/// Non-PDF formats follow [`extract`] exactly. PDF parser rejection is
/// persisted as `FAILED`; timeout, process/protocol failure, and output
/// overflow are persisted as `LIMITED` so a run can continue without parsing
/// untrusted PDF bytes in the main process.
pub fn extract_with_pdf_worker(
    request: ExtractionRequest<'_>,
    limits: &ExtractionLimits,
    worker: &PdfWorkerConfig,
) -> DfResult<ExtractionBundle> {
    extract_impl(request, limits, Some(worker))
}

fn extract_top_level_pdf(
    builder: &mut BundleBuilder<'_>,
    worker: Option<&PdfWorkerConfig>,
) -> SubjectEvidence {
    let Some(worker) = worker else {
        builder.mark_limited(
            "PDF text extraction requires a resource-isolated worker; in-process parsing is disabled",
        );
        builder.metadata = json!({
            "backend": "pdf-extract",
            "in_process": false,
            "isolation_required": true,
        });
        return builder.add_subject(
            TextSubjectKind::Document,
            None,
            builder.request.display_name,
            None,
            "application/pdf",
            builder.request.source_size_bytes,
            None,
            json!({"isolation_required": true}),
            false,
        );
    };

    let max_output_bytes = pdf_output_limit(builder.limits.max_text_chars);
    let outcome = pdf_worker::invoke(worker, builder.request.bytes, max_output_bytes);
    let (text, subject_metadata) = match outcome {
        PdfWorkerOutcome::Text(text) => {
            let output_bytes = text.len();
            builder.metadata = json!({
                "backend": "pdf-extract",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "OK",
                "worker_output_bytes": output_bytes,
            });
            (
                Some(text),
                json!({
                    "isolated_worker": true,
                    "worker_output_bytes": output_bytes,
                }),
            )
        }
        PdfWorkerOutcome::Rejected(error) => {
            builder.status = ExtractionStatus::Failed;
            builder.error = Some(bounded_error(&error));
            builder.metadata = json!({
                "backend": "pdf-extract",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "REJECTED",
            });
            (
                None,
                json!({"isolated_worker": true, "worker_status": "REJECTED"}),
            )
        }
        PdfWorkerOutcome::Internal(error) => {
            builder.status = ExtractionStatus::Failed;
            builder.error = Some(bounded_error(&error));
            builder.metadata = json!({
                "backend": "pdf-extract",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "INTERNAL_ERROR",
            });
            (
                None,
                json!({"isolated_worker": true, "worker_status": "INTERNAL_ERROR"}),
            )
        }
        PdfWorkerOutcome::OutputLimit => {
            builder.mark_limited("PDF worker output exceeded the configured text byte limit");
            builder.metadata = json!({
                "backend": "pdf-extract",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "OUTPUT_LIMIT",
                "worker_output_limit_bytes": max_output_bytes,
            });
            (
                None,
                json!({"isolated_worker": true, "worker_status": "OUTPUT_LIMIT"}),
            )
        }
        PdfWorkerOutcome::Limited(error) => {
            builder.mark_limited(&error);
            builder.metadata = json!({
                "backend": "pdf-extract",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "UNAVAILABLE",
            });
            (
                None,
                json!({"isolated_worker": true, "worker_status": "UNAVAILABLE"}),
            )
        }
    };
    let primary = builder.add_subject(
        TextSubjectKind::Document,
        None,
        builder.request.display_name,
        None,
        "application/pdf",
        builder.request.source_size_bytes,
        text.as_deref(),
        subject_metadata,
        false,
    );
    if primary.truncated {
        builder.mark_limited("normalized PDF text reached its configured limit");
    }
    primary
}

#[derive(Debug)]
struct EmbeddedPdfResult {
    text: Option<String>,
    metadata: Value,
    status: ExtractionStatus,
    warning: Option<String>,
}

fn extract_embedded_pdf(
    worker: Option<&PdfWorkerConfig>,
    bytes: &[u8],
    available_text_chars: u64,
) -> EmbeddedPdfResult {
    let Some(worker) = worker else {
        return EmbeddedPdfResult {
            text: None,
            metadata: json!({
                "format": "PDF",
                "in_process": false,
                "isolation_required": true,
            }),
            status: ExtractionStatus::Limited,
            warning: Some("PDF content requires a resource-isolated worker".to_string()),
        };
    };
    if available_text_chars == 0 {
        return EmbeddedPdfResult {
            text: None,
            metadata: json!({
                "format": "PDF",
                "in_process": false,
                "worker_status": "TEXT_BUDGET_EXHAUSTED",
            }),
            status: ExtractionStatus::Limited,
            warning: Some("global text budget was exhausted before PDF extraction".to_string()),
        };
    }

    let max_output_bytes = pdf_output_limit(available_text_chars);
    match pdf_worker::invoke(worker, bytes, max_output_bytes) {
        PdfWorkerOutcome::Text(text) => {
            let output_bytes = text.len();
            let (text, truncated) = truncate_to_chars(text, available_text_chars);
            EmbeddedPdfResult {
                text: Some(text),
                metadata: json!({
                    "format": "PDF",
                    "in_process": false,
                    "isolated": true,
                    "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                    "worker_status": "OK",
                    "worker_output_bytes": output_bytes,
                    "text_budget_truncated": truncated,
                }),
                status: if truncated {
                    ExtractionStatus::Limited
                } else {
                    ExtractionStatus::Extracted
                },
                warning: truncated
                    .then(|| "PDF text exceeded the remaining global text budget".to_string()),
            }
        }
        PdfWorkerOutcome::Rejected(error) => EmbeddedPdfResult {
            text: None,
            metadata: json!({
                "format": "PDF",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "REJECTED",
                "worker_error": bounded_error(&error),
            }),
            status: ExtractionStatus::Limited,
            warning: Some(format!(
                "PDF worker rejected content: {}",
                bounded_error(&error)
            )),
        },
        PdfWorkerOutcome::OutputLimit => EmbeddedPdfResult {
            text: None,
            metadata: json!({
                "format": "PDF",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "OUTPUT_LIMIT",
                "worker_output_limit_bytes": max_output_bytes,
            }),
            status: ExtractionStatus::Limited,
            warning: Some("PDF worker output exceeded the remaining text budget".to_string()),
        },
        PdfWorkerOutcome::Internal(error) | PdfWorkerOutcome::Limited(error) => EmbeddedPdfResult {
            text: None,
            metadata: json!({
                "format": "PDF",
                "in_process": false,
                "isolated": true,
                "worker_protocol": worker_protocol::PROTOCOL_VERSION,
                "worker_status": "UNAVAILABLE",
                "worker_error": bounded_error(&error),
            }),
            status: ExtractionStatus::Limited,
            warning: Some(format!("PDF worker failed: {}", bounded_error(&error))),
        },
    }
}

fn pdf_output_limit(text_chars: u64) -> u64 {
    text_chars
        .checked_mul(4)
        .unwrap_or(worker_protocol::HARD_MAX_OUTPUT_BYTES)
        .min(worker_protocol::HARD_MAX_OUTPUT_BYTES)
}

fn truncate_to_chars(mut text: String, maximum: u64) -> (String, bool) {
    let maximum = usize::try_from(maximum).unwrap_or(usize::MAX);
    let boundary = text.char_indices().nth(maximum).map(|(index, _)| index);
    if let Some(boundary) = boundary {
        text.truncate(boundary);
        (text, true)
    } else {
        (text, false)
    }
}

fn extract_eml(builder: &mut BundleBuilder<'_>, pdf_worker: Option<&PdfWorkerConfig>) {
    let parsed = match parse_mail(builder.request.bytes, builder.limits) {
        Ok(parsed) => parsed,
        Err(error) => {
            builder.status = ExtractionStatus::Failed;
            builder.error = Some(bounded_error(&error));
            return;
        }
    };
    builder.title = parsed.subject.clone();
    builder.metadata = parsed.metadata.clone();
    if parsed.limited {
        builder.mark_limited("mail metadata or attachment limits were reached");
    }
    let primary = builder.add_subject(
        TextSubjectKind::Document,
        None,
        builder.request.display_name,
        None,
        "message/rfc822",
        builder.request.source_size_bytes,
        Some(&parsed.body),
        json!({"mail_body": true}),
        false,
    );
    if primary.truncated {
        builder.mark_limited("mail body text reached its configured limit");
    }
    builder.mail_message = Some(MailMessage {
        representation_id: builder.request.representation_id,
        message_id: parsed.message_id,
        in_reply_to: parsed.in_reply_to,
        references: parsed.references,
        from: parsed.from,
        to: parsed.to,
        cc: parsed.cc,
        sent_at: parsed.sent_at,
        subject: parsed.subject,
        normalized_subject: parsed.normalized_subject,
        body_sha256: primary.sha256.clone(),
    });

    for attachment in parsed.attachments {
        let virtual_path = attachment_virtual_path(attachment.ordinal, &attachment.file_name);
        let (raw_text, text_metadata, mut status) = match attachment.extractable_bytes.as_deref() {
            None => (
                None,
                json!({"limit_reached": true}),
                ExtractionStatus::Limited,
            ),
            Some(bytes) => {
                let format = detect(&attachment.file_name, Some(&attachment.mime), bytes);
                match format {
                    DocumentFormat::Text | DocumentFormat::Html | DocumentFormat::Docx => {
                        match builder.extract_embedded_payload(
                            format,
                            &attachment.file_name,
                            Some(&attachment.mime),
                            bytes,
                        ) {
                            Ok(payload) => (
                                Some(payload.text),
                                payload.metadata,
                                ExtractionStatus::Extracted,
                            ),
                            Err(error) => {
                                builder.mark_limited(&format!(
                                    "attachment extraction failed: {}",
                                    bounded_error(&error)
                                ));
                                (
                                    None,
                                    json!({"format": format.as_str()}),
                                    ExtractionStatus::Failed,
                                )
                            }
                        }
                    }
                    DocumentFormat::Pdf => {
                        let available = builder
                            .limits
                            .max_total_text_chars
                            .saturating_sub(builder.total_text_chars)
                            .min(builder.limits.max_text_chars);
                        let result = extract_embedded_pdf(pdf_worker, bytes, available);
                        if let Some(warning) = &result.warning {
                            builder.mark_limited(&format!(
                                "PDF attachment `{}`: {warning}",
                                bounded_metadata(
                                    &attachment.file_name,
                                    builder.limits.max_metadata_chars
                                )
                            ));
                        }
                        (result.text, result.metadata, result.status)
                    }
                    _ => (
                        None,
                        json!({"format": format.as_str()}),
                        ExtractionStatus::Unsupported,
                    ),
                }
            }
        };
        let subject = builder.add_subject(
            TextSubjectKind::MailAttachment,
            Some(primary.id),
            &attachment.file_name,
            Some(&virtual_path),
            &attachment.mime,
            attachment.size_bytes,
            raw_text.as_deref(),
            text_metadata,
            status == ExtractionStatus::Limited,
        );
        if subject.truncated && status == ExtractionStatus::Extracted {
            status = ExtractionStatus::Limited;
            builder.mark_limited("attachment text reached its configured limit");
        }
        if status == ExtractionStatus::Limited {
            builder.mark_limited("attachment content exceeded an extraction limit");
        }
        builder.mail_attachments.push(MailAttachment {
            id: MailAttachmentId::new(),
            representation_id: builder.request.representation_id,
            subject_id: subject.id,
            ordinal: attachment.ordinal,
            file_name: attachment.file_name,
            mime: attachment.mime,
            size_bytes: attachment.size_bytes,
            sha256: attachment.sha256,
            extraction_status: status,
            created_at: builder.request.created_at,
        });
    }
    builder.primary = Some(primary);
}

fn extract_zip(builder: &mut BundleBuilder<'_>, pdf_worker: Option<&PdfWorkerConfig>) {
    let document = builder.add_subject(
        TextSubjectKind::Document,
        None,
        builder.request.display_name,
        None,
        "application/zip",
        builder.request.source_size_bytes,
        None,
        json!({"archive_container": true}),
        false,
    );
    builder.primary = Some(document.clone());
    let scan = scan_archive(builder.request.bytes, builder.limits, pdf_worker);
    builder.metadata = json!({
        "archive_entries": scan.entries.len(),
        "expanded_bytes": scan.expanded_bytes,
        "warnings": scan.warnings,
    });
    if scan.limited {
        builder.mark_limited("archive validation or extraction limits were reached");
    }
    for mut entry in scan.entries {
        let subject = match entry.payload.take() {
            Some(payload) => {
                let subject = builder.add_subject(
                    TextSubjectKind::ArchiveEntry,
                    Some(document.id),
                    &entry.virtual_path,
                    Some(&entry.virtual_path),
                    &payload.mime,
                    entry.size_bytes,
                    Some(&payload.text),
                    merge_json(entry.metadata.clone(), payload.metadata),
                    false,
                );
                if subject.truncated {
                    entry.status = ExtractionStatus::Limited;
                    builder.mark_limited("archive entry text reached its configured limit");
                }
                Some(subject)
            }
            None => entry.subject_mime.as_deref().map(|mime| {
                builder.add_subject(
                    TextSubjectKind::ArchiveEntry,
                    Some(document.id),
                    &entry.virtual_path,
                    Some(&entry.virtual_path),
                    mime,
                    entry.size_bytes,
                    None,
                    entry.metadata.clone(),
                    entry.status == ExtractionStatus::Limited,
                )
            }),
        };
        builder.archive_entries.push(ArchiveEntry {
            id: ArchiveEntryId::new(),
            representation_id: builder.request.representation_id,
            subject_id: subject.map(|subject| subject.id),
            ordinal: entry.ordinal,
            virtual_path: entry.virtual_path,
            compressed_bytes: entry.compressed_bytes,
            size_bytes: entry.size_bytes,
            crc32: entry.crc32,
            encrypted: entry.encrypted,
            directory: entry.directory,
            sha256: entry.sha256,
            extraction_status: entry.status,
            created_at: builder.request.created_at,
        });
    }
}

#[derive(Debug, Clone)]
struct SubjectEvidence {
    id: TextSubjectId,
    sha256: Option<String>,
    chars: u64,
    truncated: bool,
}

struct BundleBuilder<'a> {
    request: ExtractionRequest<'a>,
    limits: &'a ExtractionLimits,
    format: DocumentFormat,
    mime: String,
    status: ExtractionStatus,
    title: Option<String>,
    metadata: Value,
    error: Option<String>,
    text_subjects: Vec<TextSubject>,
    text_segments: Vec<ExtractedTextSegment>,
    mail_message: Option<MailMessage>,
    mail_attachments: Vec<MailAttachment>,
    archive_entries: Vec<ArchiveEntry>,
    total_text_chars: u64,
    container_expanded_bytes: u64,
    container_entries: u64,
    primary: Option<SubjectEvidence>,
}

impl<'a> BundleBuilder<'a> {
    fn new(
        request: ExtractionRequest<'a>,
        limits: &'a ExtractionLimits,
        format: DocumentFormat,
        mime: String,
    ) -> Self {
        Self {
            request,
            limits,
            format,
            mime,
            status: ExtractionStatus::Extracted,
            title: None,
            metadata: json!({}),
            error: None,
            text_subjects: Vec::new(),
            text_segments: Vec::new(),
            mail_message: None,
            mail_attachments: Vec::new(),
            archive_entries: Vec::new(),
            total_text_chars: 0,
            container_expanded_bytes: 0,
            container_entries: 0,
            primary: None,
        }
    }

    fn extract_embedded_payload(
        &mut self,
        format: DocumentFormat,
        display_name: &str,
        mime_hint: Option<&str>,
        bytes: &[u8],
    ) -> Result<formats::ExtractedPayload, String> {
        let mut constrained = self.limits.clone();
        if format == DocumentFormat::Docx {
            constrained.max_archive_entries = self
                .limits
                .max_archive_entries
                .saturating_sub(self.container_entries);
            constrained.max_archive_total_bytes = self
                .limits
                .max_archive_total_bytes
                .saturating_sub(self.container_expanded_bytes);
            constrained.max_archive_entry_bytes = constrained
                .max_archive_entry_bytes
                .min(constrained.max_archive_total_bytes);
            if constrained.max_archive_entries == 0 || constrained.max_archive_total_bytes == 0 {
                return Err("embedded DOCX exhausted the container budget".to_string());
            }
        }
        let payload = extract_text_payload(format, display_name, mime_hint, bytes, &constrained)?;
        self.container_expanded_bytes = self
            .container_expanded_bytes
            .checked_add(payload.expanded_bytes)
            .ok_or_else(|| "embedded container expansion counter overflow".to_string())?;
        self.container_entries = self
            .container_entries
            .checked_add(payload.container_entries)
            .ok_or_else(|| "embedded container entry counter overflow".to_string())?;
        if self.container_expanded_bytes > self.limits.max_archive_total_bytes
            || self.container_entries > self.limits.max_archive_entries
        {
            return Err("embedded container budget exceeded after extraction".to_string());
        }
        Ok(payload)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_subject(
        &mut self,
        kind: TextSubjectKind,
        parent_subject_id: Option<TextSubjectId>,
        display_name: &str,
        virtual_path: Option<&str>,
        mime: &str,
        size_bytes: u64,
        raw_text: Option<&str>,
        metadata: Value,
        forced_truncated: bool,
    ) -> SubjectEvidence {
        let id = TextSubjectId::new();
        let remaining = self
            .limits
            .max_total_text_chars
            .saturating_sub(self.total_text_chars);
        let normalized = raw_text.and_then(|text| {
            if remaining == 0 {
                None
            } else {
                Some(normalize_text(
                    text,
                    self.limits.max_text_chars.min(remaining),
                ))
            }
        });
        let budget_blocked = raw_text.is_some() && normalized.is_none();
        let truncated = forced_truncated
            || budget_blocked
            || normalized.as_ref().is_some_and(|text| text.truncated);
        let (sha256, chars) = normalized
            .as_ref()
            .map_or((None, 0), |text| (Some(text.sha256.clone()), text.chars));
        if let Some(text) = &normalized {
            for segment in segment_text(&text.text, self.limits.text_segment_chars) {
                self.text_segments.push(ExtractedTextSegment {
                    subject_id: id,
                    ordinal: segment.ordinal,
                    char_start: segment.char_start,
                    char_end: segment.char_end,
                    text: segment.text,
                    sha256: segment.sha256,
                });
            }
        }
        self.total_text_chars = self.total_text_chars.saturating_add(chars);
        self.text_subjects.push(TextSubject {
            id,
            representation_id: self.request.representation_id,
            kind,
            parent_subject_id,
            display_name: bounded_metadata(display_name, self.limits.max_metadata_chars),
            virtual_path: virtual_path
                .map(|path| bounded_metadata(path, self.limits.max_virtual_path_chars)),
            mime: bounded_metadata(mime, self.limits.max_metadata_chars),
            size_bytes,
            normalized_text_sha256: sha256.clone(),
            normalized_chars: chars,
            text_truncated: truncated,
            metadata,
            created_at: self.request.created_at,
        });
        SubjectEvidence {
            id,
            sha256,
            chars,
            truncated,
        }
    }

    fn mark_limited(&mut self, error: &str) {
        if self.status != ExtractionStatus::Failed {
            self.status = ExtractionStatus::Limited;
        }
        if self.error.is_none() {
            self.error = Some(bounded_error(error));
        }
    }

    fn finish(mut self, primary: Option<SubjectEvidence>) -> ExtractionBundle {
        let primary = primary.or_else(|| self.primary.take());
        let representation = DocumentRepresentation {
            id: self.request.representation_id,
            content_id: self.request.content_id,
            extractor_version: self.request.extractor_version.to_string(),
            config_digest: self.request.config_digest.to_string(),
            format: self.format,
            mime: self.mime,
            status: self.status,
            title: self.title,
            normalized_text_sha256: primary.as_ref().and_then(|item| item.sha256.clone()),
            normalized_chars: primary.as_ref().map_or(0, |item| item.chars),
            text_truncated: primary.as_ref().is_some_and(|item| item.truncated),
            metadata: merge_json(
                json!({
                    "input_bytes": self.request.source_size_bytes,
                    "read_bytes": self.request.bytes.len(),
                    "input_sha256": self.request.source_sha256,
                    "total_normalized_chars": self.total_text_chars,
                    "embedded_container_expanded_bytes": self.container_expanded_bytes,
                    "embedded_container_entries": self.container_entries,
                }),
                self.metadata,
            ),
            error: self.error,
            created_at: self.request.created_at,
        };
        ExtractionBundle {
            source_sha256: self.request.source_sha256.to_string(),
            representation,
            text_subjects: self.text_subjects,
            text_segments: self.text_segments,
            mail_message: self.mail_message,
            mail_attachments: self.mail_attachments,
            archive_entries: self.archive_entries,
        }
    }
}

fn validate_request(request: &ExtractionRequest<'_>, limits: &ExtractionLimits) -> DfResult<()> {
    if request.display_name.is_empty() {
        return Err(DfError::Validation(
            "extraction display name cannot be empty".to_string(),
        ));
    }
    if request.source_size_bytes > i64::MAX as u64 {
        return Err(DfError::Validation(
            "extraction source size exceeds SQLite INTEGER bounds".to_string(),
        ));
    }
    if request.source_sha256.len() != 64
        || !request
            .source_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DfError::Validation(
            "source SHA-256 must be 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    let read_bytes = u64::try_from(request.bytes.len()).unwrap_or(u64::MAX);
    if request.source_size_bytes <= limits.max_input_bytes {
        if read_bytes != request.source_size_bytes {
            return Err(DfError::Validation(
                "bounded input length does not match the declared source size".to_string(),
            ));
        }
        if sha256_hex(request.bytes) != request.source_sha256 {
            return Err(DfError::Conflict(
                "source bytes do not match the canonical content SHA-256".to_string(),
            ));
        }
    } else if read_bytes != limits.max_input_bytes.saturating_add(1) {
        return Err(DfError::Validation(
            "oversized input must contain exactly max_input_bytes + 1 prefix bytes".to_string(),
        ));
    }
    if u64::try_from(request.display_name.chars().count()).unwrap_or(u64::MAX)
        > limits.max_metadata_chars
    {
        return Err(DfError::Validation(
            "extraction display name exceeds the metadata limit".to_string(),
        ));
    }
    if request.extractor_version != EXTRACTOR_VERSION {
        return Err(DfError::Validation(
            "request extractor version does not match this backend".to_string(),
        ));
    }
    if request.config_digest.len() != 64
        || !request
            .config_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DfError::Validation(
            "extraction config digest must be 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    if request.config_digest != limits.digest()? {
        return Err(DfError::Validation(
            "request config digest does not match the supplied extraction limits".to_string(),
        ));
    }
    Ok(())
}

fn attachment_virtual_path(ordinal: u32, name: &str) -> String {
    let component = safe_virtual_component(name);
    format!("attachments/{ordinal:06}/{component}")
}

fn safe_virtual_component(value: &str) -> String {
    let mut output = String::new();
    let mut previous_replacement = false;
    for character in value.nfc() {
        let invalid = character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\'
            );
        if invalid {
            if !previous_replacement {
                output.push('_');
            }
            previous_replacement = true;
        } else {
            output.push(character);
            previous_replacement = false;
        }
    }
    while output.ends_with([' ', '.']) {
        output.pop();
    }
    if output.is_empty() {
        "attachment".to_string()
    } else if df_fs_safety::SafeRelativePath::parse(Path::new(&output)).is_err() {
        format!("attachment-{}", &sha256_hex(value.as_bytes())[..12])
    } else {
        output
    }
}

fn bounded_metadata(value: &str, maximum: u64) -> String {
    value
        .nfc()
        .filter(|character| !character.is_control())
        .take(usize::try_from(maximum).expect("validated"))
        .collect()
}

fn bounded_error(value: &str) -> String {
    value
        .nfc()
        .filter(|character| !character.is_control())
        .take(512)
        .collect()
}

fn merge_json(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            for (key, value) in extra {
                base.insert(key, value);
            }
            Value::Object(base)
        }
        (base, _) => base,
    }
}

#[cfg(test)]
mod tests;
