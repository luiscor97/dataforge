//! Deterministic synthetic corpus generator (RFC-0001 §40).
//!
//! Builds a folder tree that looks like a real inherited collection — nested
//! directories, exact duplicates scattered across branches, Unicode names,
//! empty folders, a sprinkle of rule-matching temporaries and the occasional
//! multi-megabyte file — while staying fully reproducible: the same seed and
//! spec always produce the same relative paths and file bytes.
//!
//! Everything is derived from a tiny xorshift64* PRNG so the tool adds no
//! dependencies and the corpus can be regenerated anywhere. The generator
//! writes **only** inside the destination directory it is given, refuses a
//! non-empty destination and never truncates a file that appears concurrently.

use std::io::Write;
use std::path::{Path, PathBuf};

/// xorshift64* — tiny, deterministic, good enough for corpus shaping.
/// Not cryptographic and not meant to be.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // A zero state would be a fixed point; nudge it.
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..n` (n > 0).
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// One weighted size band of a corpus profile. Sizes inside a band are
/// sampled log-uniformly (power-of-two bucket + uniform remainder) so small
/// sizes dominate the band the way they do in real collections, using only
/// integer arithmetic — float rounding must never make two platforms
/// generate different corpora from the same seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeBand {
    /// Relative weight in percent. All bands of a spec must sum to 100.
    pub weight_percent: u8,
    /// Smallest size in bytes (inclusive, >= 1).
    pub min_bytes: u64,
    /// Largest size in bytes (inclusive, >= min).
    pub max_bytes: u64,
}

/// Shape of the corpus to generate.
#[derive(Debug, Clone)]
pub struct CorpusSpec {
    /// Total number of files to write.
    pub files: u64,
    /// PRNG seed; same seed + spec = identical tree.
    pub seed: u64,
    /// Percentage (0–100) of files whose content duplicates an earlier file.
    pub duplicate_percent: u8,
    /// Maximum folder depth below the corpus root.
    pub max_depth: u8,
    /// Every Nth file is ~1 MiB instead of a few KiB (0 disables). Ignored
    /// when `size_bands` is set.
    pub large_file_every: u64,
    /// Optional weighted size distribution (benchmark profiles, M1.0.1).
    /// `None` keeps the legacy small-file shape unchanged.
    pub size_bands: Option<Vec<SizeBand>>,
}

impl Default for CorpusSpec {
    fn default() -> Self {
        Self {
            files: 1_000,
            seed: 42,
            duplicate_percent: 20,
            max_depth: 6,
            large_file_every: 500,
            size_bands: None,
        }
    }
}

impl CorpusSpec {
    /// Reject a spec whose bands cannot be sampled deterministically.
    fn validate(&self) -> std::io::Result<()> {
        if let Some(bands) = &self.size_bands {
            let total: u32 = bands.iter().map(|b| u32::from(b.weight_percent)).sum();
            if bands.is_empty() || total != 100 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("size band weights must sum to 100, got {total}"),
                ));
            }
            for band in bands {
                if band.min_bytes == 0 || band.max_bytes < band.min_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "size bands need 1 <= min <= max",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Sample one size from the weighted bands: pick the band by cumulative
/// weight, then a power-of-two bucket inside it, then a uniform remainder.
/// Integer-only on purpose (see [`SizeBand`]).
fn sample_banded_size(rng: &mut Rng, bands: &[SizeBand]) -> u64 {
    let mut roll = rng.below(100);
    let mut chosen = &bands[bands.len() - 1];
    for band in bands {
        let weight = u64::from(band.weight_percent);
        if roll < weight {
            chosen = band;
            break;
        }
        roll -= weight;
    }
    // Log-uniform: choose an exponent bucket between min and max, then a
    // uniform offset within the bucket, clamped into the band.
    let span_log2 = 64 - (chosen.max_bytes / chosen.min_bytes).leading_zeros() as u64;
    let bucket = rng.below(span_log2.max(1));
    let base = chosen.min_bytes.saturating_shl(bucket as u32);
    let size = base + rng.below(base.max(1));
    size.clamp(chosen.min_bytes, chosen.max_bytes)
}

/// `u64::checked_shl` that saturates instead of wrapping.
trait SaturatingShl {
    fn saturating_shl(self, shift: u32) -> u64;
}

impl SaturatingShl for u64 {
    fn saturating_shl(self, shift: u32) -> u64 {
        self.checked_shl(shift).unwrap_or(u64::MAX)
    }
}

/// What was actually written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CorpusSummary {
    pub files_written: u64,
    pub duplicate_files: u64,
    pub folders_created: u64,
    pub empty_folders: u64,
    pub bytes_written: u64,
}

/// Name pools: plain ASCII, Spanish accents/ñ, and non-Latin scripts, so the
/// scanner's Unicode handling is exercised at scale (RFC-0001 §13.1).
const FILE_STEMS: &[&str] = &[
    "informe",
    "acta \u{f1}",
    "resumen",
    "r\u{e9}sum\u{e9}",
    "escrito",
    "demanda",
    "presupuesto",
    "\u{6863}\u{6848}", // 档案
    "carta",
    "minuta",
];
const FILE_EXTS: &[&str] = &[".txt", ".pdf", ".docx", ".xlsx", ".msg", ".tmp"];
const DIR_STEMS: &[&str] = &[
    "casos",
    "clientes",
    "administraci\u{f3}n",
    "correspondencia",
    "a\u{f1}o",
    "proyectos",
    "material",
    "archivo",
];

/// Deterministic payload for one file: `len` bytes derived from `seed`.
/// Reference implementation for the streaming writer; tests assert equality.
#[cfg(test)]
fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        out.extend_from_slice(&rng.next().to_le_bytes());
    }
    out.truncate(len);
    out
}

/// Stream the deterministic payload to `file` in bounded chunks so a
/// multi-GiB corpus entry never allocates its full size in memory. Byte
/// stream is identical to [`payload`] for the same `(seed, len)`.
fn write_payload(file: &mut std::fs::File, seed: u64, len: u64) -> std::io::Result<()> {
    const CHUNK: usize = 1024 * 1024;
    let mut rng = Rng::new(seed);
    let mut chunk = Vec::with_capacity(CHUNK.min(len as usize));
    let mut remaining = len;
    while remaining > 0 {
        chunk.clear();
        let want = remaining.min(CHUNK as u64) as usize;
        while chunk.len() < want {
            chunk.extend_from_slice(&rng.next().to_le_bytes());
        }
        chunk.truncate(want);
        file.write_all(&chunk)?;
        remaining -= want as u64;
    }
    Ok(())
}

/// Prepare a missing or empty corpus root before the first generated entry.
fn prepare_empty_root(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;

    let metadata = std::fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "corpus destination `{}` exists but is not a plain directory",
                root.display()
            ),
        ));
    }

    if std::fs::read_dir(root)?.next().transpose()?.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("corpus destination `{}` must be empty", root.display()),
        ));
    }
    Ok(())
}

/// Open one generated file without ever reusing or truncating an entry.
fn create_new_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Generate the corpus under `root`.
///
/// The root is created when missing and otherwise must be completely empty.
/// Every file uses create-new semantics, so an entry planted after the empty
/// check makes generation fail instead of overwriting foreign data. Nothing
/// outside `root` is touched and partial output is never cleaned implicitly.
pub fn generate(root: &Path, spec: &CorpusSpec) -> std::io::Result<CorpusSummary> {
    spec.validate()?;
    prepare_empty_root(root)?;

    let mut rng = Rng::new(spec.seed);
    let mut summary = CorpusSummary::default();

    // Candidate directories, with their depth. New subfolders are created as
    // generation proceeds, biased towards shallow levels.
    let mut dirs: Vec<(PathBuf, u8)> = vec![(root.to_path_buf(), 0)];
    // Payload identities already written, so duplicates regenerate the exact
    // same bytes without holding file contents in memory.
    let mut written_payloads: Vec<(u64, u64)> = Vec::new();

    for index in 0..spec.files {
        // Occasionally grow the tree: one new folder every ~8 files.
        if rng.below(8) == 0 {
            let (parent, parent_depth) = dirs[rng.below(dirs.len() as u64) as usize].clone();
            if parent_depth < spec.max_depth {
                let stem = DIR_STEMS[rng.below(DIR_STEMS.len() as u64) as usize];
                let dir = parent.join(format!("{stem}-{:04}", rng.below(10_000)));
                if !dir.exists() {
                    std::fs::create_dir(&dir)?;
                    summary.folders_created += 1;
                    dirs.push((dir, parent_depth + 1));
                }
            }
        }
        // And occasionally leave an empty folder behind (~1 every 200 files).
        if rng.below(200) == 0 {
            let (parent, parent_depth) = dirs[rng.below(dirs.len() as u64) as usize].clone();
            if parent_depth < spec.max_depth {
                let dir = parent.join(format!("vac\u{ed}a-{:04}", rng.below(10_000)));
                if !dir.exists() {
                    std::fs::create_dir(&dir)?;
                    summary.folders_created += 1;
                    summary.empty_folders += 1;
                }
            }
        }

        let (dir, _) = dirs[rng.below(dirs.len() as u64) as usize].clone();
        let stem = FILE_STEMS[rng.below(FILE_STEMS.len() as u64) as usize];
        let ext = FILE_EXTS[rng.below(FILE_EXTS.len() as u64) as usize];
        let name = format!("{stem}-{index:06}{ext}");

        // Duplicate an earlier payload, or mint a new one.
        let duplicate =
            !written_payloads.is_empty() && rng.below(100) < u64::from(spec.duplicate_percent);
        let (payload_seed, len) = if duplicate {
            summary.duplicate_files += 1;
            written_payloads[rng.below(written_payloads.len() as u64) as usize]
        } else {
            let len = if let Some(bands) = &spec.size_bands {
                sample_banded_size(&mut rng, bands)
            } else if spec.large_file_every > 0 && index > 0 && index % spec.large_file_every == 0 {
                1024 * 1024
            } else {
                64 + rng.below(4_032)
            };
            let entry = (rng.next(), len);
            written_payloads.push(entry);
            entry
        };

        let mut file = create_new_file(&dir.join(name))?;
        write_payload(&mut file, payload_seed, len)?;
        summary.files_written += 1;
        summary.bytes_written += len;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact, platform-neutral relative paths plus entry type and file bytes.
    fn tree_fingerprint(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut out = Vec::new();
        let mut queue = vec![root.to_path_buf()];
        while let Some(dir) = queue.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let metadata = entry.metadata().unwrap();
                if metadata.is_dir() {
                    queue.push(entry.path());
                    out.push((relative, None));
                } else {
                    out.push((relative, Some(std::fs::read(entry.path()).unwrap())));
                }
            }
        }
        out.sort();
        out
    }

    /// The whole point of the corpus: identical inputs, identical tree.
    #[test]
    fn the_same_seed_produces_an_identical_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CorpusSpec {
            files: 250,
            ..CorpusSpec::default()
        };
        let a = generate(&tmp.path().join("a"), &spec).unwrap();
        let b = generate(&tmp.path().join("b"), &spec).unwrap();
        assert_eq!(a, b, "summaries must match");
        assert_eq!(
            tree_fingerprint(&tmp.path().join("a")),
            tree_fingerprint(&tmp.path().join("b")),
            "trees must have identical paths, entry types and file bytes"
        );
    }

    #[test]
    fn a_different_seed_produces_a_different_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let base = CorpusSpec {
            files: 120,
            ..CorpusSpec::default()
        };
        let other = CorpusSpec {
            seed: 43,
            ..base.clone()
        };
        generate(&tmp.path().join("a"), &base).unwrap();
        generate(&tmp.path().join("b"), &other).unwrap();
        assert_ne!(
            tree_fingerprint(&tmp.path().join("a")),
            tree_fingerprint(&tmp.path().join("b"))
        );
    }

    #[test]
    fn the_spec_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CorpusSpec {
            files: 400,
            duplicate_percent: 30,
            ..CorpusSpec::default()
        };
        let summary = generate(tmp.path(), &spec).unwrap();
        assert_eq!(summary.files_written, 400);
        assert!(summary.duplicate_files > 0, "duplicates must exist");
        assert!(summary.folders_created > 0, "tree must be nested");
        // Unicode names really landed on disk.
        let fingerprint = tree_fingerprint(tmp.path());
        assert!(fingerprint.iter().any(|(p, _)| p.contains('\u{f1}')));
    }

    #[test]
    fn duplicates_regenerate_identical_bytes() {
        assert_eq!(payload(7, 128), payload(7, 128));
        assert_ne!(payload(7, 128), payload(8, 128));
    }

    /// Streaming must emit byte-for-byte what the in-memory generator emits,
    /// including odd lengths that cross the 1 MiB chunk boundary.
    #[test]
    fn streamed_payload_matches_in_memory_payload() {
        let tmp = tempfile::tempdir().unwrap();
        for len in [0u64, 1, 7, 8, 4_096, 1024 * 1024, 1024 * 1024 + 3] {
            let path = tmp.path().join(format!("p-{len}"));
            let mut file = create_new_file(&path).unwrap();
            write_payload(&mut file, 99, len).unwrap();
            drop(file);
            assert_eq!(
                std::fs::read(&path).unwrap(),
                payload(99, len as usize),
                "len {len} must match"
            );
        }
    }

    #[test]
    fn banded_sizes_stay_in_range_and_are_deterministic() {
        let bands = [
            SizeBand {
                weight_percent: 70,
                min_bytes: 1,
                max_bytes: 64 * 1024,
            },
            SizeBand {
                weight_percent: 30,
                min_bytes: 64 * 1024,
                max_bytes: 10 * 1024 * 1024,
            },
        ];
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..10_000 {
            let size = sample_banded_size(&mut a, &bands);
            assert!((1..=10 * 1024 * 1024).contains(&size));
            assert_eq!(
                size,
                sample_banded_size(&mut b, &bands),
                "must be deterministic"
            );
        }
    }

    #[test]
    fn band_weights_must_sum_to_one_hundred() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CorpusSpec {
            files: 1,
            size_bands: Some(vec![SizeBand {
                weight_percent: 60,
                min_bytes: 1,
                max_bytes: 10,
            }]),
            ..CorpusSpec::default()
        };
        let error = generate(&tmp.path().join("bad"), &spec).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_banded_corpus_is_reproducible() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = CorpusSpec {
            files: 300,
            size_bands: Some(vec![
                SizeBand {
                    weight_percent: 90,
                    min_bytes: 1,
                    max_bytes: 16 * 1024,
                },
                SizeBand {
                    weight_percent: 10,
                    min_bytes: 16 * 1024,
                    max_bytes: 256 * 1024,
                },
            ]),
            ..CorpusSpec::default()
        };
        let a = generate(&tmp.path().join("a"), &spec).unwrap();
        let b = generate(&tmp.path().join("b"), &spec).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            tree_fingerprint(&tmp.path().join("a")),
            tree_fingerprint(&tmp.path().join("b"))
        );
    }

    #[test]
    fn a_non_empty_root_is_rejected_without_modifying_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("corpus");
        std::fs::create_dir(&root).unwrap();
        let sentinel = root.join("foreign.txt");
        std::fs::write(&sentinel, b"foreign bytes").unwrap();

        let error = generate(
            &root,
            &CorpusSpec {
                files: 10,
                ..CorpusSpec::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"foreign bytes");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn create_new_refuses_a_raced_file_without_truncating_it() {
        let tmp = tempfile::tempdir().unwrap();
        let raced = tmp.path().join("raced.txt");
        std::fs::write(&raced, b"foreign bytes").unwrap();

        let error = create_new_file(&raced).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&raced).unwrap(), b"foreign bytes");
    }

    #[test]
    fn the_exact_fingerprint_detects_same_length_content_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("corpus");
        generate(
            &root,
            &CorpusSpec {
                files: 10,
                max_depth: 0,
                ..CorpusSpec::default()
            },
        )
        .unwrap();
        let before = tree_fingerprint(&root);
        let (relative, original) = before
            .iter()
            .find_map(|(relative, bytes)| bytes.as_ref().map(|bytes| (relative, bytes)))
            .unwrap();
        let mut changed = original.clone();
        changed[0] ^= 0xff;
        let path = relative
            .split('/')
            .fold(root.clone(), |path, component| path.join(component));
        std::fs::write(path, changed).unwrap();

        assert_ne!(before, tree_fingerprint(&root));
    }
}
