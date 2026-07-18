use df_domain::{DocumentFormat, ExtractionStatus};
use serde_json::{json, Value};

use crate::formats::{
    detect_format, extract_text_payload, is_zip_magic, mime_from_extension, ExtractedPayload,
};
use crate::normalize::sha256_hex;
use crate::zip_safety::ValidatedZip;
use crate::{extract_embedded_pdf, ExtractionLimits, PdfWorkerConfig};

#[derive(Debug)]
pub(crate) struct ArchiveScan {
    pub entries: Vec<ArchiveItemDraft>,
    pub expanded_bytes: u64,
    pub limited: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct ArchiveItemDraft {
    pub ordinal: u32,
    pub virtual_path: String,
    pub compressed_bytes: u64,
    pub size_bytes: u64,
    pub crc32: u32,
    pub encrypted: bool,
    pub directory: bool,
    pub sha256: Option<String>,
    pub status: ExtractionStatus,
    pub payload: Option<ExtractedPayload>,
    /// Creates an evidence subject even when extraction produced no text.
    pub subject_mime: Option<String>,
    pub metadata: Value,
}

pub(crate) fn scan_archive(
    bytes: &[u8],
    limits: &ExtractionLimits,
    pdf_worker: Option<&PdfWorkerConfig>,
) -> ArchiveScan {
    let mut state = ArchiveState {
        limits,
        pdf_worker,
        entries: Vec::new(),
        expanded_bytes: 0,
        container_entries: 0,
        extracted_text_chars: 0,
        limited: false,
        warnings: Vec::new(),
    };
    if let Err(error) = state.walk(bytes, None, 1) {
        state.limit(error);
    }
    ArchiveScan {
        entries: state.entries,
        expanded_bytes: state.expanded_bytes,
        limited: state.limited,
        warnings: state.warnings,
    }
}

struct ArchiveState<'a> {
    limits: &'a ExtractionLimits,
    pdf_worker: Option<&'a PdfWorkerConfig>,
    entries: Vec<ArchiveItemDraft>,
    expanded_bytes: u64,
    container_entries: u64,
    extracted_text_chars: u64,
    limited: bool,
    warnings: Vec<String>,
}

impl ArchiveState<'_> {
    fn walk(
        &mut self,
        bytes: &[u8],
        prefix: Option<&str>,
        nesting_depth: u64,
    ) -> Result<(), String> {
        if nesting_depth > self.limits.max_archive_nesting_depth {
            return Err("nested ZIP depth exceeds the configured limit".to_string());
        }
        let remaining_entries = self
            .limits
            .max_archive_entries
            .saturating_sub(self.container_entries);
        let remaining_bytes = self
            .limits
            .max_archive_total_bytes
            .saturating_sub(self.expanded_bytes);
        let mut archive =
            ValidatedZip::open(bytes, self.limits, remaining_entries, remaining_bytes)?;
        self.container_entries = self
            .container_entries
            .checked_add(u64::try_from(archive.entries.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| "archive entry counter overflow".to_string())?;
        let headers = archive.entries.clone();
        for header in headers {
            let local_name = header.name.trim_end_matches('/');
            let virtual_path = match prefix {
                Some(prefix) => format!("{prefix}!/{local_name}"),
                None => local_name.to_string(),
            };
            if u64::try_from(virtual_path.chars().count()).unwrap_or(u64::MAX)
                > self.limits.max_virtual_path_chars
            {
                return Err("nested ZIP virtual path exceeds the configured limit".to_string());
            }
            let ordinal = u32::try_from(self.entries.len())
                .map_err(|_| "archive entry ordinal overflow".to_string())?;
            if header.directory {
                self.entries.push(ArchiveItemDraft {
                    ordinal,
                    virtual_path,
                    compressed_bytes: header.compressed_bytes,
                    size_bytes: header.size_bytes,
                    crc32: header.crc32,
                    encrypted: header.encrypted,
                    directory: true,
                    sha256: None,
                    status: ExtractionStatus::Extracted,
                    payload: None,
                    subject_mime: None,
                    metadata: json!({"nesting_depth": nesting_depth}),
                });
                continue;
            }
            if header.encrypted {
                self.entries.push(ArchiveItemDraft {
                    ordinal,
                    virtual_path,
                    compressed_bytes: header.compressed_bytes,
                    size_bytes: header.size_bytes,
                    crc32: header.crc32,
                    encrypted: true,
                    directory: false,
                    sha256: None,
                    status: ExtractionStatus::Unsupported,
                    payload: None,
                    subject_mime: None,
                    metadata: json!({"nesting_depth": nesting_depth}),
                });
                continue;
            }

            let data = match archive.read_entry(&header) {
                Ok(data) => data,
                Err(error) => {
                    self.entries.push(ArchiveItemDraft {
                        ordinal,
                        virtual_path,
                        compressed_bytes: header.compressed_bytes,
                        size_bytes: header.size_bytes,
                        crc32: header.crc32,
                        encrypted: false,
                        directory: false,
                        sha256: None,
                        status: ExtractionStatus::Failed,
                        payload: None,
                        subject_mime: None,
                        metadata: json!({"nesting_depth": nesting_depth}),
                    });
                    self.limit(error);
                    continue;
                }
            };
            self.expanded_bytes = self
                .expanded_bytes
                .checked_add(u64::try_from(data.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| "archive expansion counter overflow".to_string())?;
            if self.expanded_bytes > self.limits.max_archive_total_bytes {
                return Err("archive actual expansion exceeded the configured limit".to_string());
            }
            let digest = sha256_hex(&data);
            let mime = mime_from_extension(&virtual_path);
            let format = detect_format(&virtual_path, Some(&mime), &data);

            if format == DocumentFormat::Zip && is_zip_magic(&data) {
                let item_index = self.entries.len();
                self.entries.push(ArchiveItemDraft {
                    ordinal,
                    virtual_path: virtual_path.clone(),
                    compressed_bytes: header.compressed_bytes,
                    size_bytes: header.size_bytes,
                    crc32: header.crc32,
                    encrypted: false,
                    directory: false,
                    sha256: Some(digest),
                    status: ExtractionStatus::Extracted,
                    payload: None,
                    subject_mime: None,
                    metadata: json!({"nesting_depth": nesting_depth, "nested_archive": true}),
                });
                if nesting_depth == self.limits.max_archive_nesting_depth {
                    self.entries[item_index].status = ExtractionStatus::Limited;
                    self.limit("nested ZIP depth reached the configured limit".to_string());
                } else if let Err(error) =
                    self.walk(&data, Some(&virtual_path), nesting_depth.saturating_add(1))
                {
                    self.entries[item_index].status = ExtractionStatus::Limited;
                    self.limit(error);
                }
                continue;
            }

            let mut embedded_status = None;
            let mut embedded_metadata = json!({});
            let payload = match format {
                DocumentFormat::Text | DocumentFormat::Html | DocumentFormat::Docx => {
                    let mut payload_limits = self.limits.clone();
                    payload_limits.max_archive_entries = self
                        .limits
                        .max_archive_entries
                        .saturating_sub(self.container_entries);
                    payload_limits.max_archive_total_bytes = self
                        .limits
                        .max_archive_total_bytes
                        .saturating_sub(self.expanded_bytes);
                    payload_limits.max_archive_entry_bytes = payload_limits
                        .max_archive_entry_bytes
                        .min(payload_limits.max_archive_total_bytes);
                    if format == DocumentFormat::Docx
                        && (payload_limits.max_archive_entries == 0
                            || payload_limits.max_archive_total_bytes == 0)
                    {
                        self.limit("embedded DOCX exhausted the container budget".to_string());
                        None
                    } else {
                        match extract_text_payload(
                            format,
                            &virtual_path,
                            Some(&mime),
                            &data,
                            &payload_limits,
                        ) {
                            Ok(payload) => {
                                self.expanded_bytes = self
                                    .expanded_bytes
                                    .checked_add(payload.expanded_bytes)
                                    .ok_or_else(|| {
                                        "archive expansion counter overflow".to_string()
                                    })?;
                                self.container_entries = self
                                    .container_entries
                                    .checked_add(payload.container_entries)
                                    .ok_or_else(|| "archive entry counter overflow".to_string())?;
                                self.charge_text(&payload.text);
                                Some(payload)
                            }
                            Err(error) => {
                                self.limit(error);
                                None
                            }
                        }
                    }
                }
                DocumentFormat::Pdf => {
                    let available = self
                        .limits
                        .max_total_text_chars
                        .saturating_sub(self.extracted_text_chars)
                        .min(self.limits.max_text_chars);
                    let result = extract_embedded_pdf(self.pdf_worker, &data, available);
                    if let Some(warning) = result.warning {
                        self.limit(format!("PDF archive entry `{virtual_path}`: {warning}"));
                    }
                    embedded_status = Some(result.status);
                    embedded_metadata = result.metadata.clone();
                    result.text.map(|text| {
                        self.charge_text(&text);
                        ExtractedPayload {
                            text,
                            mime: "application/pdf".to_string(),
                            metadata: result.metadata,
                            expanded_bytes: 0,
                            container_entries: 0,
                        }
                    })
                }
                _ => None,
            };
            let status = embedded_status.unwrap_or_else(|| match (format, payload.is_some()) {
                (DocumentFormat::Text | DocumentFormat::Html | DocumentFormat::Docx, false) => {
                    ExtractionStatus::Failed
                }
                (DocumentFormat::Unsupported | DocumentFormat::Eml, _) => {
                    ExtractionStatus::Unsupported
                }
                _ => ExtractionStatus::Extracted,
            });
            self.entries.push(ArchiveItemDraft {
                ordinal,
                virtual_path,
                compressed_bytes: header.compressed_bytes,
                size_bytes: header.size_bytes,
                crc32: header.crc32,
                encrypted: false,
                directory: false,
                sha256: Some(digest),
                status,
                payload,
                subject_mime: (format == DocumentFormat::Pdf).then_some(mime),
                metadata: merge_metadata(
                    json!({"nesting_depth": nesting_depth, "format": format.as_str()}),
                    embedded_metadata,
                ),
            });
        }
        Ok(())
    }

    fn limit(&mut self, warning: String) {
        self.limited = true;
        // A malformed archive can contain thousands of distinct failures. A
        // fixed warning cap prevents error metadata becoming another bomb.
        if self.warnings.len() < 16 {
            self.warnings.push(warning.chars().take(256).collect());
        }
    }

    fn charge_text(&mut self, text: &str) {
        let chars = u64::try_from(text.chars().count())
            .unwrap_or(u64::MAX)
            .min(self.limits.max_text_chars);
        self.extracted_text_chars = self
            .extracted_text_chars
            .saturating_add(chars)
            .min(self.limits.max_total_text_chars);
    }
}

fn merge_metadata(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            base.extend(extra);
            Value::Object(base)
        }
        (base, _) => base,
    }
}
