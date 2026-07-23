//! `df-corpus` — generate a deterministic synthetic corpus on disk.
//!
//! Development tool (RFC-0001 §40): the corpus feeds the scale tests and the
//! M1.0.1 performance benchmarks; it is never shipped as product
//! functionality. It writes only inside `--output`.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use df_corpus::{CorpusSpec, SizeBand};

/// Benchmark corpus profiles (M1.0.1). Each profile pins a deterministic
/// size distribution; `--files` scales the count so CI can run a small
/// version of the same shape.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Profile {
    /// A — Small Files: everything 1 B..16 KiB (default 100_000 files).
    ASmall,
    /// B — Mixed: 70% <64 KiB, 20% 64 KiB..10 MiB, 9% 10..64 MiB, 1%
    /// 64..256 MiB (default 10_000 files; the literal RFC shape at 100_000
    /// files needs >1 TB free and stays opt-in via --files).
    BMixed,
    /// C — Large Files: 100 MiB..2 GiB per file (default 50 files).
    CLarge,
    /// D — Million Entries: the M0.8 small-file shape (default 1_000_000
    /// files; use --files for a CI-sized version).
    DMillion,
}

impl Profile {
    fn default_files(self) -> u64 {
        match self {
            Profile::ASmall => 100_000,
            Profile::BMixed => 10_000,
            Profile::CLarge => 50,
            Profile::DMillion => 1_000_000,
        }
    }

    fn spec(self, files: u64, seed: u64) -> CorpusSpec {
        const KIB: u64 = 1024;
        const MIB: u64 = 1024 * 1024;
        let band = |weight_percent: u8, min_bytes: u64, max_bytes: u64| SizeBand {
            weight_percent,
            min_bytes,
            max_bytes,
        };
        match self {
            Profile::ASmall => CorpusSpec {
                files,
                seed,
                duplicate_percent: 15,
                max_depth: 6,
                large_file_every: 0,
                size_bands: Some(vec![band(100, 1, 16 * KIB)]),
            },
            Profile::BMixed => CorpusSpec {
                files,
                seed,
                duplicate_percent: 15,
                max_depth: 6,
                large_file_every: 0,
                size_bands: Some(vec![
                    band(70, 1, 64 * KIB),
                    band(20, 64 * KIB, 10 * MIB),
                    band(9, 10 * MIB, 64 * MIB),
                    band(1, 64 * MIB, 256 * MIB),
                ]),
            },
            Profile::CLarge => CorpusSpec {
                files,
                seed,
                duplicate_percent: 4,
                max_depth: 2,
                large_file_every: 0,
                size_bands: Some(vec![band(100, 100 * MIB, 2048 * MIB)]),
            },
            // The M0.8 legacy shape: small files + ~1 MiB every 500.
            Profile::DMillion => CorpusSpec {
                files,
                seed,
                duplicate_percent: 20,
                max_depth: 6,
                large_file_every: 500,
                size_bands: None,
            },
        }
    }
}

#[derive(Parser)]
#[command(
    name = "df-corpus",
    version,
    about = "Deterministic synthetic corpus generator for DataForge tests"
)]
struct Cli {
    /// Directory to generate the corpus in (created if missing).
    #[arg(long)]
    output: PathBuf,
    /// Benchmark profile (M1.0.1). Omitted = legacy free-form flags below.
    #[arg(long, value_enum)]
    profile: Option<Profile>,
    /// Number of files to write. With --profile, overrides its default count.
    #[arg(long)]
    files: Option<u64>,
    /// PRNG seed; the same seed produces an identical tree.
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Percentage (0-100) of files duplicating an earlier file's content.
    /// Ignored when --profile is set.
    #[arg(long, default_value_t = 20)]
    duplicate_percent: u8,
    /// Maximum folder depth below the corpus root. Ignored with --profile.
    #[arg(long, default_value_t = 6)]
    max_depth: u8,
    /// Every Nth file is ~1 MiB (0 disables). Ignored with --profile.
    #[arg(long, default_value_t = 500)]
    large_file_every: u64,
}

fn main() {
    let cli = Cli::parse();
    let spec = match cli.profile {
        Some(profile) => {
            let files = cli.files.unwrap_or_else(|| profile.default_files());
            profile.spec(files, cli.seed)
        }
        None => CorpusSpec {
            files: cli.files.unwrap_or(1_000),
            seed: cli.seed,
            duplicate_percent: cli.duplicate_percent.min(100),
            max_depth: cli.max_depth,
            large_file_every: cli.large_file_every,
            size_bands: None,
        },
    };
    match df_corpus::generate(&cli.output, &spec) {
        Ok(summary) => {
            if let Some(profile) = cli.profile {
                println!("Profile   : {profile:?}");
            }
            println!("Corpus    : {}", cli.output.display());
            println!("Seed      : {}", cli.seed);
            println!("Files     : {}", summary.files_written);
            println!("Duplicates: {}", summary.duplicate_files);
            println!("Folders   : {}", summary.folders_created);
            println!("Empty dirs: {}", summary.empty_folders);
            println!("Bytes     : {}", summary.bytes_written);
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
