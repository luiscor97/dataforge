//! `df-corpus` — generate a deterministic synthetic corpus on disk.
//!
//! Development tool (RFC-0001 §40): the corpus feeds the scale tests and is
//! never shipped as product functionality. It writes only inside `--output`.

use std::path::PathBuf;

use clap::Parser;

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
    /// Number of files to write.
    #[arg(long, default_value_t = 1_000)]
    files: u64,
    /// PRNG seed; the same seed produces an identical tree.
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Percentage (0-100) of files duplicating an earlier file's content.
    #[arg(long, default_value_t = 20)]
    duplicate_percent: u8,
    /// Maximum folder depth below the corpus root.
    #[arg(long, default_value_t = 6)]
    max_depth: u8,
    /// Every Nth file is ~1 MiB (0 disables large files).
    #[arg(long, default_value_t = 500)]
    large_file_every: u64,
}

fn main() {
    let cli = Cli::parse();
    let spec = df_corpus::CorpusSpec {
        files: cli.files,
        seed: cli.seed,
        duplicate_percent: cli.duplicate_percent.min(100),
        max_depth: cli.max_depth,
        large_file_every: cli.large_file_every,
    };
    match df_corpus::generate(&cli.output, &spec) {
        Ok(summary) => {
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
