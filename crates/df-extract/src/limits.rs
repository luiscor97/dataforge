use df_error::{DfError, DfResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Every allocation-amplifying operation performed by the extractor has an
/// explicit, persisted limit. The values are deliberately conservative: a
/// caller may lower them for an untrusted corpus, but cannot disable them with
/// zero or an effectively unbounded sentinel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractionLimits {
    pub max_input_bytes: u64,
    pub max_text_chars: u64,
    pub max_total_text_chars: u64,
    pub text_segment_chars: u64,
    pub max_metadata_chars: u64,
    pub max_header_values: u64,
    pub max_mail_attachments: u64,
    pub max_attachment_bytes: u64,
    pub max_total_attachment_bytes: u64,
    pub max_archive_entries: u64,
    pub max_archive_entry_bytes: u64,
    pub max_archive_total_bytes: u64,
    pub max_archive_compression_ratio: u64,
    pub max_archive_nesting_depth: u64,
    pub max_archive_path_depth: u64,
    pub max_virtual_path_chars: u64,
    pub html_render_width: u64,
}

impl ExtractionLimits {
    /// Non-overridable safety ceilings. Configuration may only make extraction
    /// stricter than these values; it cannot enlarge the resource envelope.
    pub const HARD_MAXIMUMS: Self = Self {
        max_input_bytes: 64 * 1024 * 1024,
        max_text_chars: 4_000_000,
        max_total_text_chars: 8_000_000,
        text_segment_chars: 64_000,
        max_metadata_chars: 4_096,
        max_header_values: 256,
        max_mail_attachments: 256,
        max_attachment_bytes: 16 * 1024 * 1024,
        max_total_attachment_bytes: 64 * 1024 * 1024,
        max_archive_entries: 10_000,
        max_archive_entry_bytes: 16 * 1024 * 1024,
        max_archive_total_bytes: 128 * 1024 * 1024,
        max_archive_compression_ratio: 100,
        max_archive_nesting_depth: 4,
        max_archive_path_depth: 32,
        max_virtual_path_chars: 4_096,
        html_render_width: 120,
    };
}

impl Default for ExtractionLimits {
    fn default() -> Self {
        Self::HARD_MAXIMUMS
    }
}

impl ExtractionLimits {
    /// Reject configurations which would turn a bound off, overflow a domain
    /// ordinal, or fail to fit APIs taking `usize` on the current host.
    pub fn validate(&self) -> DfResult<()> {
        let bounded = [
            (
                "max_input_bytes",
                self.max_input_bytes,
                Self::HARD_MAXIMUMS.max_input_bytes,
            ),
            (
                "max_text_chars",
                self.max_text_chars,
                Self::HARD_MAXIMUMS.max_text_chars,
            ),
            (
                "max_total_text_chars",
                self.max_total_text_chars,
                Self::HARD_MAXIMUMS.max_total_text_chars,
            ),
            (
                "text_segment_chars",
                self.text_segment_chars,
                Self::HARD_MAXIMUMS.text_segment_chars,
            ),
            (
                "max_metadata_chars",
                self.max_metadata_chars,
                Self::HARD_MAXIMUMS.max_metadata_chars,
            ),
            (
                "max_header_values",
                self.max_header_values,
                Self::HARD_MAXIMUMS.max_header_values,
            ),
            (
                "max_mail_attachments",
                self.max_mail_attachments,
                Self::HARD_MAXIMUMS.max_mail_attachments,
            ),
            (
                "max_attachment_bytes",
                self.max_attachment_bytes,
                Self::HARD_MAXIMUMS.max_attachment_bytes,
            ),
            (
                "max_total_attachment_bytes",
                self.max_total_attachment_bytes,
                Self::HARD_MAXIMUMS.max_total_attachment_bytes,
            ),
            (
                "max_archive_entries",
                self.max_archive_entries,
                Self::HARD_MAXIMUMS.max_archive_entries,
            ),
            (
                "max_archive_entry_bytes",
                self.max_archive_entry_bytes,
                Self::HARD_MAXIMUMS.max_archive_entry_bytes,
            ),
            (
                "max_archive_total_bytes",
                self.max_archive_total_bytes,
                Self::HARD_MAXIMUMS.max_archive_total_bytes,
            ),
            (
                "max_archive_compression_ratio",
                self.max_archive_compression_ratio,
                Self::HARD_MAXIMUMS.max_archive_compression_ratio,
            ),
            (
                "max_archive_nesting_depth",
                self.max_archive_nesting_depth,
                Self::HARD_MAXIMUMS.max_archive_nesting_depth,
            ),
            (
                "max_archive_path_depth",
                self.max_archive_path_depth,
                Self::HARD_MAXIMUMS.max_archive_path_depth,
            ),
            (
                "max_virtual_path_chars",
                self.max_virtual_path_chars,
                Self::HARD_MAXIMUMS.max_virtual_path_chars,
            ),
            (
                "html_render_width",
                self.html_render_width,
                Self::HARD_MAXIMUMS.html_render_width,
            ),
        ];
        for (name, value, hard_maximum) in bounded {
            if value == 0 {
                return Err(DfError::Validation(format!(
                    "extraction limit `{name}` must be greater than zero"
                )));
            }
            if value > hard_maximum {
                return Err(DfError::Validation(format!(
                    "extraction limit `{name}` exceeds its hard safety ceiling of {hard_maximum}"
                )));
            }
        }
        if self.text_segment_chars > self.max_text_chars {
            return Err(DfError::Validation(
                "`text_segment_chars` cannot exceed `max_text_chars`".to_string(),
            ));
        }
        if self.max_text_chars > self.max_total_text_chars {
            return Err(DfError::Validation(
                "`max_text_chars` cannot exceed `max_total_text_chars`".to_string(),
            ));
        }
        if self.max_attachment_bytes > self.max_total_attachment_bytes {
            return Err(DfError::Validation(
                "`max_attachment_bytes` cannot exceed `max_total_attachment_bytes`".to_string(),
            ));
        }
        if self.max_archive_entry_bytes > self.max_archive_total_bytes {
            return Err(DfError::Validation(
                "`max_archive_entry_bytes` cannot exceed `max_archive_total_bytes`".to_string(),
            ));
        }
        if self.max_mail_attachments > u64::from(u32::MAX)
            || self.max_archive_entries > u64::from(u32::MAX)
        {
            return Err(DfError::Validation(
                "attachment and archive entry limits must fit a u32 ordinal".to_string(),
            ));
        }
        let maximum_segments = self.max_total_text_chars.div_ceil(self.text_segment_chars);
        if maximum_segments > u64::from(u32::MAX) {
            return Err(DfError::Validation(
                "text limits permit more segments than a u32 ordinal can represent".to_string(),
            ));
        }
        Ok(())
    }

    /// Canonical persisted representation used as extraction-run evidence.
    pub fn canonical_json(&self) -> DfResult<serde_json::Value> {
        self.validate()?;
        serde_json::to_value(self)
            .map_err(|error| DfError::Serialization(format!("extraction limits: {error}")))
    }

    /// SHA-256 of the canonical struct serialization. Struct field order is
    /// stable and all fields are integers, so this is independent of locale.
    pub fn digest(&self) -> DfResult<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| DfError::Serialization(format!("extraction limits: {error}")))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}
