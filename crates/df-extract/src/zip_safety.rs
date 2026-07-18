use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::path::Path;

use df_fs_safety::SafeRelativePath;
use zip::ZipArchive;

use crate::ExtractionLimits;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ZipEntryHeader {
    pub index: usize,
    pub name: String,
    pub compressed_bytes: u64,
    pub size_bytes: u64,
    pub crc32: u32,
    pub encrypted: bool,
    pub directory: bool,
}

pub(crate) struct ValidatedZip<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    pub entries: Vec<ZipEntryHeader>,
    pub declared_bytes: u64,
}

impl<'a> ValidatedZip<'a> {
    /// Parse and validate the complete central directory before any entry is
    /// inflated. A rejected path or bomb declaration therefore yields no
    /// partial reads and, by construction, no filesystem writes.
    pub fn open(
        bytes: &'a [u8],
        limits: &ExtractionLimits,
        remaining_entries: u64,
        remaining_expanded_bytes: u64,
    ) -> Result<Self, String> {
        let mut archive = ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| format!("invalid ZIP container: {error}"))?;
        let count = u64::try_from(archive.len()).map_err(|_| "ZIP entry count overflow")?;
        if count > remaining_entries || count > limits.max_archive_entries {
            return Err(format!(
                "ZIP entry count {count} exceeds the configured remaining limit"
            ));
        }

        let mut entries = Vec::with_capacity(archive.len());
        let mut declared_bytes = 0_u64;
        let mut canonical_names = BTreeSet::new();
        for index in 0..archive.len() {
            // Raw mode exposes metadata even for encrypted entries and never
            // starts decompression.
            let file = archive
                .by_index_raw(index)
                .map_err(|error| format!("cannot inspect ZIP entry {index}: {error}"))?;
            let name = file.name().to_string();
            validate_entry_name(&name, file.is_dir(), limits)?;
            let collision_key = name.trim_end_matches('/').to_lowercase();
            if !canonical_names.insert(collision_key) {
                return Err(format!(
                    "ZIP contains a duplicate or case-colliding entry at ordinal {index}"
                ));
            }

            let size = file.size();
            let compressed = file.compressed_size();
            if size > limits.max_archive_entry_bytes {
                return Err(format!(
                    "ZIP entry {index} declares {size} bytes, above the per-entry limit"
                ));
            }
            if !file.is_dir() && size > 0 {
                if compressed == 0 {
                    return Err(format!(
                        "ZIP entry {index} declares non-empty output from zero compressed bytes"
                    ));
                }
                let allowed = compressed.saturating_mul(limits.max_archive_compression_ratio);
                if size > allowed {
                    return Err(format!(
                        "ZIP entry {index} exceeds the configured compression ratio"
                    ));
                }
            }
            declared_bytes = declared_bytes
                .checked_add(size)
                .ok_or_else(|| "ZIP declared-size sum overflow".to_string())?;
            if declared_bytes > remaining_expanded_bytes
                || declared_bytes > limits.max_archive_total_bytes
            {
                return Err("ZIP declared output exceeds the expansion budget".to_string());
            }
            entries.push(ZipEntryHeader {
                index,
                name,
                compressed_bytes: compressed,
                size_bytes: size,
                crc32: file.crc32(),
                encrypted: file.encrypted(),
                directory: file.is_dir(),
            });
        }

        Ok(Self {
            archive,
            entries,
            declared_bytes,
        })
    }

    /// Inflate exactly one preflighted entry through a hard `Read::take`
    /// boundary, then require the actual size and CRC-checked EOF to agree with
    /// the central directory declaration.
    pub fn read_entry(&mut self, entry: &ZipEntryHeader) -> Result<Vec<u8>, String> {
        if entry.directory {
            return Ok(Vec::new());
        }
        if entry.encrypted {
            return Err(format!("ZIP entry {} is encrypted", entry.index));
        }
        let file = self
            .archive
            .by_index(entry.index)
            .map_err(|error| format!("cannot open ZIP entry {}: {error}", entry.index))?;
        let cap = entry
            .size_bytes
            .checked_add(1)
            .ok_or_else(|| "ZIP entry read limit overflow".to_string())?;
        let capacity = usize::try_from(entry.size_bytes)
            .map_err(|_| "ZIP entry size does not fit this platform".to_string())?;
        let mut data = Vec::with_capacity(capacity);
        file.take(cap)
            .read_to_end(&mut data)
            .map_err(|error| format!("cannot read ZIP entry {}: {error}", entry.index))?;
        let actual = u64::try_from(data.len()).map_err(|_| "ZIP entry size overflow")?;
        if actual != entry.size_bytes {
            return Err(format!(
                "ZIP entry {} actual size {actual} differs from declared size {}",
                entry.index, entry.size_bytes
            ));
        }
        Ok(data)
    }
}

fn validate_entry_name(
    name: &str,
    directory: bool,
    limits: &ExtractionLimits,
) -> Result<(), String> {
    let char_count = u64::try_from(name.chars().count()).unwrap_or(u64::MAX);
    if char_count == 0 || char_count > limits.max_virtual_path_chars {
        return Err("ZIP entry name is empty or exceeds the virtual-path limit".to_string());
    }
    if name.contains('\\') {
        return Err("ZIP entry name contains a non-canonical backslash separator".to_string());
    }
    if name.starts_with('/') || name.starts_with("//") {
        return Err("ZIP entry name is absolute".to_string());
    }
    if directory != name.ends_with('/') {
        return Err("ZIP entry directory marker is inconsistent".to_string());
    }
    let canonical = if directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    if canonical.is_empty() || canonical.contains("//") {
        return Err("ZIP entry name contains an empty path component".to_string());
    }
    let parts = canonical.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err("ZIP entry name contains traversal or dot components".to_string());
    }
    let depth = u64::try_from(parts.len()).unwrap_or(u64::MAX);
    if depth > limits.max_archive_path_depth {
        return Err("ZIP entry path depth exceeds the configured limit".to_string());
    }
    SafeRelativePath::parse(Path::new(canonical))
        .map_err(|error| format!("illegal ZIP entry name: {error}"))?;
    Ok(())
}
