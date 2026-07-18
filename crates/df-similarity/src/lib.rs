//! Streaming similarity analysis for DataForge (Milestone 0.3).
//!
//! The engine uses content-defined chunks to discover related, non-identical
//! files. SHA-256 identity remains authoritative and every persisted relation
//! is review-only evidence. Corpus-sized structures live in SQLite; process
//! memory is bounded by one FastCDC buffer, one chunk and small work pages.

use std::io;
use std::path::{Component, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use df_db::similarity::{self, ExactChunkOverlap, SimilarityContentSource, SimilarityRunSpec};
use df_db::{inventory, repository, Db};
use df_domain::{
    Actor, ContentRelationship, ContentRelationshipKind, FileFingerprint, RelationshipDirection,
    SimilarityRelationId, SimilarityRun, SimilarityRunCounters, SimilarityRunStatus,
};
use df_error::{DfError, DfResult};
use fastcdc::v2020::StreamCDC;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Versioned family of the chunk-boundary and signature contract.
pub const ALGORITHM_FAMILY: &str = "fastcdc-v2020-l1-minhash-v1";

const SOURCE_PAGE: u32 = 64;
const CANDIDATE_PAGE: u32 = 128;
const MINHASH_DOMAIN: &[u8] = b"dataforge:minhash:v1";
const LSH_DOMAIN: &[u8] = b"dataforge:lsh-band:v1";

/// Tunable and fully serialized configuration of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarityOptions {
    pub min_chunk_bytes: u32,
    pub avg_chunk_bytes: u32,
    pub max_chunk_bytes: u32,
    pub min_file_bytes: u64,
    pub threshold: f64,
    pub min_shared_chunks: u32,
    pub min_shared_bytes: u64,
    pub minhash_permutations: u32,
    pub lsh_bands: u32,
    pub max_bucket_contents: u32,
    pub max_candidates: u64,
}

impl Default for SimilarityOptions {
    fn default() -> Self {
        Self {
            min_chunk_bytes: 16 * 1024,
            avg_chunk_bytes: 64 * 1024,
            max_chunk_bytes: 256 * 1024,
            min_file_bytes: 16 * 1024,
            threshold: 0.50,
            min_shared_chunks: 2,
            min_shared_bytes: 32 * 1024,
            minhash_permutations: 128,
            lsh_bands: 32,
            max_bucket_contents: 64,
            max_candidates: 200_000,
        }
    }
}

impl SimilarityOptions {
    pub fn validate(&self) -> DfResult<()> {
        if self.min_chunk_bytes < fastcdc::v2020::MINIMUM_MIN
            || self.min_chunk_bytes > fastcdc::v2020::MINIMUM_MAX
            || self.avg_chunk_bytes < fastcdc::v2020::AVERAGE_MIN
            || self.avg_chunk_bytes > fastcdc::v2020::AVERAGE_MAX
            || self.max_chunk_bytes < fastcdc::v2020::MAXIMUM_MIN
            || self.max_chunk_bytes > fastcdc::v2020::MAXIMUM_MAX
            || self.min_chunk_bytes > self.avg_chunk_bytes
            || self.avg_chunk_bytes > self.max_chunk_bytes
            || !(0.0..=1.0).contains(&self.threshold)
            || self.min_shared_chunks == 0
            || self.min_shared_bytes == 0
            || self.minhash_permutations < 16
            || self.lsh_bands == 0
            || !self.minhash_permutations.is_multiple_of(self.lsh_bands)
            || self.max_bucket_contents < 2
            || self.max_candidates == 0
            || self.min_file_bytes == 0
            || self.min_file_bytes > i64::MAX as u64
            || self.min_shared_bytes > i64::MAX as u64
            || self.max_candidates >= i64::MAX as u64
        {
            return Err(DfError::Validation(
                "invalid FastCDC/MinHash similarity configuration".to_string(),
            ));
        }
        Ok(())
    }

    /// Chunk/signature identity excludes relation thresholds so a second run
    /// can reuse immutable signatures while applying a different policy.
    pub fn algorithm_version(&self) -> String {
        format!(
            "{ALGORITHM_FAMILY}-{}-{}-{}-p{}-b{}",
            self.min_chunk_bytes,
            self.avg_chunk_bytes,
            self.max_chunk_bytes,
            self.minhash_permutations,
            self.lsh_bands
        )
    }

    fn run_spec(
        &self,
        project_id: df_domain::ProjectId,
        snapshot_id: df_domain::SnapshotId,
    ) -> DfResult<SimilarityRunSpec> {
        self.validate()?;
        let algorithm_version = self.algorithm_version();
        let config_value = serde_json::json!({
            "algorithm_version": algorithm_version,
            "options": self,
        });
        let config_json = serde_json::to_string(&config_value)
            .map_err(|error| DfError::Serialization(format!("similarity config: {error}")))?;
        let config_digest = hex::encode(Sha256::digest(config_json.as_bytes()));
        Ok(SimilarityRunSpec {
            project_id,
            snapshot_id,
            algorithm_version: self.algorithm_version(),
            config_digest,
            config_json,
            min_chunk_bytes: self.min_chunk_bytes,
            avg_chunk_bytes: self.avg_chunk_bytes,
            max_chunk_bytes: self.max_chunk_bytes,
            min_file_bytes: self.min_file_bytes,
            threshold: self.threshold,
            min_shared_chunks: self.min_shared_chunks,
            min_shared_bytes: self.min_shared_bytes,
            minhash_permutations: self.minhash_permutations,
            lsh_bands: self.lsh_bands,
            max_bucket_contents: self.max_bucket_contents,
            max_candidates: self.max_candidates,
        })
    }
}

/// Result of a completed or cooperatively paused run.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarityOutcome {
    pub run_id: String,
    pub snapshot_id: String,
    pub status: String,
    pub algorithm_version: String,
    pub config_digest: String,
    pub config: serde_json::Value,
    pub counters: SimilarityRunCounters,
    pub candidate_cap_reached: bool,
    pub cancelled: bool,
}

impl SimilarityOutcome {
    fn from_run(run: &SimilarityRun, cancelled: bool) -> Self {
        Self {
            run_id: run.id.to_string(),
            snapshot_id: run.snapshot_id.to_string(),
            status: run.status.as_str().to_string(),
            algorithm_version: run.algorithm_version.clone(),
            config_digest: run.config_digest.clone(),
            config: run.config.clone(),
            counters: run.counters,
            candidate_cap_reached: run.candidate_cap_reached,
            cancelled,
        }
    }
}

/// Analyze the latest completed and structurally sealed snapshot. Replaying
/// the same configuration is idempotent; cancelling leaves a RUNNING run that
/// restarts from immutable per-content completion markers.
pub fn analyze_project(
    db: &mut Db,
    actor: Actor,
    options: &SimilarityOptions,
    cancel: Option<&AtomicBool>,
) -> DfResult<SimilarityOutcome> {
    options.validate()?;
    let project = repository::load_project(db)?;
    let snapshot = inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    df_db::analysis::require_current_analysis_completion(
        db,
        project.id,
        snapshot.id,
        project.profile.as_str(),
    )?;
    let spec = options.run_spec(project.id, snapshot.id)?;
    let run = similarity::start_or_resume_run(db, &spec, actor)?;
    if run.status == SimilarityRunStatus::Completed {
        return Ok(SimilarityOutcome::from_run(&run, false));
    }

    let mut cursor: Option<String> = None;
    loop {
        let sources =
            similarity::similarity_sources_after(db, snapshot.id, cursor.as_deref(), SOURCE_PAGE)?;
        if sources.is_empty() {
            break;
        }
        for source in &sources {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return Ok(SimilarityOutcome::from_run(&run, true));
            }
            if source.size_bytes >= options.min_file_bytes
                && !similarity::has_content_signature(
                    db,
                    source.content_id,
                    &spec.algorithm_version,
                )?
            {
                chunk_content(db, source, options, &spec.algorithm_version)?;
            }
        }
        cursor = sources.last().map(|source| source.content_id.to_string());
    }

    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Ok(SimilarityOutcome::from_run(&run, true));
    }

    // Run-scoped pairs may have been left by a crash. Rebuilding them from
    // immutable signatures is deterministic and bounded by max_candidates.
    similarity::reset_run_pairs(db, run.id)?;
    let discovery_limit = options.max_candidates + 1;
    similarity::generate_rare_chunk_candidates(db, run.id, discovery_limit)?;
    let after_rare = similarity::candidate_count(db, run.id)?;
    if after_rare < discovery_limit {
        similarity::generate_lsh_candidates(db, run.id, discovery_limit - after_rare)?;
    }
    let discovered = similarity::candidate_count(db, run.id)?;
    let candidate_cap_reached = discovered > options.max_candidates;
    if candidate_cap_reached {
        similarity::trim_candidates(db, run.id, options.max_candidates)?;
    }
    similarity::refresh_candidate_band_evidence(db, run.id)?;

    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Ok(SimilarityOutcome::from_run(&run, true));
        }
        let candidates = similarity::pending_candidates(db, run.id, CANDIDATE_PAGE)?;
        if candidates.is_empty() {
            break;
        }
        for candidate in candidates {
            evaluate_candidate(db, &run, options, &spec.algorithm_version, &candidate)?;
        }
    }

    let completed = similarity::complete_run(db, run.id, candidate_cap_reached, actor)?;
    Ok(SimilarityOutcome::from_run(&completed, false))
}

fn safe_source_path(source: &SimilarityContentSource) -> DfResult<PathBuf> {
    let raw = source.raw_relative_path.as_ref().ok_or_else(|| {
        DfError::Validation(format!(
            "content `{}` only has a legacy/lossy source path; rescan before similarity analysis",
            source.content_id
        ))
    })?;
    let relative = PathBuf::from(raw.to_os_string());
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(DfError::Validation(format!(
            "stored raw source path for `{}` is not relative",
            source.content_id
        )));
    }
    Ok(source.root_path.join(relative))
}

fn chunk_content(
    db: &mut Db,
    source: &SimilarityContentSource,
    options: &SimilarityOptions,
    algorithm_version: &str,
) -> DfResult<()> {
    let path = safe_source_path(source)?;
    let stored = FileFingerprint::parse(&source.fingerprint)?;
    let pre = df_fs_safety::capture_fingerprint(&path)?;
    if FileFingerprint::compare(&stored, &pre).is_changed() {
        return Err(DfError::Conflict(format!(
            "source `{}` changed after inventory; rescan before similarity analysis",
            source.relative_path
        )));
    }
    let file = std::fs::File::open(df_fs_safety::extended_for_io(&path).as_ref())
        .map_err(|error| DfError::io(&path, error))?;
    let mut writer = similarity::begin_content_chunks(db, source.content_id, algorithm_version)?;
    let mut signature = vec![u64::MAX; options.minhash_permutations as usize];
    let mut sha256 = Sha256::new();
    let mut chunker = StreamCDC::new(
        file,
        options.min_chunk_bytes,
        options.avg_chunk_bytes,
        options.max_chunk_bytes,
    );
    for result in chunker.by_ref() {
        let chunk =
            result.map_err(|error| DfError::io(&path, io::Error::other(error.to_string())))?;
        sha256.update(&chunk.data);
        let digest = blake3::hash(&chunk.data);
        update_minhash(&mut signature, digest.as_bytes());
        writer.write_chunk(chunk.offset, chunk.length as u64, digest.to_hex().as_ref())?;
    }
    drop(chunker);

    let post = df_fs_safety::capture_fingerprint(&path)?;
    if FileFingerprint::compare(&pre, &post).is_changed() {
        return Err(DfError::Conflict(format!(
            "source `{}` changed while content chunks were read",
            source.relative_path
        )));
    }
    let observed_sha256 = hex::encode(sha256.finalize());
    let band_hashes = lsh_band_hashes(&signature, options.lsh_bands as usize);
    writer.finish(&signature, &band_hashes, &observed_sha256)
}

/// Double hashing derives deterministic pseudo-permutations from one BLAKE3
/// token. This is stable, cheap and independent of process/hash-map seeds.
fn update_minhash(signature: &mut [u64], chunk_digest: &[u8; 32]) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(MINHASH_DOMAIN);
    hasher.update(chunk_digest);
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    let h1 = u64::from_le_bytes(bytes[0..8].try_into().expect("8-byte slice"));
    let h2 = u64::from_le_bytes(bytes[8..16].try_into().expect("8-byte slice")) | 1;
    for (index, minimum) in signature.iter_mut().enumerate() {
        let permuted = h1.wrapping_add((index as u64).wrapping_mul(h2));
        *minimum = (*minimum).min(permuted);
    }
}

fn lsh_band_hashes(signature: &[u64], bands: usize) -> Vec<String> {
    let rows = signature.len() / bands;
    signature
        .chunks_exact(rows)
        .enumerate()
        .map(|(index, values)| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(LSH_DOMAIN);
            hasher.update(&(index as u64).to_le_bytes());
            for value in values {
                hasher.update(&value.to_le_bytes());
            }
            hasher.finalize().to_hex().to_string()
        })
        .collect()
}

fn decode_signature(blob: &[u8]) -> DfResult<Vec<u64>> {
    if blob.is_empty() || !blob.len().is_multiple_of(8) {
        return Err(DfError::Serialization(format!(
            "stored MinHash signature has invalid byte length {}",
            blob.len()
        )));
    }
    Ok(blob
        .chunks_exact(8)
        .map(|bytes| u64::from_le_bytes(bytes.try_into().expect("8-byte chunk")))
        .collect())
}

fn minhash_similarity(a: &[u8], b: &[u8]) -> DfResult<f64> {
    let a = decode_signature(a)?;
    let b = decode_signature(b)?;
    if a.len() != b.len() {
        return Err(DfError::Conflict(
            "candidate MinHash signatures use different permutation counts".to_string(),
        ));
    }
    let equal = a
        .iter()
        .zip(&b)
        .filter(|(left, right)| left == right)
        .count();
    Ok(equal as f64 / a.len() as f64)
}

fn temporal_direction(
    a: Option<df_domain::Timestamp>,
    b: Option<df_domain::Timestamp>,
) -> RelationshipDirection {
    match (a, b) {
        (Some(left), Some(right)) if left < right => RelationshipDirection::AToB,
        (Some(left), Some(right)) if right < left => RelationshipDirection::BToA,
        _ => RelationshipDirection::Unknown,
    }
}

fn classify_relationship(
    exact: ExactChunkOverlap,
    size_a: u64,
    size_b: u64,
    rare_chunk_hits: u32,
) -> (ContentRelationshipKind, f64) {
    let smaller = size_a.min(size_b).max(1);
    let coverage_of_smaller = exact.shared_bytes as f64 / smaller as f64;
    let size_ratio = smaller as f64 / size_a.max(size_b).max(1) as f64;
    if coverage_of_smaller >= 0.90 && size_ratio <= 0.90 {
        (ContentRelationshipKind::TruncatedVariant, 0.95)
    } else if exact.similarity >= 0.65 {
        (ContentRelationshipKind::LikelyVersion, 0.90)
    } else if rare_chunk_hits > 0 && size_ratio <= 0.75 {
        (ContentRelationshipKind::RecomposedContent, 0.80)
    } else {
        (ContentRelationshipKind::SimilarContent, 0.75)
    }
}

fn evaluate_candidate(
    db: &mut Db,
    run: &SimilarityRun,
    options: &SimilarityOptions,
    algorithm_version: &str,
    candidate: &similarity::PendingSimilarityCandidate,
) -> DfResult<()> {
    let estimated = minhash_similarity(&candidate.signature_a, &candidate.signature_b)?;
    let exact = similarity::exact_chunk_overlap(
        db,
        algorithm_version,
        candidate.content_a,
        candidate.content_b,
        candidate.size_a,
        candidate.size_b,
    )?;
    let qualifies = exact.similarity >= options.threshold
        && exact.shared_chunks >= u64::from(options.min_shared_chunks)
        && exact.shared_bytes >= options.min_shared_bytes;
    let relation = if qualifies {
        let (kind, confidence) = classify_relationship(
            exact,
            candidate.size_a,
            candidate.size_b,
            candidate.rare_chunk_hits,
        );
        Some(ContentRelationship {
            id: SimilarityRelationId::new(),
            run_id: run.id,
            snapshot_id: run.snapshot_id,
            content_a: candidate.content_a,
            content_b: candidate.content_b,
            kind,
            direction: temporal_direction(candidate.modified_a, candidate.modified_b),
            similarity: exact.similarity,
            shared_chunks: exact.shared_chunks,
            shared_bytes: exact.shared_bytes,
            union_bytes: exact.union_bytes,
            estimated_similarity: estimated,
            confidence,
            evidence: serde_json::json!({
                "metric": "multiset_weighted_jaccard_bytes",
                "shared_bands": candidate.shared_bands,
                "rare_chunk_hits": candidate.rare_chunk_hits,
                "size_a": candidate.size_a,
                "size_b": candidate.size_b,
                "threshold": options.threshold,
                "identity_rule": "sha256_equality_only",
                "automatic_action": false,
            }),
            created_at: chrono::Utc::now(),
        })
    } else {
        None
    };
    similarity::record_candidate_evaluation(
        db,
        run.id,
        candidate.content_a,
        candidate.content_b,
        estimated,
        exact,
        relation.as_ref(),
    )
}

/// Latest sealed similarity evidence for client reports.
pub fn latest_report(
    db: &Db,
    project_id: df_domain::ProjectId,
    limit: u32,
) -> DfResult<Option<(SimilarityRun, Vec<ContentRelationship>)>> {
    let Some(run) = similarity::latest_completed_run(db, project_id)? else {
        return Ok(None);
    };
    let relationships = similarity::list_relationships(db, run.id, limit)?;
    Ok(Some((run, relationships)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{ProfileRef, Project, SourceRoot};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    fn pseudo_random_bytes(length: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut out = Vec::with_capacity(length);
        for _ in 0..length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.push(state as u8);
        }
        out
    }

    fn analyzed_fixture(temp: &Path) -> (Db, df_domain::ProjectId) {
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let base = pseudo_random_bytes(768 * 1024, 0x1234_5678);
        let mut version = base.clone();
        version[300 * 1024..308 * 1024].fill(0xA5);
        std::fs::write(source.join("contract-v1.bin"), &base).unwrap();
        std::fs::write(source.join("contract-v2.bin"), &version).unwrap();
        std::fs::write(
            source.join("unrelated.bin"),
            pseudo_random_bytes(768 * 1024, 0xDEAD_BEEF),
        )
        .unwrap();

        let mut db = Db::open(&temp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "similarity fixture",
            ProfileRef::default(),
            temp.join("output"),
            temp.join("audit"),
            "test",
        );
        repository::create_project(
            &mut db,
            &project,
            &[SourceRoot::new(project.id, source)],
            Actor::Test,
        )
        .unwrap();
        df_scan::scan_project(&mut db, Actor::Test, &df_scan::ScanOptions::default(), None)
            .unwrap();
        df_hash::hash_project(&mut db, Actor::Test, &df_hash::HashOptions::default(), None)
            .unwrap();
        df_planner::analyze_project(&mut db, Actor::Test).unwrap();
        (db, project.id)
    }

    fn analyzed_version_family(temp: &Path) -> Db {
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let base = pseudo_random_bytes(768 * 1024, 0xCAFE_BABE);
        for index in 0..5 {
            let mut version = base.clone();
            let start = (100 + index * 60) * 1024;
            version[start..start + 4096].fill(0x20 + index as u8);
            std::fs::write(source.join(format!("version-{index}.bin")), version).unwrap();
        }
        let mut db = Db::open(&temp.join("state.sqlite")).unwrap();
        let project = Project::new(
            "candidate cap fixture",
            ProfileRef::default(),
            temp.join("output"),
            temp.join("audit"),
            "test",
        );
        repository::create_project(
            &mut db,
            &project,
            &[SourceRoot::new(project.id, source)],
            Actor::Test,
        )
        .unwrap();
        df_scan::scan_project(&mut db, Actor::Test, &df_scan::ScanOptions::default(), None)
            .unwrap();
        df_hash::hash_project(&mut db, Actor::Test, &df_hash::HashOptions::default(), None)
            .unwrap();
        df_planner::analyze_project(&mut db, Actor::Test).unwrap();
        db
    }

    #[test]
    fn synthetic_versions_are_related_but_never_identical() {
        let temp = tempfile::tempdir().unwrap();
        let (mut db, project_id) = analyzed_fixture(temp.path());
        let options = SimilarityOptions {
            threshold: 0.30,
            min_shared_chunks: 1,
            min_shared_bytes: 16 * 1024,
            ..SimilarityOptions::default()
        };
        let outcome = analyze_project(&mut db, Actor::Test, &options, None).unwrap();
        assert_eq!(outcome.status, "COMPLETED");
        assert_eq!(outcome.counters.contents_total, 3);
        assert_eq!(outcome.counters.contents_chunked, 3);
        assert!(!outcome.candidate_cap_reached);

        let (run, relationships) = latest_report(&db, project_id, 100).unwrap().unwrap();
        assert_eq!(run.id.to_string(), outcome.run_id);
        assert_eq!(
            relationships.len(),
            1,
            "only the two versions should relate"
        );
        let relation = &relationships[0];
        assert!(relation.similarity > 0.30);
        assert!(
            relation.similarity < 1.0,
            "similarity must not become identity"
        );
        assert_eq!(relation.evidence["automatic_action"], false);
        let events = repository::list_events(&db, project_id).unwrap();
        assert!(events
            .iter()
            .any(|event| event.event_type == similarity::EVENT_SIMILARITY_STARTED));
        assert!(events
            .iter()
            .any(|event| event.event_type == similarity::EVENT_SIMILARITY_COMPLETED));
        df_ledger::verify_chain(&events).unwrap();

        // Same snapshot + config is a pure replay of the sealed result.
        let replay = analyze_project(&mut db, Actor::Test, &options, None).unwrap();
        assert_eq!(replay.run_id, outcome.run_id);
        assert_eq!(replay.counters, outcome.counters);

        // Thresholds define a new run but reuse the immutable chunk/signature
        // layer. Removing the source proves this replay performs no new read.
        std::fs::remove_dir_all(temp.path().join("source")).unwrap();
        let strict = SimilarityOptions {
            threshold: 0.9999,
            ..options.clone()
        };
        let strict_outcome = analyze_project(&mut db, Actor::Test, &strict, None).unwrap();
        assert_ne!(strict_outcome.run_id, outcome.run_id);
        assert_eq!(strict_outcome.status, "COMPLETED");
        assert_eq!(strict_outcome.counters.relations_total, 0);
    }

    #[test]
    fn cancelled_run_resumes_from_the_same_configuration_identity() {
        let temp = tempfile::tempdir().unwrap();
        let (mut db, _) = analyzed_fixture(temp.path());
        let options = SimilarityOptions {
            threshold: 0.30,
            min_shared_chunks: 1,
            min_shared_bytes: 16 * 1024,
            ..SimilarityOptions::default()
        };
        let cancel = AtomicBool::new(true);
        let paused = analyze_project(&mut db, Actor::Test, &options, Some(&cancel)).unwrap();
        assert!(paused.cancelled);
        assert_eq!(paused.status, "RUNNING");
        cancel.store(false, Ordering::Relaxed);
        let resumed = analyze_project(&mut db, Actor::Test, &options, Some(&cancel)).unwrap();
        assert!(!resumed.cancelled);
        assert_eq!(resumed.status, "COMPLETED");
        assert_eq!(resumed.run_id, paused.run_id);
    }

    #[test]
    fn candidate_cap_is_exactly_signalled_and_persisted() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = analyzed_version_family(temp.path());
        let options = SimilarityOptions {
            threshold: 0.10,
            min_shared_chunks: 1,
            min_shared_bytes: 1,
            max_candidates: 1,
            ..SimilarityOptions::default()
        };
        let outcome = analyze_project(&mut db, Actor::Test, &options, None).unwrap();
        assert!(outcome.candidate_cap_reached);
        assert_eq!(outcome.counters.candidates_total, 1);
        assert!(outcome.counters.relations_total <= 1);
    }

    #[test]
    fn options_reject_unbounded_or_incoherent_shapes() {
        assert!(SimilarityOptions {
            max_candidates: 0,
            ..SimilarityOptions::default()
        }
        .validate()
        .is_err());
        assert!(SimilarityOptions {
            minhash_permutations: 127,
            ..SimilarityOptions::default()
        }
        .validate()
        .is_err());
        assert!(SimilarityOptions {
            threshold: f64::NAN,
            ..SimilarityOptions::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn minhash_and_lsh_are_deterministic() {
        let token = blake3::hash(b"same chunk");
        let mut left = vec![u64::MAX; 128];
        let mut right = vec![u64::MAX; 128];
        update_minhash(&mut left, token.as_bytes());
        update_minhash(&mut right, token.as_bytes());
        assert_eq!(left, right);
        assert_eq!(lsh_band_hashes(&left, 32), lsh_band_hashes(&right, 32));
    }

    #[test]
    #[ignore = "manual 256 MiB streaming benchmark"]
    fn benchmark_streaming_memory_is_independent_of_corpus_size() {
        use std::io::Read;
        use std::time::Instant;

        let options = SimilarityOptions::default();
        let bytes = 256_u64 * 1024 * 1024;
        let source = std::io::repeat(0xA5).take(bytes);
        let started = Instant::now();
        let mut signature = vec![u64::MAX; options.minhash_permutations as usize];
        let mut chunks = 0_u64;
        let mut largest = 0_usize;
        for chunk in StreamCDC::new(
            source,
            options.min_chunk_bytes,
            options.avg_chunk_bytes,
            options.max_chunk_bytes,
        ) {
            let chunk = chunk.unwrap();
            largest = largest.max(chunk.length);
            update_minhash(&mut signature, blake3::hash(&chunk.data).as_bytes());
            chunks += 1;
        }
        let elapsed = started.elapsed();
        assert!(largest <= options.max_chunk_bytes as usize);
        assert!(chunks > 0);
        eprintln!(
            "[similarity benchmark] {bytes} bytes, {chunks} chunks in {:.2?}; bounded working set <= two max chunks + {} signature bytes",
            elapsed,
            signature.len() * 8
        );
    }
}
