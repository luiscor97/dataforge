use crate::fingerprint::parse_phash;
use crate::types::{
    ComparisonEvidence, MediaAnalysis, MediaCompareError, MediaFingerprint, MediaKind,
    ReviewCandidate, ReviewRelation, ANALYSIS_CONTRACT_VERSION, AUDIO_ALGORITHM_VERSION,
    IMAGE_ALGORITHM_VERSION, VIDEO_ALGORITHM_VERSION,
};

const MAX_AUDIO_FINGERPRINT_WORDS: usize = 10_000;
const MAX_VIDEO_KEYFRAMES: usize = 600;
const MIN_AUDIO_OVERLAP_WORDS: usize = 8;
const MIN_VIDEO_OVERLAP_FRAMES: usize = 2;
const MAX_IMAGE_HAMMING: u32 = 12;
const MAX_AUDIO_AVERAGE_HAMMING_MILLI: u64 = 10_000;
const MAX_VIDEO_AVERAGE_HAMMING_MILLI: u64 = 12_000;

/// Compare two versioned analyses and, when conservative thresholds are met,
/// return a review candidate. This API has no mutation or deletion operation.
pub fn compare_media(
    left: &MediaAnalysis,
    right: &MediaAnalysis,
) -> Result<Option<ReviewCandidate>, MediaCompareError> {
    if left.contract_version != ANALYSIS_CONTRACT_VERSION
        || right.contract_version != ANALYSIS_CONTRACT_VERSION
    {
        return Err(MediaCompareError::ContractMismatch);
    }
    if left.kind != right.kind {
        return Err(MediaCompareError::KindMismatch);
    }
    let expected_version = match left.kind {
        MediaKind::Image => IMAGE_ALGORITHM_VERSION,
        MediaKind::Audio => AUDIO_ALGORITHM_VERSION,
        MediaKind::Video => VIDEO_ALGORITHM_VERSION,
    };
    if left.algorithm.version != expected_version || right.algorithm.version != expected_version {
        return Err(MediaCompareError::ContractMismatch);
    }
    let (Some(left_fingerprint), Some(right_fingerprint)) = (&left.fingerprint, &right.fingerprint)
    else {
        return Ok(None);
    };

    match (left.kind, left_fingerprint, right_fingerprint) {
        (MediaKind::Image, MediaFingerprint::Image(left), MediaFingerprint::Image(right)) => {
            compare_images(&left.phash64, &right.phash64)
        }
        (MediaKind::Audio, MediaFingerprint::Audio(left), MediaFingerprint::Audio(right)) => {
            compare_audio(&left.subfingerprints, &right.subfingerprints)
        }
        (MediaKind::Video, MediaFingerprint::Video(left), MediaFingerprint::Video(right)) => {
            let left_hashes = left
                .keyframes
                .iter()
                .map(|keyframe| keyframe.phash64.as_str())
                .collect::<Vec<_>>();
            let right_hashes = right
                .keyframes
                .iter()
                .map(|keyframe| keyframe.phash64.as_str())
                .collect::<Vec<_>>();
            compare_video(&left_hashes, &right_hashes)
        }
        _ => Err(MediaCompareError::FingerprintMismatch),
    }
}

fn compare_images(left: &str, right: &str) -> Result<Option<ReviewCandidate>, MediaCompareError> {
    let left = parse_phash(left).ok_or_else(|| {
        MediaCompareError::MalformedFingerprint("invalid left image pHash".to_string())
    })?;
    let right = parse_phash(right).ok_or_else(|| {
        MediaCompareError::MalformedFingerprint("invalid right image pHash".to_string())
    })?;
    let distance = (left ^ right).count_ones();
    if distance > MAX_IMAGE_HAMMING {
        return Ok(None);
    }
    Ok(Some(ReviewCandidate {
        relation: ReviewRelation::ImagePerceptualMatch,
        score_millionths: similarity_millionths(u64::from(distance), 64),
        evidence: ComparisonEvidence::Image {
            hamming_distance: distance,
        },
        automatic_action: false,
    }))
}

fn compare_audio(
    left: &[u32],
    right: &[u32],
) -> Result<Option<ReviewCandidate>, MediaCompareError> {
    if left.len() > MAX_AUDIO_FINGERPRINT_WORDS || right.len() > MAX_AUDIO_FINGERPRINT_WORDS {
        return Err(MediaCompareError::MalformedFingerprint(
            "audio fingerprint exceeds its absolute word ceiling".to_string(),
        ));
    }
    let shorter = left.len().min(right.len());
    if shorter < MIN_AUDIO_OVERLAP_WORDS {
        return Ok(None);
    }
    let required_overlap = MIN_AUDIO_OVERLAP_WORDS.max(shorter.div_ceil(2));
    let Some(best) = best_aligned_hamming(left, right, required_overlap) else {
        return Ok(None);
    };
    let average_milli = best.total_distance.saturating_mul(1_000) / best.overlap as u64;
    if best.overlap.saturating_mul(2) < shorter || average_milli > MAX_AUDIO_AVERAGE_HAMMING_MILLI {
        return Ok(None);
    }
    Ok(Some(ReviewCandidate {
        relation: ReviewRelation::AudioAcousticMatch,
        score_millionths: similarity_millionths(best.total_distance, best.overlap as u64 * 32),
        evidence: ComparisonEvidence::Audio {
            overlap_words: best.overlap as u32,
            shorter_fingerprint_words: shorter as u32,
            average_hamming_milli: average_milli as u32,
        },
        automatic_action: false,
    }))
}

fn compare_video(
    left: &[&str],
    right: &[&str],
) -> Result<Option<ReviewCandidate>, MediaCompareError> {
    if left.len() > MAX_VIDEO_KEYFRAMES || right.len() > MAX_VIDEO_KEYFRAMES {
        return Err(MediaCompareError::MalformedFingerprint(
            "video fingerprint exceeds its absolute keyframe ceiling".to_string(),
        ));
    }
    let left = left
        .iter()
        .map(|value| {
            parse_phash(value).ok_or_else(|| {
                MediaCompareError::MalformedFingerprint(
                    "invalid pHash in left video fingerprint".to_string(),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let right = right
        .iter()
        .map(|value| {
            parse_phash(value).ok_or_else(|| {
                MediaCompareError::MalformedFingerprint(
                    "invalid pHash in right video fingerprint".to_string(),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let shorter = left.len().min(right.len());
    if shorter < MIN_VIDEO_OVERLAP_FRAMES {
        return Ok(None);
    }
    let required_overlap = MIN_VIDEO_OVERLAP_FRAMES.max(shorter.div_ceil(2));
    let Some(best) = best_aligned_hamming(&left, &right, required_overlap) else {
        return Ok(None);
    };
    let average_milli = best.total_distance.saturating_mul(1_000) / best.overlap as u64;
    if best.overlap.saturating_mul(2) < shorter || average_milli > MAX_VIDEO_AVERAGE_HAMMING_MILLI {
        return Ok(None);
    }
    Ok(Some(ReviewCandidate {
        relation: ReviewRelation::VideoPerceptualMatch,
        score_millionths: similarity_millionths(best.total_distance, best.overlap as u64 * 64),
        evidence: ComparisonEvidence::Video {
            overlap_keyframes: best.overlap as u32,
            shorter_keyframes: shorter as u32,
            average_hamming_milli: average_milli as u32,
        },
        automatic_action: false,
    }))
}

trait HammingWord: Copy {
    fn distance(self, other: Self) -> u32;
}

impl HammingWord for u32 {
    fn distance(self, other: Self) -> u32 {
        (self ^ other).count_ones()
    }
}

impl HammingWord for u64 {
    fn distance(self, other: Self) -> u32 {
        (self ^ other).count_ones()
    }
}

#[derive(Debug, Clone, Copy)]
struct Alignment {
    total_distance: u64,
    overlap: usize,
}

fn best_aligned_hamming<T: HammingWord>(
    left: &[T],
    right: &[T],
    minimum_overlap: usize,
) -> Option<Alignment> {
    let mut best: Option<Alignment> = None;
    let minimum_offset = -(right.len() as isize) + minimum_overlap as isize;
    let maximum_offset = left.len() as isize - minimum_overlap as isize;
    for offset in minimum_offset..=maximum_offset {
        let left_start = offset.max(0) as usize;
        let right_start = (-offset).max(0) as usize;
        let overlap = (left.len() - left_start).min(right.len() - right_start);
        if overlap < minimum_overlap {
            continue;
        }
        let total_distance = (0..overlap)
            .map(|index| u64::from(left[left_start + index].distance(right[right_start + index])))
            .sum();
        let candidate = Alignment {
            total_distance,
            overlap,
        };
        let is_better = best.is_none_or(|current| {
            // Compare averages without floating point; prefer broader evidence
            // when averages are identical.
            candidate.total_distance * (current.overlap as u64)
                < current.total_distance * (candidate.overlap as u64)
                || (candidate.total_distance * (current.overlap as u64)
                    == current.total_distance * (candidate.overlap as u64)
                    && candidate.overlap > current.overlap)
        });
        if is_better {
            best = Some(candidate);
        }
    }
    best
}

fn similarity_millionths(distance: u64, possible: u64) -> u32 {
    if possible == 0 {
        return 0;
    }
    let distance = distance.min(possible);
    (((possible - distance) * 1_000_000) / possible) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_threshold_proposes_review_but_never_action() {
        let candidate = compare_images("0000000000000000", "0000000000000003")
            .unwrap()
            .unwrap();
        assert!(!candidate.automatic_action);
        assert!(compare_images("0000000000000000", "ffffffffffffffff")
            .unwrap()
            .is_none());
    }

    #[test]
    fn audio_alignment_tolerates_a_prefix() {
        let left = (0_u32..80).map(|value| value * 17).collect::<Vec<_>>();
        let mut right = vec![u32::MAX; 4];
        right.extend_from_slice(&left);
        let candidate = compare_audio(&left, &right).unwrap().unwrap();
        assert_eq!(candidate.relation, ReviewRelation::AudioAcousticMatch);
        assert_eq!(candidate.score_millionths, 1_000_000);
    }
}
