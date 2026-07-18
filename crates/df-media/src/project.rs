//! Project-level media orchestration (RFC-0001 §21, Milestone 0.5).
//!
//! Inventory → per-content perceptual analysis → bounded pairwise
//! comparison → sealed run. The flow mirrors similarity (M0.3): runs are
//! addressed by the SHA-256 of their serialized configuration, resume at
//! content granularity, and everything they produce is review evidence —
//! never an operation, never an automatic action.

use std::path::{Component, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use df_db::media as media_db;
use df_db::{inventory, repository, Db};
use df_domain::{Actor, FileFingerprint, MediaRelationKind, MediaRun, MediaRunCounters};
use df_error::{DfError, DfResult};
use serde::Serialize;
use sha2::Digest;

use crate::engine::descriptor_for;
use crate::types::{
    FailureCode, MediaAnalysis, MediaKind, MediaLimits, MediaRequest, MediaStatus,
    ANALYSIS_CONTRACT_VERSION, AUDIO_ALGORITHM_VERSION, IMAGE_ALGORITHM_VERSION,
    VIDEO_ALGORITHM_VERSION,
};
use crate::{analyze_media, compare_media, MediaSidecars, ReviewRelation};

/// Extensions considered for each kind; lowercase, without the dot, exactly
/// as the scanner normalizes them. The lists are serialized into the run
/// configuration so the run's scope is self-describing.
pub const IMAGE_EXTENSIONS: &[&str] = &["bmp", "gif", "jpeg", "jpg", "png", "tif", "tiff", "webp"];
pub const AUDIO_EXTENSIONS: &[&str] = &["aac", "flac", "m4a", "mp3", "ogg", "opus", "wav", "wma"];
pub const VIDEO_EXTENSIONS: &[&str] = &["avi", "m4v", "mkv", "mov", "mp4", "webm", "wmv"];

const SOURCE_PAGE: u32 = 256;
const RELATION_FLUSH: usize = 256;
const HARD_MAX_PAIRS: u64 = 10_000_000;

/// Options of one project-level media run.
///
/// Sidecar paths are machine-local wiring, not evidence: their absence
/// produces explicit `FAILED` rows with `WORKER_UNAVAILABLE`, and they are
/// deliberately excluded from the reproducible configuration digest.
#[derive(Debug, Clone)]
pub struct MediaProjectOptions {
    pub limits: MediaLimits,
    /// Upper bound of pairwise comparisons across the whole run.
    pub max_pairs: u64,
    pub sidecars: MediaSidecars,
}

impl Default for MediaProjectOptions {
    fn default() -> Self {
        Self {
            limits: MediaLimits::default(),
            max_pairs: 100_000,
            sidecars: MediaSidecars::none(),
        }
    }
}

/// Serializable result of one run; mirrors the sealed row.
#[derive(Debug, Clone, Serialize)]
pub struct MediaOutcome {
    pub run_id: String,
    pub snapshot_id: String,
    pub status: String,
    pub config_digest: String,
    pub contents_total: u64,
    pub contents_analyzed: u64,
    pub contents_limited: u64,
    pub contents_failed: u64,
    pub pairs_compared: u64,
    pub pair_cap_reached: bool,
    pub relations: u64,
    pub cancelled: bool,
    /// Always true: media relations never authorise an operation.
    pub evidence_only: bool,
}

impl MediaOutcome {
    fn from_run(run: &MediaRun, cancelled: bool) -> Self {
        Self {
            run_id: run.id.to_string(),
            snapshot_id: run.snapshot_id.to_string(),
            status: run.status.as_str().to_string(),
            config_digest: run.config_digest.clone(),
            contents_total: run.counters.contents_total,
            contents_analyzed: run.counters.contents_analyzed,
            contents_limited: run.counters.contents_limited,
            contents_failed: run.counters.contents_failed,
            pairs_compared: run.counters.pairs_compared,
            pair_cap_reached: run.pair_cap_reached,
            relations: run.counters.relations_total,
            cancelled,
            evidence_only: true,
        }
    }
}

fn kind_for_extension(extension: &str) -> Option<MediaKind> {
    if IMAGE_EXTENSIONS.contains(&extension) {
        Some(MediaKind::Image)
    } else if AUDIO_EXTENSIONS.contains(&extension) {
        Some(MediaKind::Audio)
    } else if VIDEO_EXTENSIONS.contains(&extension) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

fn kind_sql(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "IMAGE",
        MediaKind::Audio => "AUDIO",
        MediaKind::Video => "VIDEO",
    }
}

fn status_sql(status: MediaStatus) -> &'static str {
    match status {
        MediaStatus::Extracted => "EXTRACTED",
        MediaStatus::Limited => "LIMITED",
        MediaStatus::Failed => "FAILED",
    }
}

fn relation_kind(relation: ReviewRelation) -> MediaRelationKind {
    match relation {
        ReviewRelation::ImagePerceptualMatch => MediaRelationKind::ImagePerceptualMatch,
        ReviewRelation::AudioAcousticMatch => MediaRelationKind::AudioAcousticMatch,
        ReviewRelation::VideoPerceptualMatch => MediaRelationKind::VideoPerceptualMatch,
    }
}

fn failure_sql(code: FailureCode) -> DfResult<String> {
    let value = serde_json::to_value(code)
        .map_err(|error| DfError::Validation(format!("failure code serialization: {error}")))?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| DfError::Validation("failure code is not a string".to_string()))
}

/// The exact serialized configuration whose SHA-256 addresses the run.
/// Field order is fixed by this struct, so the digest is reproducible.
#[derive(Serialize)]
struct RunConfig<'a> {
    contract_version: &'a str,
    image_algorithm: &'a str,
    audio_algorithm: &'a str,
    video_algorithm: &'a str,
    limits: MediaLimits,
    max_pairs: u64,
    image_extensions: &'a [&'a str],
    audio_extensions: &'a [&'a str],
    video_extensions: &'a [&'a str],
}

/// Analyse every media-typed content of the latest snapshot and derive
/// bounded review relations. Resumable; the sealed run is immutable.
pub fn analyze_media_project(
    db: &mut Db,
    actor: Actor,
    options: &MediaProjectOptions,
    cancel: Option<&AtomicBool>,
) -> DfResult<MediaOutcome> {
    let limits = options
        .limits
        .validate()
        .map_err(|error| DfError::Validation(error.to_string()))?;
    if options.max_pairs == 0 || options.max_pairs > HARD_MAX_PAIRS {
        return Err(DfError::Validation(format!(
            "max_pairs must be between 1 and {HARD_MAX_PAIRS}"
        )));
    }

    let project = repository::load_project(db)?;
    let snapshot = inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;

    let config_json = serde_json::to_string(&RunConfig {
        contract_version: ANALYSIS_CONTRACT_VERSION,
        image_algorithm: IMAGE_ALGORITHM_VERSION,
        audio_algorithm: AUDIO_ALGORITHM_VERSION,
        video_algorithm: VIDEO_ALGORITHM_VERSION,
        limits,
        max_pairs: options.max_pairs,
        image_extensions: IMAGE_EXTENSIONS,
        audio_extensions: AUDIO_EXTENSIONS,
        video_extensions: VIDEO_EXTENSIONS,
    })
    .map_err(|error| DfError::Validation(format!("media config serialization: {error}")))?;
    let config_digest = hex::encode(sha2::Sha256::digest(config_json.as_bytes()));

    let spec = media_db::MediaRunSpec {
        project_id: project.id,
        snapshot_id: snapshot.id,
        contract_version: ANALYSIS_CONTRACT_VERSION.to_string(),
        config_digest,
        config_json,
    };
    let run = media_db::start_or_resume_run(db, &spec, actor)?;
    if run.status == df_domain::MediaRunStatus::Completed {
        return Ok(MediaOutcome::from_run(&run, false));
    }

    let all_extensions: Vec<&str> = IMAGE_EXTENSIONS
        .iter()
        .chain(AUDIO_EXTENSIONS)
        .chain(VIDEO_EXTENSIONS)
        .copied()
        .collect();

    // Phase 1 — one analysis per unique content, resumable per page.
    let mut cursor: Option<String> = None;
    loop {
        let sources = media_db::media_sources_after(
            db,
            run.id,
            snapshot.id,
            &all_extensions,
            cursor.as_deref(),
            SOURCE_PAGE,
        )?;
        if sources.is_empty() {
            break;
        }
        let mut batch = Vec::with_capacity(sources.len());
        for source in &sources {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                media_db::record_media_evidence(db, run.id, snapshot.id, &batch)?;
                return Ok(MediaOutcome::from_run(&run, true));
            }
            batch.push(evidence_for_source(source, limits, &options.sidecars)?);
        }
        media_db::record_media_evidence(db, run.id, snapshot.id, &batch)?;
        cursor = sources.last().map(|source| source.content_id.to_string());
    }

    // Phase 2 — bounded, deterministic pairwise comparison. Run-scoped
    // relations left by a crash are rebuilt from the immutable evidence.
    media_db::reset_run_relations(db, run.id)?;
    let mut pairs_compared = 0u64;
    let mut pair_cap_reached = false;
    let mut relations_total = 0u64;
    let mut pending: Vec<media_db::MediaRelationInput> = Vec::new();

    'kinds: for kind in [MediaKind::Image, MediaKind::Audio, MediaKind::Video] {
        let rows = media_db::extracted_analyses(db, run.id, kind_sql(kind))?;
        let mut analyses = Vec::with_capacity(rows.len());
        for (content_id, json) in rows {
            let analysis: MediaAnalysis = serde_json::from_str(&json).map_err(|error| {
                DfError::Validation(format!("stored media analysis for `{content_id}`: {error}"))
            })?;
            analyses.push((content_id, analysis));
        }
        for i in 0..analyses.len() {
            for j in (i + 1)..analyses.len() {
                if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    return Ok(MediaOutcome::from_run(&run, true));
                }
                if pairs_compared == options.max_pairs {
                    // There is at least this one more pair: the cap cut a
                    // real tail, not a coincidence of counts.
                    pair_cap_reached = true;
                    break 'kinds;
                }
                pairs_compared += 1;
                let (id_a, analysis_a) = &analyses[i];
                let (id_b, analysis_b) = &analyses[j];
                let candidate = compare_media(analysis_a, analysis_b)
                    .map_err(|error| DfError::Validation(error.to_string()))?;
                if let Some(candidate) = candidate {
                    let (content_a, content_b) = if id_a.to_string() < id_b.to_string() {
                        (*id_a, *id_b)
                    } else {
                        (*id_b, *id_a)
                    };
                    pending.push(media_db::MediaRelationInput {
                        content_a,
                        content_b,
                        kind: relation_kind(candidate.relation),
                        score_millionths: candidate.score_millionths,
                        evidence_json: serde_json::to_string(&candidate.evidence).map_err(
                            |error| DfError::Validation(format!("evidence serialization: {error}")),
                        )?,
                    });
                    relations_total += 1;
                    if pending.len() >= RELATION_FLUSH {
                        media_db::record_media_relations(db, run.id, snapshot.id, &pending)?;
                        pending.clear();
                    }
                }
            }
        }
    }
    media_db::record_media_relations(db, run.id, snapshot.id, &pending)?;

    let (total, analyzed, limited, failed) = media_db::evidence_counters(db, run.id)?;
    let counters = MediaRunCounters {
        contents_total: total,
        contents_analyzed: analyzed,
        contents_limited: limited,
        contents_failed: failed,
        pairs_compared,
        relations_total,
    };
    let completed = media_db::complete_run(db, run.id, counters, pair_cap_reached, actor)?;
    Ok(MediaOutcome::from_run(&completed, false))
}

/// Reject stored relative paths that could step outside the source root.
fn safe_source_path(source: &media_db::MediaContentSource) -> DfResult<PathBuf> {
    let relative = source
        .raw_relative_path
        .as_ref()
        .map(|raw| PathBuf::from(raw.to_os_string()))
        .unwrap_or_else(|| PathBuf::from(&source.relative_path));
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(DfError::Validation(format!(
            "stored source path for `{}` is not relative",
            source.content_id
        )));
    }
    Ok(source.root_path.join(relative))
}

/// Analyse one content. Per-file media problems become evidence rows; a
/// source that changed since inventory is a hard conflict (rescan first),
/// exactly like similarity.
fn evidence_for_source(
    source: &media_db::MediaContentSource,
    limits: MediaLimits,
    sidecars: &MediaSidecars,
) -> DfResult<media_db::MediaEvidenceInput> {
    let kind = kind_for_extension(&source.extension).ok_or_else(|| {
        DfError::Validation(format!(
            "content `{}` has no media kind for extension `{}`",
            source.content_id, source.extension
        ))
    })?;

    // Size precheck: never read more bytes than the configured ceiling.
    if source.size_bytes > limits.max_input_bytes {
        let analysis = MediaAnalysis::new(
            kind,
            MediaStatus::Limited,
            descriptor_for(kind, limits, "not-run"),
            None,
            None,
            Some(FailureCode::InputLimit),
            Some("source exceeds max_input_bytes; not read"),
        );
        return evidence_input(source, kind, analysis);
    }

    let path = safe_source_path(source)?;
    let stored = FileFingerprint::parse(&source.fingerprint)?;
    let pre = df_fs_safety::capture_fingerprint(&path)?;
    if FileFingerprint::compare(&stored, &pre).is_changed() {
        return Err(DfError::Conflict(format!(
            "source `{}` changed after inventory; rescan before media analysis",
            source.relative_path
        )));
    }
    let bytes = std::fs::read(df_fs_safety::extended_for_io(&path).as_ref())
        .map_err(|error| DfError::io(&path, error))?;
    if hex::encode(sha2::Sha256::digest(&bytes)) != source.sha256 {
        return Err(DfError::Conflict(format!(
            "source `{}` no longer matches its inventoried SHA-256; rescan first",
            source.relative_path
        )));
    }

    let analysis = analyze_media(MediaRequest::new(kind, &bytes), limits, sidecars)
        .map_err(|error| DfError::Validation(error.to_string()))?;
    evidence_input(source, kind, analysis)
}

fn evidence_input(
    source: &media_db::MediaContentSource,
    kind: MediaKind,
    analysis: MediaAnalysis,
) -> DfResult<media_db::MediaEvidenceInput> {
    let failure_code = analysis.failure_code.map(failure_sql).transpose()?;
    let analysis_json = serde_json::to_string(&analysis)
        .map_err(|error| DfError::Validation(format!("analysis serialization: {error}")))?;
    Ok(media_db::MediaEvidenceInput {
        content_id: source.content_id,
        media_kind: kind_sql(kind).to_string(),
        status: status_sql(analysis.status).to_string(),
        analysis_json,
        failure_code,
    })
}
