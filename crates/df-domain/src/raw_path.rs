//! Exact, reversible path representation (RFC-0001 §13.4, ADR-0020).
//!
//! Windows paths are sequences of UTF-16 code units, and they are **not
//! required to be valid UTF-16**: an unpaired surrogate is a perfectly legal
//! filename. `to_string_lossy` turns those into U+FFFD, and U+FFFD is a real
//! character — so the lossy name may name a *different file*, or no file at
//! all. Recording `name_is_lossy` told us a name was damaged but not what it
//! originally was, which is no help when the job is to copy it.
//!
//! So DataForge keeps three separate things, and never confuses them:
//!
//! | form | what it is | used for |
//! |---|---|---|
//! | **display** | `to_string_lossy` | showing a human, logs, reports |
//! | **comparison** | lowercased display | grouping, dedup keys, indexes |
//! | **raw** | the exact UTF-16 code units | **opening the file** |
//!
//! Only the raw form is authoritative. Any code that opens, reads or copies a
//! source file must reconstruct its path from [`RawPath`]; using the display
//! form to touch the filesystem is a bug, not a shortcut.
//!
//! ## The one storage strategy (ADR-0020)
//!
//! The raw form is stored as **little-endian UTF-16 in a SQLite BLOB**. Where
//! a binary blob cannot travel — inside the canonical JSON of the execution
//! manifest — the very same bytes are rendered as lowercase hex. One strategy,
//! one encoding, two renderings.

use serde::{Deserialize, Serialize};

use df_error::{DfError, DfResult};

/// A path (or path component) kept exactly as the OS gave it to us.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawPath {
    /// UTF-16 code units, possibly including unpaired surrogates.
    units: Vec<u16>,
}

impl RawPath {
    /// Capture the exact representation of an OS string.
    #[cfg(windows)]
    pub fn from_os_str(value: &std::ffi::OsStr) -> Self {
        use std::os::windows::ffi::OsStrExt;
        Self {
            units: value.encode_wide().collect(),
        }
    }

    /// Non-Windows: encode the (already UTF-8) name as UTF-16.
    ///
    /// Lossless for anything Rust can hold in an `OsStr` on these platforms
    /// that is valid Unicode. Byte sequences that are not valid UTF-8 are a
    /// Unix concern and are out of scope in v0.1.1-dev (ADR-0020).
    #[cfg(not(windows))]
    pub fn from_os_str(value: &std::ffi::OsStr) -> Self {
        Self {
            units: value.to_string_lossy().encode_utf16().collect(),
        }
    }

    /// Rebuild the OS string. This is the only form allowed to reach the
    /// filesystem.
    #[cfg(windows)]
    pub fn to_os_string(&self) -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&self.units)
    }

    #[cfg(not(windows))]
    pub fn to_os_string(&self) -> std::ffi::OsString {
        std::ffi::OsString::from(String::from_utf16_lossy(&self.units))
    }

    /// The exact bytes stored in SQLite: little-endian UTF-16.
    pub fn to_blob(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.units.len() * 2);
        for unit in &self.units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    }

    /// Read back the stored blob.
    pub fn from_blob(bytes: &[u8]) -> DfResult<Self> {
        if bytes.len() % 2 != 0 {
            return Err(DfError::Validation(format!(
                "raw path blob has an odd length ({}); it is not UTF-16LE",
                bytes.len()
            )));
        }
        Ok(Self {
            units: bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect(),
        })
    }

    /// The same bytes as lowercase hex, for JSON (which cannot carry a blob).
    pub fn to_hex(&self) -> String {
        self.to_blob().iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(text: &str) -> DfResult<Self> {
        if text.len() % 2 != 0 {
            return Err(DfError::Validation(
                "raw path hex has an odd length".to_string(),
            ));
        }
        let mut bytes = Vec::with_capacity(text.len() / 2);
        for pair in text.as_bytes().chunks_exact(2) {
            let hex = std::str::from_utf8(pair)
                .map_err(|_| DfError::Validation("raw path hex is not ASCII".to_string()))?;
            bytes.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| DfError::Validation(format!("bad hex `{hex}`")))?,
            );
        }
        Self::from_blob(&bytes)
    }

    /// Lossy text for humans. **Never** use this to open anything.
    pub fn display(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }

    /// Comparison key: the display form, lowercased. Deliberately separate
    /// from the raw form — two different raw names can share a key, which is
    /// fine for grouping and fatal for opening.
    pub fn comparison_key(&self) -> String {
        self.display().to_lowercase()
    }

    /// Would the display form lose information? True when the name is not
    /// representable as UTF-8 (an unpaired surrogate, typically).
    pub fn is_lossy(&self) -> bool {
        String::from_utf16(&self.units).is_err()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(units: &[u16]) -> RawPath {
        RawPath {
            units: units.to_vec(),
        }
    }

    #[test]
    fn ordinary_unicode_names_round_trip() {
        for name in [
            "acta.docx",
            "expediente ñ.pdf",
            "документ.txt",
            "文件.txt",
            "emoji 🎉.txt",
            "",
        ] {
            let original = RawPath::from_os_str(std::ffi::OsStr::new(name));
            assert_eq!(original.display(), name);
            assert!(!original.is_lossy(), "`{name}` should be representable");
            assert_eq!(RawPath::from_blob(&original.to_blob()).unwrap(), original);
            assert_eq!(RawPath::from_hex(&original.to_hex()).unwrap(), original);
        }
    }

    /// The case that motivates the whole thing: a lone surrogate is a legal
    /// Windows filename and is *not* valid UTF-16.
    #[test]
    fn an_unpaired_surrogate_survives_the_round_trip_exactly() {
        // 0xD800 with no low surrogate following it.
        let original = raw(&[0x0061, 0xD800, 0x0062]);
        assert!(
            original.is_lossy(),
            "this name is not representable in UTF-8"
        );

        // The display form is damaged: it silently became a replacement char.
        assert_eq!(original.display(), "a\u{FFFD}b");
        // Re-encoding the display form does NOT get us back: this is exactly
        // why the lossy form must never reach the filesystem.
        let from_display = RawPath::from_os_str(std::ffi::OsStr::new(&original.display()));
        assert_ne!(from_display, original);

        // The raw form does survive, through both renderings.
        assert_eq!(RawPath::from_blob(&original.to_blob()).unwrap(), original);
        assert_eq!(RawPath::from_hex(&original.to_hex()).unwrap(), original);
    }

    #[test]
    fn the_blob_is_little_endian_utf16() {
        let path = raw(&[0x0041, 0xD800]); // 'A', lone surrogate
        assert_eq!(path.to_blob(), vec![0x41, 0x00, 0x00, 0xD8]);
        assert_eq!(path.to_hex(), "410000d8");
    }

    #[test]
    fn malformed_stored_values_are_rejected() {
        assert!(RawPath::from_blob(&[0x41]).is_err(), "odd length");
        assert!(RawPath::from_hex("abc").is_err(), "odd length");
        assert!(RawPath::from_hex("zz").is_err(), "not hex");
    }

    #[test]
    fn the_comparison_key_is_separate_from_the_raw_form() {
        let upper = RawPath::from_os_str(std::ffi::OsStr::new("Acta.DOCX"));
        let lower = RawPath::from_os_str(std::ffi::OsStr::new("acta.docx"));
        // Same key: good for grouping.
        assert_eq!(upper.comparison_key(), lower.comparison_key());
        // Different raw: they are different names on disk, and only the raw
        // form may be used to open either of them.
        assert_ne!(upper, lower);
    }

    #[cfg(windows)]
    #[test]
    fn a_real_unicode_file_reopens_through_its_raw_path() {
        let tmp = tempfile::tempdir().unwrap();
        let name = "acta ñ 文件 🎉.txt";
        let path = tmp.path().join(name);
        std::fs::write(&path, b"contenido").unwrap();

        let raw = RawPath::from_os_str(path.as_os_str());
        let reopened = std::path::PathBuf::from(raw.to_os_string());
        assert_eq!(std::fs::read(&reopened).unwrap(), b"contenido");
        assert_eq!(reopened, path);
    }
}
