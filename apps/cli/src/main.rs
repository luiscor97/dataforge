//! `dataforge` — command line client of the DataForge engine.
//!
//! The CLI contains no engine logic: inventory, similarity, content
//! intelligence, planning, execution and audit all go through `df-facade`
//! (RFC-0001 rules 16/17).
//! Exit codes follow RFC-0001 §33.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use df_domain::Actor;
use df_error::{DfError, DfResult};
use df_facade::{
    AiAssistOutcome, AnalyzeOutcome, AnomalyReport, ApproveOutcome, AssistanceAuditView,
    AuditReport, ContentArtifactBuildOutcome, ContentExtractionOptions, ContentExtractionOutcome,
    ContentQueryOutcome, ContentSearchOutcome, ContextReport, CreateProjectRequest,
    DuplicateReport, ExecuteOutcome, ExtractionLimits, HashOutcome, MediaOutcome,
    MediaProjectOptions, MediaReport, MediaSidecars, PlanOutcome, PlanValidationReport,
    PluginRegistrationView, PluginReport, PluginsOutcome, ProjectStatus, QueryOptions,
    RegisteredPluginMetadata, ReviewQueue, ScanOutcome, SearchBuildOptions, SearchRequest,
    SimilarityOptions, SimilarityOutcome, SimilarityReport, SnapshotBuildOptions, TreeCloneReport,
    TreeRelationReport, VerifyOutcome,
};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "dataforge",
    version,
    about = "Open-source document reconstruction engine (local-first)",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Emit machine-readable JSON instead of human text.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Manage reconstruction projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Inventory the source roots into a new snapshot (read-only).
    Scan {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Compute BLAKE3 + SHA-256 for every scanned file. Resumable.
    Hash {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Carry content identity forward from the previous snapshot when
        /// the physical fingerprint is byte-identical (ADR-0035). Full
        /// mode remains the default and the evidential recommendation.
        #[arg(long)]
        incremental: bool,
    },
    /// Analyse the hashed snapshot (exact duplicate sets).
    Analyze {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Discover similar/version-related non-identical contents (M0.3).
    Similarity {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Minimum exact weighted Jaccard score in [0, 1].
        #[arg(long, default_value_t = 0.50)]
        threshold: f64,
        /// Minimum number of shared chunks required for a relationship.
        #[arg(long, default_value_t = 2)]
        min_shared_chunks: u32,
        /// Minimum number of shared bytes required for a relationship.
        #[arg(long, default_value_t = 32 * 1024)]
        min_shared_bytes: u64,
        /// Deterministic maximum number of candidate pairs to evaluate.
        #[arg(long, default_value_t = 200_000)]
        max_candidates: u64,
    },
    /// Perceptual image/audio/video evidence for human review (M0.5).
    Media {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Absolute path to an FFmpeg executable (audio/video). Without it,
        /// audio and video produce explicit failure evidence.
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
        /// Absolute path to the isolated image worker. Defaults to the
        /// `df-media-worker` next to this executable, if present.
        #[arg(long)]
        image_worker: Option<PathBuf>,
        /// Deterministic maximum number of pairwise comparisons.
        #[arg(long, default_value_t = 100_000)]
        max_pairs: u64,
    },
    /// Extract, index, search and query document content (M0.4).
    Content {
        #[command(subcommand)]
        command: ContentCommand,
    },
    /// Signed, sandboxed suggestion plugins (M0.6).
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// Assisted intelligence: explanations and labels, never actions (M0.7).
    Ai {
        #[command(subcommand)]
        command: AiCommand,
    },
    /// Manage reconstruction plans.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// Execute the approved plan (verified copy). Resumable.
    Execute {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Acknowledge a destination filesystem without physical identity
        /// guarantees (network shares, FAT variants) — ADR-0036.
        #[arg(long)]
        allow_degraded_destination: bool,
    },
    /// Verify the executed plan from primary evidence.
    Verify {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Evidence reports derived from the inventory.
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Review ambiguous structural findings before creating a plan.
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// Audit ledger operations.
    Audit {
        #[command(subcommand)]
        command: AuditCommand,
    },
}

#[derive(Subcommand)]
enum ContentCommand {
    /// Extract the latest analysed snapshot. Safe to rerun after interruption.
    Extract {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Absolute isolated PDF-worker path. Otherwise only a sibling
        /// `df-extract-worker` sidecar is considered; PATH is never searched.
        #[arg(long)]
        pdf_worker: Option<PathBuf>,
        /// Unique contents loaded per resumable database page.
        #[arg(long, default_value_t = 64)]
        page_size: u32,
        /// Maximum source prefix retained in memory for one document.
        #[arg(long, default_value_t = 64 * 1024 * 1024)]
        max_input_bytes: u64,
        /// Maximum normalized characters retained for one text subject.
        #[arg(long, default_value_t = 4_000_000)]
        max_text_chars: u64,
        /// Maximum normalized characters retained across one container.
        #[arg(long, default_value_t = 8_000_000)]
        max_total_text_chars: u64,
        /// Unicode characters per transactional SQLite text segment.
        #[arg(long, default_value_t = 64_000)]
        text_segment_chars: u64,
        /// Maximum decoded bytes for one EML attachment.
        #[arg(long, default_value_t = 16 * 1024 * 1024)]
        max_attachment_bytes: u64,
        /// Maximum decoded attachment bytes across one EML.
        #[arg(long, default_value_t = 64 * 1024 * 1024)]
        max_total_attachment_bytes: u64,
        /// Maximum number of virtual ZIP entries.
        #[arg(long, default_value_t = 10_000)]
        max_archive_entries: u64,
        /// Maximum expanded bytes for one ZIP entry.
        #[arg(long, default_value_t = 16 * 1024 * 1024)]
        max_archive_entry_bytes: u64,
        /// Maximum expanded bytes across one ZIP.
        #[arg(long, default_value_t = 128 * 1024 * 1024)]
        max_archive_total_bytes: u64,
        /// Maximum expanded/compressed ZIP ratio.
        #[arg(long, default_value_t = 100)]
        max_archive_compression_ratio: u64,
        /// Maximum nested archive depth.
        #[arg(long, default_value_t = 4)]
        max_archive_nesting_depth: u64,
    },
    /// Explicitly seal an unrecoverable extraction run as failed.
    Fail {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Extraction run identifier.
        #[arg(long)]
        run: String,
        /// Audited reason this run cannot be resumed.
        #[arg(long)]
        reason: String,
    },
    /// Rebuild immutable Tantivy and Parquet artifacts from SQLite evidence.
    Build {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Completed extraction run identifier (default: latest for the
        /// latest snapshot).
        #[arg(long)]
        run: Option<String>,
        /// SQLite subjects loaded per search-index page.
        #[arg(long, default_value_t = 512)]
        search_page_size: u32,
        /// Tantivy writer memory budget in bytes.
        #[arg(long, default_value_t = 50_000_000)]
        writer_memory_bytes: usize,
        /// SQLite subjects loaded per Parquet page.
        #[arg(long, default_value_t = 2_048)]
        analytical_page_size: u32,
        /// Parquet Zstandard compression level.
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
    },
    /// Search the newest verified Tantivy artifact for an extraction run.
    Search {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Completed extraction run identifier (default: latest for the
        /// latest snapshot).
        #[arg(long)]
        run: Option<String>,
        /// Tantivy query text.
        #[arg(long)]
        query: String,
        /// Maximum hits (1-100).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Result offset (maximum 10000).
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Plain-text snippet character budget.
        #[arg(long, default_value_t = 240)]
        snippet_chars: usize,
    },
    /// Run one bounded read-only SQL query against registered Parquet evidence.
    Query {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Absolute isolated SQL-worker path. Otherwise only a sibling
        /// `df-query-worker` sidecar is considered; PATH is never searched.
        #[arg(long)]
        query_worker: Option<PathBuf>,
        /// Completed extraction run identifier (default: latest for the
        /// latest snapshot).
        #[arg(long)]
        run: Option<String>,
        /// Read-only SQL; the sole registered table is `content`.
        #[arg(long)]
        sql: String,
        /// Maximum rows materialized.
        #[arg(long, default_value_t = 1_000)]
        max_rows: usize,
        /// Maximum total UTF-8 bytes returned.
        #[arg(long, default_value_t = 8 * 1024 * 1024)]
        max_result_bytes: usize,
        /// Maximum characters in one result cell.
        #[arg(long, default_value_t = 65_536)]
        max_cell_chars: usize,
        /// DataFusion memory budget in bytes; disk spill is disabled.
        #[arg(long, default_value_t = 256 * 1024 * 1024)]
        memory_limit_bytes: usize,
        /// Wall-clock query timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Create a new project directory with its SQLite state.
    Create {
        /// Human name of the project.
        #[arg(long)]
        name: String,
        /// Directory that will hold the project (must be new or empty).
        #[arg(long)]
        path: PathBuf,
        /// Where verified copies will be produced in later milestones.
        /// Must not overlap the project directory or any source root.
        #[arg(long)]
        output_root: PathBuf,
        /// Where audit material will be written (default: <path>/audit).
        #[arg(long)]
        audit_root: Option<PathBuf>,
        /// Origin directory to register (repeatable). Origins are read-only.
        #[arg(long = "source")]
        source: Vec<PathBuf>,
        /// Profile name.
        #[arg(long, default_value = "generic")]
        profile: String,
    },
    /// Show the state, roots, ledger summary and integrity of a project.
    Status {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Generate a full-coverage plan for the analysed snapshot.
    Create {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// What to do with exact duplicates (RFC-0001 §15.4). The default
        /// copies every occurrence. No policy ever consolidates a copy that
        /// lives in a protected context.
        #[arg(long, value_name = "POLICY", default_value = "REPORT_ONLY")]
        duplicate_policy: String,
    },
    /// Re-check the plan invariants (destinations, collisions, coverage).
    Validate {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Approve and freeze the plan (canonical SHA-256).
    Approve {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ReportCommand {
    /// Exact duplicates (same size, same SHA-256) of the latest snapshot.
    Duplicates {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Exact tree clones (folders with byte-for-byte identical subtrees).
    TreeClones {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Folders that share content without being identical (partial clones).
    TreeRelations {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Generic low-value folders (Downloads, Backup, copies, …) and penalties.
    Contexts {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Structural anomalies with the evidence behind each finding.
    Anomalies {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Version-like relationships backed by exact shared-chunk evidence.
    Similarities {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Perceptual media review relations of the latest sealed run.
    Media {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Findings and suggestions of the latest sealed plugin runs.
    Plugins {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum PluginCommand {
    /// Verify a signed package (signature, hash, ABI, compile) and store it.
    Register {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// JSON package file: manifest, component SHA-256, publisher key and
        /// Ed25519 signature.
        #[arg(long)]
        package: PathBuf,
        /// The WebAssembly component file the package signs.
        #[arg(long)]
        component: PathBuf,
    },
    /// Stored registrations of this project.
    List {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Execute every registered plugin over the analysed snapshot. Subject
    /// metadata is granted by default; text requires --grant-text.
    Run {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Deterministic maximum number of subjects offered to each plugin.
        #[arg(long, default_value_t = 10_000)]
        max_subjects: u64,
        /// Additionally grant plugins access to normalized subject text.
        #[arg(long)]
        grant_text: bool,
    },
}

#[derive(Subcommand)]
enum AiCommand {
    /// Manage BYOK API keys in the OS credential vault.
    Key {
        #[command(subcommand)]
        command: AiKeyCommand,
    },
    /// Explain one pending review item. Without --accept-disclosure this
    /// only previews the exact disclosure and sends nothing anywhere.
    Explain {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Review item id (see `review list`).
        #[arg(long)]
        item: String,
        /// Cloud provider (`anthropic` or `openai`); mutually exclusive with
        /// --local-exe.
        #[arg(long, conflicts_with = "local_exe")]
        provider: Option<String>,
        /// Model identifier for the chosen provider.
        #[arg(long)]
        model: String,
        /// Absolute path to an air-gapped local model executable.
        #[arg(long)]
        local_exe: Option<PathBuf>,
        /// The disclosure digest previously shown; consent for exactly that
        /// disclosure.
        #[arg(long)]
        accept_disclosure: Option<String>,
    },
    /// Immutable audit trail of assistance invocations.
    Audits {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

#[derive(Subcommand)]
enum AiKeyCommand {
    /// Read the key from stdin (never from arguments) and store it.
    Set {
        /// `anthropic` or `openai`.
        #[arg(long)]
        provider: String,
    },
    /// Remove the stored key.
    Remove {
        /// `anthropic` or `openai`.
        #[arg(long)]
        provider: String,
    },
    /// Show which providers have a stored key (values are never shown).
    List,
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// List pending and decided structural review items.
    List {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
    /// Append a human decision before generating the plan.
    Decide {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
        /// Review item identifier shown by `review list`.
        #[arg(long)]
        item: String,
        /// Safe copy action: COPY_ACTIVE, COPY_REVIEW, COPY_SEPARATED or
        /// COPY_TEMPORARY. Destructive actions do not exist.
        #[arg(long)]
        decision: String,
        /// Human explanation preserved in the append-only ledger.
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum AuditCommand {
    /// Cryptographically verify the audit ledger chain.
    Verify {
        /// Project directory.
        #[arg(long)]
        path: PathBuf,
    },
}

/// Everything the CLI can print, one variant per command family.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Output {
    Status(Box<ProjectStatus>),
    Scan(ScanOutcome),
    Hash(HashOutcome),
    Analyze(AnalyzeOutcome),
    Similarity(SimilarityOutcome),
    Media(MediaOutcome),
    ContentExtraction(ContentExtractionOutcome),
    ContentArtifacts(ContentArtifactBuildOutcome),
    ContentSearch(ContentSearchOutcome),
    ContentQuery(ContentQueryOutcome),
    Plan(PlanOutcome),
    PlanValidation(PlanValidationReport),
    Approve(ApproveOutcome),
    Execute(ExecuteOutcome),
    Verify(VerifyOutcome),
    Duplicates(DuplicateReport),
    TreeClones(TreeCloneReport),
    TreeRelations(TreeRelationReport),
    Contexts(ContextReport),
    Anomalies(AnomalyReport),
    Similarities(SimilarityReport),
    MediaRelations(MediaReport),
    PluginRegistered(Box<RegisteredPluginMetadata>),
    PluginList(Vec<PluginRegistrationView>),
    PluginRuns(PluginsOutcome),
    PluginFindings(PluginReport),
    AiKeys(Vec<(String, bool)>),
    AiKeyChanged(String),
    AiAssist(Box<AiAssistOutcome>),
    AiAudits(Vec<AssistanceAuditView>),
    Review(ReviewQueue),
    Audit(AuditReport),
}

fn run(cli: &Cli) -> DfResult<Output> {
    match &cli.command {
        Command::Project { command } => match command {
            ProjectCommand::Create {
                name,
                path,
                output_root,
                audit_root,
                source,
                profile,
            } => {
                let request = CreateProjectRequest {
                    name: name.clone(),
                    project_dir: path.clone(),
                    output_root: output_root.clone(),
                    audit_root: audit_root.clone(),
                    source_roots: source.clone(),
                    profile: Some(profile.clone()),
                };
                df_facade::create_project(&request, Actor::Cli)
                    .map(Box::new)
                    .map(Output::Status)
            }
            ProjectCommand::Status { path } => df_facade::project_status(path)
                .map(Box::new)
                .map(Output::Status),
        },
        Command::Scan { path } => df_facade::scan_project(path, Actor::Cli).map(Output::Scan),
        Command::Hash { path, incremental } => df_facade::hash_project_with_options(
            path,
            Actor::Cli,
            &df_facade::HashOptions {
                incremental: *incremental,
                ..df_facade::HashOptions::default()
            },
        )
        .map(Output::Hash),
        Command::Analyze { path } => {
            df_facade::analyze_project(path, Actor::Cli).map(Output::Analyze)
        }
        Command::Similarity {
            path,
            threshold,
            min_shared_chunks,
            min_shared_bytes,
            max_candidates,
        } => df_facade::analyze_similarity_with_options(
            path,
            Actor::Cli,
            &SimilarityOptions {
                threshold: *threshold,
                min_shared_chunks: *min_shared_chunks,
                min_shared_bytes: *min_shared_bytes,
                max_candidates: *max_candidates,
                ..SimilarityOptions::default()
            },
        )
        .map(Output::Similarity),
        Command::Media {
            path,
            ffmpeg,
            image_worker,
            max_pairs,
        } => {
            let mut sidecars = MediaSidecars::none();
            if let Some(worker) = image_worker
                .clone()
                .or_else(df_facade::default_media_worker)
            {
                sidecars = sidecars.with_image_worker(worker);
            }
            if let Some(ffmpeg) = ffmpeg {
                sidecars = sidecars.with_ffmpeg(ffmpeg);
            }
            df_facade::analyze_media_with_options(
                path,
                Actor::Cli,
                &MediaProjectOptions {
                    max_pairs: *max_pairs,
                    sidecars,
                    ..MediaProjectOptions::default()
                },
            )
            .map(Output::Media)
        }
        Command::Plugin { command } => match command {
            PluginCommand::Register {
                path,
                package,
                component,
            } => df_facade::register_plugin(path, package, component, Actor::Cli)
                .map(Box::new)
                .map(Output::PluginRegistered),
            PluginCommand::List { path } => df_facade::list_plugins(path).map(Output::PluginList),
            PluginCommand::Run {
                path,
                max_subjects,
                grant_text,
            } => {
                let mut options = df_facade::default_plugin_options();
                options.max_subjects = *max_subjects;
                if *grant_text {
                    options
                        .policy
                        .granted_capabilities
                        .insert(df_facade::PluginCapability::SubjectText);
                }
                df_facade::run_plugins_with_options(path, Actor::Cli, &options)
                    .map(Output::PluginRuns)
            }
        },
        Command::Ai { command } => match command {
            AiCommand::Key { command } => match command {
                AiKeyCommand::Set { provider } => {
                    let provider = df_facade::AiKeyProvider::parse(provider)?;
                    eprintln!("Pega la API key y pulsa Enter (nunca va en argumentos):");
                    let mut key = String::new();
                    std::io::stdin()
                        .read_line(&mut key)
                        .map_err(|error| DfError::Validation(error.to_string()))?;
                    df_facade::set_ai_key(provider, &key)?;
                    Ok(Output::AiKeyChanged(format!(
                        "stored key for {}",
                        provider.as_str()
                    )))
                }
                AiKeyCommand::Remove { provider } => {
                    let provider = df_facade::AiKeyProvider::parse(provider)?;
                    df_facade::remove_ai_key(provider)?;
                    Ok(Output::AiKeyChanged(format!(
                        "removed key for {}",
                        provider.as_str()
                    )))
                }
                AiKeyCommand::List => {
                    let mut rows = Vec::new();
                    for provider in [
                        df_facade::AiKeyProvider::Anthropic,
                        df_facade::AiKeyProvider::OpenAi,
                    ] {
                        rows.push((
                            provider.as_str().to_string(),
                            df_facade::ai_key_present(provider)?,
                        ));
                    }
                    Ok(Output::AiKeys(rows))
                }
            },
            AiCommand::Explain {
                path,
                item,
                provider,
                model,
                local_exe,
                accept_disclosure,
            } => {
                let choice = match (provider, local_exe) {
                    (Some(provider), None) => df_facade::AiProviderChoice::Cloud {
                        provider: df_facade::AiKeyProvider::parse(provider)?,
                        model: model.clone(),
                    },
                    (None, Some(executable)) => df_facade::AiProviderChoice::LocalProcess {
                        executable: executable.clone(),
                        model: model.clone(),
                    },
                    _ => {
                        return Err(DfError::Validation(
                            "choose exactly one of --provider or --local-exe".to_string(),
                        ))
                    }
                };
                df_facade::ai_explain_review(
                    path,
                    item,
                    &choice,
                    accept_disclosure.as_deref(),
                    Actor::Cli,
                )
                .map(Box::new)
                .map(Output::AiAssist)
            }
            AiCommand::Audits { path, limit } => {
                df_facade::ai_audit_report(path, *limit).map(Output::AiAudits)
            }
        },
        Command::Content { command } => match command {
            ContentCommand::Extract {
                path,
                pdf_worker,
                page_size,
                max_input_bytes,
                max_text_chars,
                max_total_text_chars,
                text_segment_chars,
                max_attachment_bytes,
                max_total_attachment_bytes,
                max_archive_entries,
                max_archive_entry_bytes,
                max_archive_total_bytes,
                max_archive_compression_ratio,
                max_archive_nesting_depth,
            } => {
                let limits = ExtractionLimits {
                    max_input_bytes: *max_input_bytes,
                    max_text_chars: *max_text_chars,
                    max_total_text_chars: *max_total_text_chars,
                    text_segment_chars: *text_segment_chars,
                    max_attachment_bytes: *max_attachment_bytes,
                    max_total_attachment_bytes: *max_total_attachment_bytes,
                    max_archive_entries: *max_archive_entries,
                    max_archive_entry_bytes: *max_archive_entry_bytes,
                    max_archive_total_bytes: *max_archive_total_bytes,
                    max_archive_compression_ratio: *max_archive_compression_ratio,
                    max_archive_nesting_depth: *max_archive_nesting_depth,
                    ..ExtractionLimits::default()
                };
                df_facade::extract_project_content(
                    path,
                    Actor::Cli,
                    &ContentExtractionOptions {
                        limits,
                        page_size: *page_size,
                        pdf_worker: pdf_worker.clone(),
                    },
                )
                .map(Output::ContentExtraction)
            }
            ContentCommand::Fail { path, run, reason } => {
                df_facade::fail_content_extraction(path, run, reason, Actor::Cli)
                    .map(Output::ContentExtraction)
            }
            ContentCommand::Build {
                path,
                run,
                search_page_size,
                writer_memory_bytes,
                analytical_page_size,
                zstd_level,
            } => df_facade::build_content_artifacts(
                path,
                run.as_deref(),
                SearchBuildOptions {
                    page_size: *search_page_size,
                    writer_memory_bytes: *writer_memory_bytes,
                },
                SnapshotBuildOptions {
                    page_size: *analytical_page_size,
                    zstd_level: *zstd_level,
                },
                Actor::Cli,
            )
            .map(Output::ContentArtifacts),
            ContentCommand::Search {
                path,
                run,
                query,
                limit,
                offset,
                snippet_chars,
            } => df_facade::search_project_content(
                path,
                run.as_deref(),
                &SearchRequest {
                    query: query.clone(),
                    limit: *limit,
                    offset: *offset,
                    snippet_chars: *snippet_chars,
                },
            )
            .map(Output::ContentSearch),
            ContentCommand::Query {
                path,
                query_worker,
                run,
                sql,
                max_rows,
                max_result_bytes,
                max_cell_chars,
                memory_limit_bytes,
                timeout_seconds,
            } => df_facade::query_project_content_with_worker(
                path,
                run.as_deref(),
                sql,
                QueryOptions {
                    max_rows: *max_rows,
                    max_result_bytes: *max_result_bytes,
                    max_cell_chars: *max_cell_chars,
                    memory_limit_bytes: *memory_limit_bytes,
                    timeout_seconds: *timeout_seconds,
                },
                query_worker.as_deref(),
            )
            .map(Output::ContentQuery),
        },
        Command::Plan { command } => match command {
            PlanCommand::Create {
                path,
                duplicate_policy,
            } => {
                let policy = df_facade::DuplicatePolicy::parse(duplicate_policy)?;
                df_facade::create_plan(path, Actor::Cli, policy).map(Output::Plan)
            }
            PlanCommand::Validate { path } => {
                df_facade::validate_plan(path).map(Output::PlanValidation)
            }
            PlanCommand::Approve { path } => {
                df_facade::approve_plan(path, Actor::Cli).map(Output::Approve)
            }
        },
        Command::Execute {
            path,
            allow_degraded_destination,
        } => df_facade::execute_plan_with_options(
            path,
            Actor::Cli,
            &df_facade::ExecuteOptions {
                allow_degraded_destination: *allow_degraded_destination,
                ..df_facade::ExecuteOptions::default()
            },
        )
        .map(Output::Execute),
        Command::Verify { path } => {
            df_facade::verify_project_output(path, Actor::Cli).map(Output::Verify)
        }
        Command::Report { command } => match command {
            ReportCommand::Duplicates { path } => {
                df_facade::duplicate_report(path).map(Output::Duplicates)
            }
            ReportCommand::TreeClones { path } => {
                df_facade::tree_clone_report(path).map(Output::TreeClones)
            }
            ReportCommand::TreeRelations { path } => {
                df_facade::tree_relation_report(path).map(Output::TreeRelations)
            }
            ReportCommand::Contexts { path } => {
                df_facade::context_report(path).map(Output::Contexts)
            }
            ReportCommand::Anomalies { path } => {
                df_facade::structural_anomaly_report(path).map(Output::Anomalies)
            }
            ReportCommand::Similarities { path } => {
                df_facade::similarity_report(path).map(Output::Similarities)
            }
            ReportCommand::Media { path } => {
                df_facade::media_report(path).map(Output::MediaRelations)
            }
            ReportCommand::Plugins { path } => {
                df_facade::plugin_report(path).map(Output::PluginFindings)
            }
        },
        Command::Review { command } => match command {
            ReviewCommand::List { path } => {
                df_facade::structural_review_queue(path).map(Output::Review)
            }
            ReviewCommand::Decide {
                path,
                item,
                decision,
                reason,
            } => {
                let decision = df_facade::RuleAction::parse(decision)?;
                df_facade::decide_structural_review(path, item, decision, reason, Actor::Cli)
                    .map(Output::Review)
            }
        },
        Command::Audit { command } => match command {
            AuditCommand::Verify { path } => df_facade::verify_audit(path).map(Output::Audit),
        },
    }
}

fn print_status(status: &ProjectStatus) {
    println!("Project   : {} ({})", status.name, status.project_id);
    println!("State     : {}", status.state);
    println!("Profile   : {}", status.profile);
    println!("Created   : {}", status.created_at);
    println!("Updated   : {}", status.updated_at);
    println!("Directory : {}", status.project_dir);
    println!("Output    : {}", status.output_root);
    println!("Audit     : {}", status.audit_root);
    if status.source_roots.is_empty() {
        println!("Sources   : (none registered)");
    } else {
        println!("Sources   :");
        for root in &status.source_roots {
            println!("  - {} [{}] read-only", root.absolute_path, root.filesystem);
        }
    }
    if let (Some(snapshot), Some(inv)) = (&status.latest_snapshot_id, &status.inventory) {
        println!("Snapshot  : {snapshot}");
        println!(
            "Inventory : {} file(s), {} folder(s), {} byte(s), {} error(s), {} reparse point(s)",
            inv.files, inv.folders, inv.bytes, inv.scan_errors, inv.reparse_points
        );
        println!(
            "Hashing   : {} done, {} pending, {} failed, {} source-changed",
            inv.hash_done, inv.hash_pending, inv.hash_failed, inv.hash_source_changed
        );
    }
    match &status.last_event {
        Some(event) => println!(
            "Ledger    : {} event(s), last {} at {} by {}",
            status.event_count, event.event_type, event.timestamp, event.actor
        ),
        None => println!("Ledger    : empty"),
    }
    if let Some(integrity) = &status.integrity {
        if integrity.is_ok() {
            println!("Integrity : OK (database, foreign keys, migrations, ledger)");
        } else {
            println!("Integrity : FAILED");
            for problem in &integrity.problems {
                println!("  ! {problem}");
            }
        }
    }
    if let Some(diagnostic) = &status.structural_diagnostics {
        println!(
            "Structural: {} | {} signature(s), {} exact clone set(s), {} partial, {} embedded, {} repeated component(s)",
            if diagnostic.analysis_complete {
                "complete"
            } else {
                "not analysed"
            },
            diagnostic.folder_signatures,
            diagnostic.exact_tree_clone_sets,
            diagnostic.partial_tree_clones,
            diagnostic.embedded_trees,
            diagnostic.repeated_components
        );
        println!(
            "Diagnostic: {} protected, {} generic, {} rule match(es), {} anomaly/anomalies ({} high), {} pending review",
            diagnostic.protected_boundaries,
            diagnostic.generic_folders,
            diagnostic.rule_matches,
            diagnostic.anomalies,
            diagnostic.high_anomalies,
            diagnostic.pending_review
        );
        if diagnostic.candidate_cap_reached {
            println!(
                "Relation cap: REACHED — structural relations are conservative but not exhaustive"
            );
        }
    }
}

fn print_scan(outcome: &ScanOutcome) {
    println!("Snapshot  : {}", outcome.snapshot_id);
    println!("Scan run  : {}", outcome.scan_run_id);
    println!("Files     : {}", outcome.files);
    println!("Folders   : {}", outcome.folders);
    println!("Bytes     : {}", outcome.bytes);
    println!("Errors    : {}", outcome.errors);
    println!(
        "Reparse   : {} (recorded, not followed)",
        outcome.reparse_points
    );
    println!("State     : {}", outcome.state);
    if outcome.cancelled {
        println!("Cancelled : scan stopped early; run `dataforge scan` again to restart");
    }
}

fn print_hash(outcome: &HashOutcome) {
    println!("Snapshot        : {}", outcome.snapshot_id);
    println!("Hashed          : {}", outcome.hashed);
    if outcome.reused > 0 {
        println!(
            "Reused          : {} binding(s) carried from the previous snapshot",
            outcome.reused
        );
    }
    println!("Failed          : {}", outcome.failed);
    println!("Source changed  : {}", outcome.source_changed);
    println!("Pending         : {}", outcome.pending);
    println!("State           : {}", outcome.state);
    if outcome.cancelled {
        println!("Cancelled       : run `dataforge hash` again to resume");
    }
}

fn print_analyze(outcome: &AnalyzeOutcome) {
    println!("Snapshot         : {}", outcome.snapshot_id);
    println!("Duplicate sets   : {}", outcome.duplicate_sets);
    println!("Folder signatures: {}", outcome.folder_signatures);
    println!("Tree clone sets  : {}", outcome.tree_clone_sets);
    println!("Partial clones   : {}", outcome.partial_tree_clones);
    println!("Embedded trees   : {}", outcome.embedded_trees);
    println!("Repeated parts   : {}", outcome.repeated_components);
    println!(
        "Relation cap      : {}",
        if outcome.candidate_cap_reached {
            "REACHED (some distinct candidates were not generated)"
        } else {
            "not reached"
        }
    );
    println!("Generic folders  : {}", outcome.generic_folders);
    println!("Protected bounds : {}", outcome.protected_boundaries);
    println!("Representatives  : {}", outcome.duplicate_representatives);
    println!("Rule matches     : {}", outcome.rule_matches);
    println!(
        "Anomalies        : {} ({} high)",
        outcome.anomalies, outcome.high_anomalies
    );
    println!("Review items     : {}", outcome.review_items);
    println!("State            : {}", outcome.state);
}

fn print_similarity(outcome: &SimilarityOutcome) {
    println!("Similarity run   : {}", outcome.run_id);
    println!("Snapshot         : {}", outcome.snapshot_id);
    println!("Status           : {}", outcome.status);
    println!("Algorithm        : {}", outcome.algorithm_version);
    println!("Config SHA-256   : {}", outcome.config_digest);
    if let Some(options) = outcome.config.get("options") {
        println!(
            "Threshold        : {}",
            options
                .get("threshold")
                .and_then(serde_json::Value::as_f64)
                .map_or_else(|| "unknown".to_string(), |value| format!("{value:.4}"))
        );
    }
    println!("Contents         : {}", outcome.counters.contents_total);
    println!("Chunked          : {}", outcome.counters.contents_chunked);
    println!("Skipped          : {}", outcome.counters.contents_skipped);
    println!("Chunks           : {}", outcome.counters.chunks_total);
    println!("Candidates       : {}", outcome.counters.candidates_total);
    println!("Relationships    : {}", outcome.counters.relations_total);
    println!(
        "Candidate cap    : {}",
        if outcome.candidate_cap_reached {
            "REACHED — results are conservative but not exhaustive"
        } else {
            "not reached"
        }
    );
    if outcome.cancelled {
        println!("Cancelled        : run `dataforge similarity` again to resume");
    }
}

fn print_content_extraction(outcome: &ContentExtractionOutcome) {
    println!("Extraction run : {}", outcome.run_id);
    println!("Snapshot       : {}", outcome.snapshot_id);
    println!("Status         : {}", outcome.status);
    println!("Extractor      : {}", outcome.extractor_version);
    println!("Config SHA-256 : {}", outcome.config_digest);
    println!("Contents       : {}", outcome.counters.contents_total);
    println!("Extracted      : {}", outcome.counters.extracted);
    println!("Unsupported    : {}", outcome.counters.unsupported);
    println!("Limited        : {}", outcome.counters.limited);
    println!("Failed content : {}", outcome.counters.failed);
    println!("Text subjects  : {}", outcome.counters.text_subjects);
    println!("Text segments  : {}", outcome.counters.text_segments);
    println!("Mail messages  : {}", outcome.counters.mail_messages);
    println!("Mail threads   : {}", outcome.counters.mail_threads);
    println!("Attachments    : {}", outcome.counters.mail_attachments);
    println!("Archive entries: {}", outcome.counters.archive_entries);
    println!(
        "This invocation: {} extracted, {} reused, {} thread(s) built",
        outcome.processed_this_invocation,
        outcome.reused_this_invocation,
        outcome.threads_built_this_invocation
    );
    if let Some(error) = &outcome.error {
        println!("Error          : {error}");
    }
    if outcome.counters.limited > 0 || outcome.counters.failed > 0 {
        println!(
            "Result         : PARTIAL — inspect LIMITED/FAILED evidence; this command exits 3"
        );
    }
}

fn print_content_artifacts(outcome: &ContentArtifactBuildOutcome) {
    println!("Extraction run : {}", outcome.run_id);
    println!("Search index   : {}", outcome.search_index.id);
    println!("Search schema  : {}", outcome.search_index.schema_version);
    println!("Documents      : {}", outcome.search_index.documents);
    println!("Index path     : {}", outcome.search_index.relative_path);
    println!("Index digest   : {}", outcome.search_index.content_digest);
    println!("Parquet        : {}", outcome.analytical_snapshot.id);
    println!(
        "Parquet schema : {}",
        outcome.analytical_snapshot.schema_version
    );
    println!("Rows           : {}", outcome.analytical_snapshot.rows);
    println!(
        "Parquet path   : {}",
        outcome.analytical_snapshot.relative_path
    );
    println!("Parquet SHA-256: {}", outcome.analytical_snapshot.sha256);
}

fn print_content_search(outcome: &ContentSearchOutcome) {
    println!("Extraction run : {}", outcome.run_id);
    println!("Search index   : {}", outcome.index.id);
    println!("Query          : {}", outcome.query);
    println!("Hits           : {}", outcome.hits.len());
    for hit in &outcome.hits {
        println!();
        println!("  {:.4} — {}", hit.score, hit.representative_path);
        if let Some(virtual_path) = &hit.virtual_path {
            println!("    virtual : {virtual_path}");
        }
        println!("    subject : {}", hit.subject);
        println!("    context : {} | {}", hit.context, hit.mime);
        if !hit.snippet.is_empty() {
            println!("    snippet : {}", hit.snippet.replace(['\r', '\n'], " "));
        }
    }
}

fn print_content_query(outcome: &ContentQueryOutcome) {
    println!("Extraction run : {}", outcome.run_id);
    println!("Parquet        : {}", outcome.snapshot.id);
    println!("Rows           : {}", outcome.result.rows.len());
    if !outcome.result.columns.is_empty() {
        println!(
            "{}",
            outcome
                .result
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>()
                .join("\t")
        );
    }
    for row in &outcome.result.rows {
        println!(
            "{}",
            row.iter()
                .map(|value| value
                    .as_deref()
                    .unwrap_or("NULL")
                    .replace(['\r', '\n'], " "))
                .collect::<Vec<_>>()
                .join("\t")
        );
    }
}

fn print_plan(outcome: &PlanOutcome) {
    println!("Plan        : {} (v{})", outcome.plan_id, outcome.version);
    println!("Snapshot    : {}", outcome.snapshot_id);
    println!("Operations  : {}", outcome.operations);
    println!("  copies      : {}", outcome.copies);
    println!("    review    : {}", outcome.review_copies);
    println!("    separated : {}", outcome.separated_copies);
    println!("    temporary : {}", outcome.temporary_copies);
    println!("  directories : {}", outcome.directories);
    println!("  no action   : {}", outcome.no_action);
    println!("  blocked     : {}", outcome.blocked);
    println!("State       : {}", outcome.state);
    println!("Next        : review it, then `dataforge plan approve`");
}

fn print_plan_validation(report: &PlanValidationReport) {
    println!("Plan       : {} (v{})", report.plan_id, report.version);
    println!("Status     : {}", report.status);
    println!("Operations : {}", report.operations);
    if report.ok {
        println!("Validation : OK (destinations, collisions, coverage)");
    } else {
        println!("Validation : FAILED");
        for problem in &report.problems {
            println!("  ! {problem}");
        }
    }
}

fn print_approve(outcome: &ApproveOutcome) {
    println!("Plan       : {} (v{})", outcome.plan_id, outcome.version);
    println!("Approved   : {} operation(s)", outcome.operations_approved);
    println!("Plan hash  : {}", outcome.serialized_sha256);
    println!("State      : {}", outcome.state);
}

fn print_execute(outcome: &ExecuteOutcome) {
    println!("Plan            : {}", outcome.plan_id);
    println!("Completed       : {}", outcome.completed);
    println!("Failed final    : {}", outcome.failed_final);
    println!("Failed retryable: {}", outcome.failed_retryable);
    println!("Pending         : {}", outcome.pending);
    println!("Bytes copied    : {}", outcome.bytes_copied);
    println!("State           : {}", outcome.state);
    if outcome.cancelled || outcome.pending > 0 || outcome.failed_retryable > 0 {
        println!("Next            : run `dataforge execute` again to resume");
    }
}

fn print_verify(outcome: &VerifyOutcome) {
    println!("Verification : {}", outcome.verification_run_id);
    println!("Plan         : {}", outcome.plan_id);
    println!("Checked      : {} artefact(s)", outcome.checked);
    println!("Verdict      : {}", outcome.verdict);
    println!("State        : {}", outcome.state);
    if !outcome.findings.is_empty() {
        println!("Findings     :");
        for finding in &outcome.findings {
            println!(
                "  [{}] {} — {}: {}",
                finding.severity, finding.kind, finding.subject, finding.detail
            );
        }
    }
}

fn print_duplicates(report: &DuplicateReport) {
    println!("Snapshot        : {}", report.snapshot_id);
    println!("Duplicate sets  : {}", report.sets.len());
    println!("Redundant files : {}", report.redundant_files);
    println!("Redundant bytes : {}", report.redundant_bytes);
    for set in &report.sets {
        println!();
        println!("  sha256 {} ({} bytes)", set.sha256, set.size_bytes);
        for path in &set.occurrences {
            // The representative is the best canonical location, not a
            // verdict that the others are dispensable (RFC-0001 §15.5).
            let mark = if set.representative.as_deref() == Some(path.as_str()) {
                "*"
            } else {
                " "
            };
            println!("    {mark} {path}");
        }
        if let Some(reason) = &set.representative_reason {
            println!("      * representative: {reason}");
        }
    }
    if report.sets.is_empty() {
        println!("No exact duplicates found.");
    }
}

fn print_tree_relations(report: &TreeRelationReport) {
    println!("Snapshot        : {}", report.snapshot_id);
    println!("Partial clones  : {}", report.partial_clones);
    println!("Embedded trees  : {}", report.embedded);
    println!("Repeated parts  : {}", report.repeated_components);
    for relation in &report.relations {
        println!();
        println!(
            "  {} — {:.0}% shared ({} file(s), {} bytes)",
            relation.relationship,
            relation.similarity * 100.0,
            relation.shared_files,
            relation.shared_bytes
        );
        println!("    A: {}", relation.path_a);
        println!("    B: {}", relation.path_b);
        if relation.relationship == "REPEATED_COMPONENT_ONLY" {
            println!(
                "    Repeated-content evidence only; these branches are not treated as clones"
            );
            println!(
                "    Only in A: {} file(s) | Only in B: {} file(s)",
                relation.unique_a_files, relation.unique_b_files
            );
        } else {
            match relation.contained.as_deref() {
                Some("A") => println!(
                    "    A is fully contained in B ({} file(s) only in B)",
                    relation.unique_b_files
                ),
                Some("B") => println!(
                    "    B is fully contained in A ({} file(s) only in A)",
                    relation.unique_a_files
                ),
                _ => println!(
                    "    Only in A: {} file(s) | Only in B: {} file(s) \
                 — dropping either side loses data (RFC-0001 §19.4)",
                    relation.unique_a_files, relation.unique_b_files
                ),
            }
        }
    }
    if report.relations.is_empty() {
        println!("No partial, embedded, or repeated-component tree relations found.");
    } else {
        println!();
        println!("Evidence only: nothing here is proposed for removal.");
    }
}

fn print_tree_clones(report: &TreeCloneReport) {
    println!("Snapshot        : {}", report.snapshot_id);
    println!("Clone sets      : {}", report.sets.len());
    println!("Cloned folders  : {}", report.cloned_folders);
    println!("Redundant bytes : {}", report.redundant_bytes);
    for set in &report.sets {
        println!();
        println!(
            "  {} — {} file(s), {} bytes",
            set.relationship.as_str(),
            set.subtree_files,
            set.subtree_bytes
        );
        for folder in &set.folders {
            println!("    - {folder}");
        }
    }
    if report.sets.is_empty() {
        println!("No exact tree clones found.");
    }
}

fn print_contexts(report: &ContextReport) {
    println!("Snapshot        : {}", report.snapshot_id);
    println!("Generic folders : {}", report.generic_folders.len());
    for folder in &report.generic_folders {
        let marker = folder.marker.as_deref().unwrap_or("generic");
        println!("  -{:<3} [{}] {}", folder.penalty, marker, folder.path);
    }
    if report.generic_folders.is_empty() {
        println!("No generic low-value folders found.");
    }
    println!("Protected bounds : {}", report.protected_folders.len());
    for folder in &report.protected_folders {
        println!(
            "  ! [{}] {} — {}",
            folder.marker, folder.path, folder.reason
        );
    }
}

fn print_anomalies(report: &AnomalyReport) {
    println!("Snapshot      : {}", report.snapshot_id);
    println!(
        "Anomalies     : {} high, {} warning(s), {} information",
        report.high, report.warnings, report.information
    );
    for anomaly in &report.anomalies {
        println!();
        println!(
            "  [{}] {} — {}",
            anomaly.severity, anomaly.kind, anomaly.summary
        );
        println!("    id: {}", anomaly.id);
        if anomaly.requires_review {
            println!("    review required");
        }
    }
    if report.anomalies.is_empty() {
        println!("No structural anomalies found.");
    }
}

fn print_ai_assist(outcome: &AiAssistOutcome) {
    let disclosure = &outcome.disclosure;
    println!("Purpose        : {}", disclosure.purpose);
    println!(
        "Provider       : {} / {} ({})",
        disclosure.provider, disclosure.model, disclosure.endpoint
    );
    println!(
        "Disclosure     : {} byte(s) visible, {} byte(s) on the wire",
        disclosure.visible_content_bytes, disclosure.transport_bytes
    );
    for field in &disclosure.fields {
        println!();
        println!(
            "  [{}] {} — {} byte(s), {} redaction(s)",
            field.evidence_id, field.field_name, field.visible_bytes, field.redactions
        );
        println!("    {}", field.visible_text);
    }
    println!();
    println!("Disclosure SHA-256: {}", disclosure.disclosure_sha256);
    if !outcome.executed {
        println!();
        println!(
            "Preview only — nothing was sent. To consent to exactly this \
             disclosure, repeat with --accept-disclosure {}",
            disclosure.disclosure_sha256
        );
        return;
    }
    println!(
        "Status         : {}",
        outcome.status.as_deref().unwrap_or("-")
    );
    if let Some(explanation) = &outcome.explanation {
        println!();
        println!("Explanation    : {explanation}");
    }
    for suggestion in &outcome.suggestions {
        println!();
        println!("  [{}] {}", suggestion.id, suggestion.label);
        println!("    {}", suggestion.explanation);
    }
    println!();
    println!("Evidence only: assisted intelligence cannot execute anything.");
}

fn print_ai_audits(audits: &[AssistanceAuditView]) {
    if audits.is_empty() {
        println!("No assistance invocations have been recorded.");
        return;
    }
    for audit in audits {
        println!(
            "{} — {} {} / {} — {}{}",
            audit.created_at,
            audit.purpose,
            audit.provider,
            audit.model,
            audit.status,
            audit
                .failure
                .as_deref()
                .map(|failure| format!(" ({failure})"))
                .unwrap_or_default()
        );
        println!("  disclosure {}", audit.disclosure_sha256);
    }
}

fn print_plugin_registered(metadata: &RegisteredPluginMetadata) {
    println!("Plugin           : {}", metadata.key);
    println!("Publisher        : {}", metadata.manifest.publisher);
    println!("Component SHA-256: {}", metadata.component_sha256);
    println!("Publisher key    : {}", metadata.publisher_public_key_hex);
    println!("Capabilities     : {:?}", metadata.manifest.capabilities);
    println!("Verified: signature, hash, manifest, ABI and full compile.");
}

fn print_plugin_list(plugins: &[PluginRegistrationView]) {
    if plugins.is_empty() {
        println!("No plugins are registered in this project.");
        return;
    }
    for plugin in plugins {
        println!("{}", plugin.plugin);
        println!("  component : {}", plugin.component_sha256);
        println!("  publisher : {}", plugin.publisher_public_key_hex);
    }
}

fn print_plugin_runs(outcome: &PluginsOutcome) {
    println!("Snapshot         : {}", outcome.snapshot_id);
    for run in &outcome.runs {
        println!();
        println!("  {} — {}", run.plugin, run.status);
        println!("    run       : {}", run.run_id);
        println!(
            "    subjects  : {} total, {} analysed, {} failed{}",
            run.subjects_total,
            run.subjects_analyzed,
            run.subjects_failed,
            if run.subject_cap_reached {
                " (cap REACHED — not exhaustive)"
            } else {
                ""
            }
        );
        println!("    findings  : {}", run.findings);
    }
    println!();
    println!("Evidence only    : findings suggest, they never execute.");
}

fn print_plugin_findings(report: &PluginReport) {
    println!("Snapshot : {}", report.snapshot_id);
    for run in &report.runs {
        println!(
            "  {} — {} finding(s) over {} subject(s)",
            run.plugin, run.findings_total, run.subjects_total
        );
    }
    for finding in &report.findings {
        println!();
        println!(
            "  [{}] {} — {}",
            finding.severity, finding.code, finding.plugin
        );
        println!("    subject : {}", finding.subject_id);
        println!("    {}", finding.message);
    }
    if report.findings.is_empty() {
        println!();
        println!("No findings were reported.");
    }
    println!();
    println!("Evidence only: plugin findings never authorise an operation.");
}

fn print_media(outcome: &MediaOutcome) {
    println!("Media run        : {}", outcome.run_id);
    println!("Snapshot         : {}", outcome.snapshot_id);
    println!("Status           : {}", outcome.status);
    println!("Config SHA-256   : {}", outcome.config_digest);
    println!("Contents         : {}", outcome.contents_total);
    println!("Analyzed         : {}", outcome.contents_analyzed);
    println!("Limited          : {}", outcome.contents_limited);
    println!("Failed           : {}", outcome.contents_failed);
    println!("Pairs compared   : {}", outcome.pairs_compared);
    println!("Relations        : {}", outcome.relations);
    println!(
        "Pair cap         : {}",
        if outcome.pair_cap_reached {
            "REACHED — results are conservative but not exhaustive"
        } else {
            "not reached"
        }
    );
    println!("Evidence only    : media relations never authorise an operation");
}

fn print_media_relations(report: &MediaReport) {
    let status = &report.status;
    println!("Media run      : {}", status.run_id);
    println!("Snapshot       : {}", status.snapshot_id);
    println!("Relations      : {}", status.counters.relations_total);
    println!("Pairs compared : {}", status.counters.pairs_compared);
    if status.pair_cap_reached {
        println!("Pair cap       : REACHED — the report is not exhaustive");
    }
    for relation in &status.relations {
        println!();
        println!(
            "  {} — score {:.1}%",
            relation.relation,
            f64::from(relation.score_millionths) / 10_000.0
        );
        println!(
            "    A: {}",
            relation
                .path_a
                .as_deref()
                .unwrap_or(relation.content_a.as_str())
        );
        println!(
            "    B: {}",
            relation
                .path_b
                .as_deref()
                .unwrap_or(relation.content_b.as_str())
        );
    }
    if status.relations.is_empty() {
        println!("No perceptual media relations met the engine thresholds.");
    }
    if status.relations_truncated {
        println!();
        println!("(list truncated; use --json for the full sealed evidence)");
    }
    println!();
    println!("Evidence only: media relations never authorise an operation.");
}

fn print_similarities(report: &SimilarityReport) {
    let status = &report.status;
    println!("Similarity run : {}", status.run_id);
    println!("Snapshot       : {}", status.snapshot_id);
    println!("Relationships  : {}", status.counters.relations_total);
    println!("Candidates     : {}", status.counters.candidates_total);
    if status.candidate_cap_reached {
        println!("Candidate cap  : REACHED — the report is not exhaustive");
    }
    for relation in &status.relationships {
        println!();
        println!(
            "  {} — {:.1}% exact shared-byte similarity",
            relation.kind,
            relation.similarity * 100.0
        );
        println!("    A: {}", relation.path_a);
        println!("    B: {}", relation.path_b);
        println!(
            "    {} shared chunk(s), {} shared byte(s); direction {}",
            relation.shared_chunks, relation.shared_bytes, relation.direction
        );
    }
    if status.relationships.is_empty() {
        println!("No non-identical content relationships met the configured threshold.");
    }
    if status.relationships_truncated {
        println!(
            "Showing the first {} of {} relationships.",
            status.relationships.len(),
            status.counters.relations_total
        );
    }
    println!("Evidence only: no relation authorizes deletion or consolidation.");
}

fn print_review(queue: &ReviewQueue) {
    println!("Snapshot : {}", queue.snapshot_id);
    println!("Pending  : {}", queue.pending);
    println!("Decided  : {}", queue.decided);
    for item in &queue.items {
        println!();
        println!(
            "  [{}] {} {} — {}",
            item.risk, item.status, item.kind, item.reason
        );
        println!("    id          : {}", item.id);
        if let Some(occurrence) = &item.occurrence_id {
            println!("    occurrence  : {occurrence}");
        }
        if let Some(folder) = &item.folder_a {
            println!("    folder a    : {folder}");
        }
        if let Some(folder) = &item.folder_b {
            println!("    folder b    : {folder}");
        }
        println!("    recommended : {}", item.recommended_action);
        if !item.materializable {
            println!(
                "    next step   : repair source access and rescan; bucket decisions are disabled"
            );
        }
        if let Some(decision) = &item.decision {
            println!("    decision    : {decision}");
        }
        if let Some(rationale) = &item.rationale {
            println!("    rationale   : {rationale}");
        }
        if let Some(evidence) = &item.evidence {
            println!(
                "    evidence    : {}",
                serde_json::to_string(evidence).unwrap_or_else(|_| "<invalid>".to_string())
            );
        }
    }
}

fn print_audit(report: &AuditReport) {
    println!("Project : {}", report.project_id);
    println!("Events  : {}", report.event_count);
    if report.ledger_ok {
        println!("Ledger  : OK (hash chain verified)");
    } else {
        println!("Ledger  : FAILED");
        if let Some(problem) = &report.problem {
            println!("  ! {problem}");
        }
    }
}

fn print_human(output: &Output) {
    match output {
        Output::Status(status) => print_status(status),
        Output::Scan(outcome) => print_scan(outcome),
        Output::Hash(outcome) => print_hash(outcome),
        Output::Analyze(outcome) => print_analyze(outcome),
        Output::Similarity(outcome) => print_similarity(outcome),
        Output::ContentExtraction(outcome) => print_content_extraction(outcome),
        Output::ContentArtifacts(outcome) => print_content_artifacts(outcome),
        Output::ContentSearch(outcome) => print_content_search(outcome),
        Output::ContentQuery(outcome) => print_content_query(outcome),
        Output::Plan(outcome) => print_plan(outcome),
        Output::PlanValidation(report) => print_plan_validation(report),
        Output::Approve(outcome) => print_approve(outcome),
        Output::Execute(outcome) => print_execute(outcome),
        Output::Verify(outcome) => print_verify(outcome),
        Output::Duplicates(report) => print_duplicates(report),
        Output::TreeClones(report) => print_tree_clones(report),
        Output::TreeRelations(report) => print_tree_relations(report),
        Output::Contexts(report) => print_contexts(report),
        Output::Anomalies(report) => print_anomalies(report),
        Output::Similarities(report) => print_similarities(report),
        Output::Media(outcome) => print_media(outcome),
        Output::MediaRelations(report) => print_media_relations(report),
        Output::PluginRegistered(metadata) => print_plugin_registered(metadata),
        Output::PluginList(plugins) => print_plugin_list(plugins),
        Output::PluginRuns(outcome) => print_plugin_runs(outcome),
        Output::PluginFindings(report) => print_plugin_findings(report),
        Output::AiKeys(rows) => {
            for (provider, present) in rows {
                println!(
                    "{provider}: {}",
                    if *present { "key stored" } else { "no key" }
                );
            }
        }
        Output::AiKeyChanged(message) => println!("{message}"),
        Output::AiAssist(outcome) => print_ai_assist(outcome),
        Output::AiAudits(audits) => print_ai_audits(audits),
        Output::Review(queue) => print_review(queue),
        Output::Audit(report) => print_audit(report),
    }
}

/// RFC-0001 §33 exit code for a *successful* command whose result still
/// signals a problem (failed integrity, broken ledger, partial hash).
fn verdict_exit_code(output: &Output) -> i32 {
    match output {
        Output::Status(status) => {
            if status.integrity.as_ref().is_some_and(|r| !r.is_ok()) {
                4
            } else {
                0
            }
        }
        Output::Audit(report) => {
            if report.ledger_ok {
                0
            } else {
                4
            }
        }
        Output::Scan(outcome) => {
            if outcome.cancelled || outcome.errors > 0 {
                3
            } else {
                0
            }
        }
        Output::Hash(outcome) => {
            if outcome.cancelled
                || outcome.pending > 0
                || outcome.failed > 0
                || outcome.source_changed > 0
            {
                3
            } else {
                0
            }
        }
        Output::Analyze(_) => 0,
        Output::Similarity(outcome) => {
            if outcome.cancelled || outcome.status != "COMPLETED" {
                3
            } else {
                0
            }
        }
        Output::Media(outcome) => {
            if outcome.cancelled || outcome.status != "COMPLETED" || outcome.contents_failed > 0 {
                3
            } else {
                0
            }
        }
        Output::MediaRelations(_) => 0,
        Output::PluginRuns(outcome) => {
            if outcome.cancelled
                || outcome
                    .runs
                    .iter()
                    .any(|run| run.status != "COMPLETED" || run.subjects_failed > 0)
            {
                3
            } else {
                0
            }
        }
        Output::PluginRegistered(_) | Output::PluginList(_) | Output::PluginFindings(_) => 0,
        Output::AiAssist(outcome) => {
            if !outcome.executed || outcome.status.as_deref() == Some("ACCEPTED") {
                0
            } else {
                3
            }
        }
        Output::AiKeys(_) | Output::AiKeyChanged(_) | Output::AiAudits(_) => 0,
        Output::ContentExtraction(outcome) => {
            if outcome.status == "COMPLETED"
                && outcome.counters.limited == 0
                && outcome.counters.failed == 0
            {
                0
            } else {
                3
            }
        }
        Output::ContentArtifacts(_) | Output::ContentSearch(_) | Output::ContentQuery(_) => 0,
        Output::Plan(outcome) => {
            if outcome.blocked > 0 {
                3
            } else {
                0
            }
        }
        Output::PlanValidation(report) => {
            if report.ok {
                0
            } else {
                2
            }
        }
        Output::Approve(_) => 0,
        Output::Execute(outcome) => {
            if outcome.cancelled
                || outcome.pending > 0
                || outcome.failed_retryable > 0
                || outcome.failed_final > 0
            {
                3
            } else {
                0
            }
        }
        Output::Verify(outcome) => {
            if outcome.verdict == "FAILED" {
                4
            } else {
                0
            }
        }
        // Evidence reports always succeed: finding duplicates, clones or
        // partial clones is information, not a failure.
        Output::Duplicates(_) => 0,
        Output::TreeClones(_) => 0,
        Output::TreeRelations(_) => 0,
        Output::Contexts(_) => 0,
        Output::Anomalies(_) | Output::Similarities(_) | Output::Review(_) => 0,
    }
}

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(output) => {
            if cli.json {
                match serde_json::to_string_pretty(&output) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("error: failed to serialize output: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                print_human(&output);
            }
            let code = verdict_exit_code(&output);
            if code != 0 {
                std::process::exit(code);
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(error.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn content_extract_defaults_are_bounded_and_explicit() {
        let cli = Cli::parse_from(["dataforge", "content", "extract", "--path", "project"]);
        let Command::Content {
            command:
                ContentCommand::Extract {
                    page_size,
                    max_input_bytes,
                    max_archive_entries,
                    max_archive_nesting_depth,
                    ..
                },
        } = cli.command
        else {
            panic!("content extract command expected");
        };
        assert_eq!(page_size, 64);
        assert_eq!(max_input_bytes, 64 * 1024 * 1024);
        assert_eq!(max_archive_entries, 10_000);
        assert_eq!(max_archive_nesting_depth, 4);
    }

    #[test]
    fn completed_extraction_with_limited_or_failed_evidence_is_a_partial_exit() {
        for (limited, failed) in [(1, 0), (0, 1)] {
            let counters = df_domain::ExtractionRunCounters {
                contents_total: 1,
                limited,
                failed,
                ..df_domain::ExtractionRunCounters::default()
            };
            let output = Output::ContentExtraction(ContentExtractionOutcome {
                run_id: "run".to_string(),
                snapshot_id: "snapshot".to_string(),
                status: "COMPLETED".to_string(),
                extractor_version: "extractor".to_string(),
                config_digest: "0".repeat(64),
                counters,
                processed_this_invocation: 1,
                reused_this_invocation: 0,
                threads_built_this_invocation: 0,
                error: None,
            });
            assert_eq!(verdict_exit_code(&output), 3);
        }
    }

    #[test]
    fn create_then_status_through_the_facade() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let output = tmp.path().join("salida");

        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "Prueba CLI",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            output.to_str().unwrap(),
        ]);
        let created = match run(&create).expect("create succeeds") {
            Output::Status(status) => status,
            _ => panic!("create returns a status"),
        };
        assert_eq!(created.state, "CREATED");
        assert_eq!(created.event_count, 1);

        let status = Cli::parse_from([
            "dataforge",
            "project",
            "status",
            "--path",
            project_dir.to_str().unwrap(),
        ]);
        let report = match run(&status).expect("status succeeds") {
            Output::Status(status) => status,
            _ => panic!("status returns a status"),
        };
        assert_eq!(report.project_id, created.project_id);
        assert!(report.integrity.as_ref().expect("integrity ran").is_ok());
    }

    #[test]
    fn full_pipeline_scan_hash_report_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let origin = tmp.path().join("origen");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("x.txt"), b"dup").unwrap();
        std::fs::write(origin.join("y.txt"), b"dup").unwrap();

        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "Pipeline",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            tmp.path().join("salida").to_str().unwrap(),
            "--source",
            origin.to_str().unwrap(),
        ]);
        run(&create).expect("create succeeds");

        let path = project_dir.to_str().unwrap();
        let scan =
            run(&Cli::parse_from(["dataforge", "scan", "--path", path])).expect("scan succeeds");
        match &scan {
            Output::Scan(outcome) => {
                assert_eq!(outcome.files, 2);
                assert_eq!(outcome.state, "SCANNED");
                assert_eq!(verdict_exit_code(&scan), 0);
            }
            _ => panic!("scan returns a scan outcome"),
        }

        let hash =
            run(&Cli::parse_from(["dataforge", "hash", "--path", path])).expect("hash succeeds");
        match &hash {
            Output::Hash(outcome) => {
                assert_eq!(outcome.hashed, 2);
                assert_eq!(outcome.state, "HASHED");
                assert_eq!(verdict_exit_code(&hash), 0);
            }
            _ => panic!("hash returns a hash outcome"),
        }

        let analyze = run(&Cli::parse_from(["dataforge", "analyze", "--path", path]))
            .expect("analyze succeeds");
        match &analyze {
            Output::Analyze(outcome) => assert_eq!(outcome.state, "ANALYZED"),
            _ => panic!("analyze returns an analyze outcome"),
        }

        let similarity = run(&Cli::parse_from([
            "dataforge",
            "similarity",
            "--path",
            path,
        ]))
        .expect("similarity succeeds");
        match &similarity {
            Output::Similarity(outcome) => {
                assert_eq!(outcome.status, "COMPLETED");
                assert_eq!(outcome.counters.contents_skipped, 1);
                assert_eq!(verdict_exit_code(&similarity), 0);
            }
            _ => panic!("similarity returns a similarity outcome"),
        }

        let similarities = run(&Cli::parse_from([
            "dataforge",
            "report",
            "similarities",
            "--path",
            path,
        ]))
        .expect("similarity report succeeds");
        match &similarities {
            Output::Similarities(report) => {
                assert!(report.evidence_only);
                assert!(report.status.relationships.is_empty());
            }
            _ => panic!("report similarities returns a similarity report"),
        }

        let dupes = run(&Cli::parse_from([
            "dataforge",
            "report",
            "duplicates",
            "--path",
            path,
        ]))
        .expect("report succeeds");
        match &dupes {
            Output::Duplicates(report) => {
                assert_eq!(report.sets.len(), 1);
                assert_eq!(report.redundant_files, 1);
            }
            _ => panic!("report returns duplicates"),
        }

        let plan = run(&Cli::parse_from([
            "dataforge",
            "plan",
            "create",
            "--path",
            path,
        ]))
        .expect("plan create succeeds");
        match &plan {
            Output::Plan(outcome) => {
                assert_eq!(outcome.copies, 2);
                assert_eq!(outcome.state, "PLAN_READY");
                assert_eq!(verdict_exit_code(&plan), 0);
            }
            _ => panic!("plan create returns a plan outcome"),
        }

        let validation = run(&Cli::parse_from([
            "dataforge",
            "plan",
            "validate",
            "--path",
            path,
        ]))
        .expect("plan validate succeeds");
        match &validation {
            Output::PlanValidation(report) => assert!(report.ok, "{:?}", report.problems),
            _ => panic!("plan validate returns a validation report"),
        }

        let approve = run(&Cli::parse_from([
            "dataforge",
            "plan",
            "approve",
            "--path",
            path,
        ]))
        .expect("plan approve succeeds");
        match &approve {
            Output::Approve(outcome) => assert_eq!(outcome.state, "PLAN_APPROVED"),
            _ => panic!("plan approve returns an approve outcome"),
        }

        // Write safety is Windows-only in this version: on POSIX, execution
        // must refuse explicitly (fail closed) with the approved plan and a
        // valid ledger left intact — that refusal is the pinned behavior.
        // The Windows half continues through copy and verification.
        #[cfg(not(windows))]
        {
            let refused = run(&Cli::parse_from(["dataforge", "execute", "--path", path]))
                .expect_err("execute must refuse without platform write safety");
            assert!(
                refused.to_string().contains("refusing to execute"),
                "unexpected refusal: {refused}"
            );
            let audit = run(&Cli::parse_from([
                "dataforge",
                "audit",
                "verify",
                "--path",
                path,
            ]))
            .expect("audit succeeds after the refusal");
            match &audit {
                Output::Audit(report) => assert!(report.ledger_ok),
                _ => panic!("audit returns an audit report"),
            }
        }

        #[cfg(windows)]
        {
            let execute = run(&Cli::parse_from(["dataforge", "execute", "--path", path]))
                .expect("execute succeeds");
            match &execute {
                Output::Execute(outcome) => {
                    assert_eq!(outcome.state, "EXECUTED");
                    assert_eq!(verdict_exit_code(&execute), 0);
                }
                _ => panic!("execute returns an execute outcome"),
            }

            let verify = run(&Cli::parse_from(["dataforge", "verify", "--path", path]))
                .expect("verify succeeds");
            match &verify {
                Output::Verify(outcome) => {
                    assert_eq!(outcome.verdict, "COMPLETED", "{:?}", outcome.findings);
                    assert_eq!(verdict_exit_code(&verify), 0);
                }
                _ => panic!("verify returns a verify outcome"),
            }

            // The verified copy landed in the output root.
            assert_eq!(
                std::fs::read(tmp.path().join("salida").join("origen").join("x.txt")).unwrap(),
                b"dup"
            );

            let audit = run(&Cli::parse_from([
                "dataforge",
                "audit",
                "verify",
                "--path",
                path,
            ]))
            .expect("audit succeeds");
            match &audit {
                Output::Audit(report) => {
                    assert!(report.ledger_ok);
                    assert_eq!(verdict_exit_code(&audit), 0);
                }
                _ => panic!("audit returns an audit report"),
            }
        }
    }

    #[test]
    fn status_of_missing_project_maps_to_generic_failure_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let status = Cli::parse_from([
            "dataforge",
            "project",
            "status",
            "--path",
            tmp.path().join("nada").to_str().unwrap(),
        ]);
        let err = run(&status).unwrap_err();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn invalid_creation_maps_to_validation_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "  ",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            tmp.path().join("salida").to_str().unwrap(),
        ]);
        let err = run(&create).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn scanning_before_creating_sources_is_a_validation_error() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("proyecto");
        let create = Cli::parse_from([
            "dataforge",
            "project",
            "create",
            "--name",
            "Sin fuentes",
            "--path",
            project_dir.to_str().unwrap(),
            "--output-root",
            tmp.path().join("salida").to_str().unwrap(),
        ]);
        run(&create).expect("create succeeds");
        let scan = Cli::parse_from(["dataforge", "scan", "--path", project_dir.to_str().unwrap()]);
        let err = run(&scan).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }
}
