//! Physical file fingerprints (RFC-0001 §13.5, §14.1, §14.5).
//!
//! A fingerprint is the cheap identity used to notice that a file changed
//! between scan, hash and copy. It is **not** a content hash.
//!
//! ## Why v2 (ADR-0019)
//!
//! v1 was `(size, mtime)`. That misses the case that matters most on inherited
//! material: a file swapped for a different one of the same size, with the
//! mtime preserved (every copy tool can do this, and `SetFileTime` makes it
//! trivial). DataForge would then hash one file and copy another, and call the
//! result verified.
//!
//! v2 adds the filesystem's own identity — volume serial + file id, the
//! closest thing to an inode — plus change time and attributes. Two different
//! files cannot share a file id on the same volume, so the swap is caught.
//!
//! ## Degraded identity is not "no change"
//!
//! Some filesystems (notably some network redirectors) do not hand out file
//! ids. When that happens the fingerprint is **degraded**, and comparing two
//! degraded fingerprints can only ever say "nothing I can see changed" — never
//! "this is the same file". The verdict type says so out loud rather than
//! letting a caller mistake one for the other, and v1 tokens are always
//! degraded: they carry no identity at all.

use serde::{Deserialize, Serialize};

use df_error::{DfError, DfResult};

/// The filesystem's own identity for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalIdentity {
    pub volume_serial: u64,
    pub file_id: u64,
}

/// v1: size + modification time. No physical identity (ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintV1 {
    pub size_bytes: u64,
    /// Unix milliseconds, when the filesystem reported one.
    pub modified_at_ms: Option<i64>,
}

/// v2: v1 plus physical identity, change time and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintV2 {
    pub size_bytes: u64,
    pub modified_at_ms: Option<i64>,
    /// NTFS change time: moves when metadata changes, even if the writer
    /// restored the modification time. `None` where unavailable.
    pub change_time_ms: Option<i64>,
    pub attributes: u32,
    /// `None` means the filesystem could not identify the file: the
    /// fingerprint is degraded, which is *not* the same as unchanged.
    pub identity: Option<PhysicalIdentity>,
}

/// A versioned fingerprint. Stored as a token, always parsed before use — the
/// domain never compares fingerprints as opaque strings (ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "version")]
pub enum FileFingerprint {
    V1(FingerprintV1),
    V2(FingerprintV2),
}

/// How much a fingerprint can promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FingerprintGuarantee {
    /// Carries the filesystem's identity: a substitution is detectable.
    Physical,
    /// No identity available (v1, or a filesystem without file ids).
    Degraded,
}

/// The result of comparing two fingerprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FingerprintVerdict {
    /// Same physical file: identity matched on both sides.
    SamePhysical,
    /// Everything both sides carry agrees, but at least one lacks identity,
    /// so a same-size same-mtime substitution cannot be ruled out.
    SameDegraded,
    /// Demonstrably a different file (or a changed one).
    Changed,
}

impl FingerprintVerdict {
    /// Did anything observable change? Callers that only need "carry on or
    /// stop" use this; those that must report the strength of the evidence
    /// look at the variant itself.
    pub fn is_changed(self) -> bool {
        matches!(self, Self::Changed)
    }
}

const NONE: &str = "none";

fn opt_i64(text: &str) -> DfResult<Option<i64>> {
    if text == NONE {
        return Ok(None);
    }
    text.parse::<i64>()
        .map(Some)
        .map_err(|_| DfError::Validation(format!("bad fingerprint number `{text}`")))
}

fn opt_u64(text: &str) -> DfResult<Option<u64>> {
    if text == NONE {
        return Ok(None);
    }
    text.parse::<u64>()
        .map(Some)
        .map_err(|_| DfError::Validation(format!("bad fingerprint number `{text}`")))
}

fn show_i64(value: Option<i64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| NONE.into())
}

impl FileFingerprint {
    /// Canonical token stored in SQLite.
    ///
    /// ```text
    /// v1:<size>:<mtime|none>
    /// v2:<size>:<mtime|none>:<ctime|none>:<attrs>:<volume|none>:<file_id|none>
    /// ```
    ///
    /// The version prefix means a v1 token can never accidentally compare
    /// equal to a v2 one.
    pub fn token(&self) -> String {
        match self {
            Self::V1(v1) => format!("v1:{}:{}", v1.size_bytes, show_i64(v1.modified_at_ms)),
            Self::V2(v2) => {
                let (volume, file_id) = match v2.identity {
                    Some(id) => (id.volume_serial.to_string(), id.file_id.to_string()),
                    None => (NONE.to_string(), NONE.to_string()),
                };
                format!(
                    "v2:{}:{}:{}:{}:{}:{}",
                    v2.size_bytes,
                    show_i64(v2.modified_at_ms),
                    show_i64(v2.change_time_ms),
                    v2.attributes,
                    volume,
                    file_id
                )
            }
        }
    }

    /// Parse a stored token. v1 tokens written by earlier versions keep
    /// working (ADR-0019 compatibility).
    pub fn parse(token: &str) -> DfResult<Self> {
        let bad = || DfError::Validation(format!("unparsable fingerprint `{token}`"));
        let parts: Vec<&str> = token.split(':').collect();
        match parts.first().copied() {
            Some("v1") => {
                if parts.len() != 3 {
                    return Err(bad());
                }
                Ok(Self::V1(FingerprintV1 {
                    size_bytes: parts[1].parse().map_err(|_| bad())?,
                    modified_at_ms: opt_i64(parts[2])?,
                }))
            }
            Some("v2") => {
                if parts.len() != 7 {
                    return Err(bad());
                }
                let volume = opt_u64(parts[5])?;
                let file_id = opt_u64(parts[6])?;
                let identity = match (volume, file_id) {
                    (Some(volume_serial), Some(file_id)) => Some(PhysicalIdentity {
                        volume_serial,
                        file_id,
                    }),
                    // Half an identity is no identity.
                    _ => None,
                };
                Ok(Self::V2(FingerprintV2 {
                    size_bytes: parts[1].parse().map_err(|_| bad())?,
                    modified_at_ms: opt_i64(parts[2])?,
                    change_time_ms: opt_i64(parts[3])?,
                    attributes: parts[4].parse().map_err(|_| bad())?,
                    identity,
                }))
            }
            _ => Err(bad()),
        }
    }

    pub fn size_bytes(&self) -> u64 {
        match self {
            Self::V1(v1) => v1.size_bytes,
            Self::V2(v2) => v2.size_bytes,
        }
    }

    pub fn modified_at_ms(&self) -> Option<i64> {
        match self {
            Self::V1(v1) => v1.modified_at_ms,
            Self::V2(v2) => v2.modified_at_ms,
        }
    }

    pub fn identity(&self) -> Option<PhysicalIdentity> {
        match self {
            Self::V1(_) => None,
            Self::V2(v2) => v2.identity,
        }
    }

    /// How strong this fingerprint's evidence is.
    pub fn guarantee(&self) -> FingerprintGuarantee {
        match self.identity() {
            Some(_) => FingerprintGuarantee::Physical,
            None => FingerprintGuarantee::Degraded,
        }
    }

    /// Compare a stored fingerprint with a freshly captured one.
    ///
    /// Deliberately not `PartialEq`: equality would force a yes/no answer and
    /// hide the difference between "same file, proven" and "nothing I can see
    /// changed". Mixing a v1 and a v2 can only ever yield the degraded verdict
    /// — the two are never declared equivalent.
    pub fn compare(stored: &Self, current: &Self) -> FingerprintVerdict {
        if stored.size_bytes() != current.size_bytes() {
            return FingerprintVerdict::Changed;
        }
        // A missing timestamp on either side is not a mismatch; it is a gap.
        if let (Some(a), Some(b)) = (stored.modified_at_ms(), current.modified_at_ms()) {
            if a != b {
                return FingerprintVerdict::Changed;
            }
        }
        if let (Self::V2(a), Self::V2(b)) = (stored, current) {
            if let (Some(x), Some(y)) = (a.change_time_ms, b.change_time_ms) {
                if x != y {
                    return FingerprintVerdict::Changed;
                }
            }
            if a.attributes != b.attributes {
                return FingerprintVerdict::Changed;
            }
            match (a.identity, b.identity) {
                // The whole point of v2: same size and mtime, different file.
                (Some(x), Some(y)) if x != y => return FingerprintVerdict::Changed,
                (Some(x), Some(y)) if x == y => return FingerprintVerdict::SamePhysical,
                _ => {}
            }
        }
        FingerprintVerdict::SameDegraded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2(size: u64, mtime: i64, id: Option<(u64, u64)>) -> FileFingerprint {
        FileFingerprint::V2(FingerprintV2 {
            size_bytes: size,
            modified_at_ms: Some(mtime),
            change_time_ms: Some(mtime),
            attributes: 32,
            identity: id.map(|(volume_serial, file_id)| PhysicalIdentity {
                volume_serial,
                file_id,
            }),
        })
    }

    #[test]
    fn v1_tokens_still_parse() {
        let parsed = FileFingerprint::parse("v1:42:1700000000000").unwrap();
        assert_eq!(parsed.size_bytes(), 42);
        assert_eq!(parsed.modified_at_ms(), Some(1_700_000_000_000));
        assert_eq!(parsed.guarantee(), FingerprintGuarantee::Degraded);

        let no_time = FileFingerprint::parse("v1:42:none").unwrap();
        assert_eq!(no_time.modified_at_ms(), None);
    }

    #[test]
    fn tokens_round_trip_in_both_versions() {
        for fp in [
            FileFingerprint::V1(FingerprintV1 {
                size_bytes: 7,
                modified_at_ms: Some(5),
            }),
            v2(7, 5, Some((10, 20))),
            v2(7, 5, None),
        ] {
            assert_eq!(FileFingerprint::parse(&fp.token()).unwrap(), fp);
        }
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        for bad in ["", "v3:1:2", "v1:1", "v1:1:2:3", "v2:1:2", "v2:a:b:c:d:e:f"] {
            assert!(FileFingerprint::parse(bad).is_err(), "`{bad}` should fail");
        }
    }

    /// The reason v2 exists: same size, same mtime, different file.
    #[test]
    fn a_substitution_with_identical_size_and_mtime_is_detected() {
        let before = v2(100, 1_700, Some((1, 111)));
        let after = v2(100, 1_700, Some((1, 222)));
        assert_eq!(
            FileFingerprint::compare(&before, &after),
            FingerprintVerdict::Changed
        );
    }

    #[test]
    fn the_same_file_compares_as_physically_same() {
        let fp = v2(100, 1_700, Some((1, 111)));
        assert_eq!(
            FileFingerprint::compare(&fp, &fp),
            FingerprintVerdict::SamePhysical
        );
        assert_eq!(fp.guarantee(), FingerprintGuarantee::Physical);
    }

    /// A file moved within a volume keeps its file id, so it is still the same
    /// object (documented behaviour, ADR-0019): the *path* changing is a
    /// different question, tracked by the occurrence, not the fingerprint.
    #[test]
    fn a_move_within_the_volume_keeps_the_identity() {
        let before = v2(100, 1_700, Some((1, 111)));
        let after = v2(100, 1_700, Some((1, 111)));
        assert_eq!(
            FileFingerprint::compare(&before, &after),
            FingerprintVerdict::SamePhysical
        );
    }

    /// Copying the content into a new file yields a new identity.
    #[test]
    fn a_copy_of_the_content_is_a_different_object() {
        let original = v2(100, 1_700, Some((1, 111)));
        let copy = v2(100, 1_700, Some((1, 999)));
        assert_eq!(
            FileFingerprint::compare(&original, &copy),
            FingerprintVerdict::Changed
        );
    }

    #[test]
    fn without_identity_the_verdict_is_degraded_never_physical() {
        let a = v2(100, 1_700, None);
        let b = v2(100, 1_700, None);
        assert_eq!(
            FileFingerprint::compare(&a, &b),
            FingerprintVerdict::SameDegraded
        );
        assert_eq!(a.guarantee(), FingerprintGuarantee::Degraded);
    }

    /// v1 and v2 are never declared equivalent: the best they can reach is the
    /// degraded verdict, and a real difference still shows up as Changed.
    #[test]
    fn v1_and_v2_never_compare_as_physically_same() {
        let stored = FileFingerprint::V1(FingerprintV1 {
            size_bytes: 100,
            modified_at_ms: Some(1_700),
        });
        let current = v2(100, 1_700, Some((1, 111)));
        assert_eq!(
            FileFingerprint::compare(&stored, &current),
            FingerprintVerdict::SameDegraded
        );
        let grown = v2(101, 1_700, Some((1, 111)));
        assert_eq!(
            FileFingerprint::compare(&stored, &grown),
            FingerprintVerdict::Changed
        );
    }

    #[test]
    fn a_changed_size_or_mtime_is_always_a_change() {
        let base = v2(100, 1_700, Some((1, 111)));
        assert!(FileFingerprint::compare(&base, &v2(101, 1_700, Some((1, 111)))).is_changed());
        assert!(FileFingerprint::compare(&base, &v2(100, 1_800, Some((1, 111)))).is_changed());
    }
}
