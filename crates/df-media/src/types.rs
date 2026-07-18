use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

pub const ANALYSIS_CONTRACT_VERSION: &str = "dataforge.media-analysis.v1";
pub const IMAGE_ALGORITHM_VERSION: &str = "dct-phash64-v1";
pub const AUDIO_ALGORITHM_VERSION: &str = "rusty-chromaprint-0.3.0-test2-v1";
pub const VIDEO_ALGORITHM_VERSION: &str = "sampled-dct-phash64-v1";

pub(crate) const AUDIO_SAMPLE_RATE: u32 = 11_025;
pub(crate) const VIDEO_FRAME_SIDE: usize = 32;
pub(crate) const VIDEO_FRAME_BYTES: usize = VIDEO_FRAME_SIDE * VIDEO_FRAME_SIDE;

const MIB: u64 = 1024 * 1024;
const HARD_MAX_INPUT_BYTES: u64 = 256 * MIB;
const HARD_MAX_IMAGE_PIXELS: u64 = 100_000_000;
const HARD_MAX_DURATION_SECONDS: u32 = 600;
const HARD_MAX_PCM_SAMPLES: u64 = AUDIO_SAMPLE_RATE as u64 * HARD_MAX_DURATION_SECONDS as u64;
const HARD_MAX_VIDEO_KEYFRAMES: u32 = 600;
const HARD_MIN_KEYFRAME_INTERVAL_MILLIS: u32 = 250;
const HARD_MAX_KEYFRAME_INTERVAL_MILLIS: u32 = 10_000;
const HARD_MIN_MEMORY_BYTES: u64 = 128 * MIB;
const HARD_MAX_MEMORY_BYTES: u64 = 2 * 1024 * MIB;
const HARD_MIN_TIMEOUT: Duration = Duration::from_secs(1);
const HARD_MAX_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaKind {
    Image,
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy)]
pub struct MediaRequest<'a> {
    pub kind: MediaKind,
    pub bytes: &'a [u8],
}

impl<'a> MediaRequest<'a> {
    #[must_use]
    pub const fn new(kind: MediaKind, bytes: &'a [u8]) -> Self {
        Self { kind, bytes }
    }
}

/// No executable is inferred. Each configured sidecar must be an absolute,
/// plain file and is leased by `df-process-safety` before it is launched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaSidecars {
    image_worker: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
}

impl MediaSidecars {
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_image_worker(mut self, executable: impl Into<PathBuf>) -> Self {
        self.image_worker = Some(executable.into());
        self
    }

    #[must_use]
    pub fn with_ffmpeg(mut self, executable: impl Into<PathBuf>) -> Self {
        self.ffmpeg = Some(executable.into());
        self
    }

    #[must_use]
    pub fn image_worker(&self) -> Option<&Path> {
        self.image_worker.as_deref()
    }

    #[must_use]
    pub fn ffmpeg(&self) -> Option<&Path> {
        self.ffmpeg.as_deref()
    }
}

/// Caller-selectable limits constrained by non-negotiable absolute ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaLimits {
    pub max_input_bytes: u64,
    pub max_image_pixels: u64,
    pub max_duration_seconds: u32,
    pub max_pcm_samples: u64,
    pub max_video_keyframes: u32,
    pub video_keyframe_interval_millis: u32,
    pub worker_memory_bytes: u64,
    pub worker_timeout_millis: u64,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * MIB,
            max_image_pixels: 40_000_000,
            max_duration_seconds: 300,
            max_pcm_samples: AUDIO_SAMPLE_RATE as u64 * 300,
            max_video_keyframes: 150,
            video_keyframe_interval_millis: 2_000,
            worker_memory_bytes: 512 * MIB,
            worker_timeout_millis: 60_000,
        }
    }
}

impl MediaLimits {
    pub(crate) fn validate(self) -> Result<Self, MediaError> {
        if self.max_input_bytes == 0 || self.max_input_bytes > HARD_MAX_INPUT_BYTES {
            return Err(MediaError::InvalidLimits(format!(
                "max_input_bytes must be between 1 and {HARD_MAX_INPUT_BYTES}"
            )));
        }
        if self.max_image_pixels == 0 || self.max_image_pixels > HARD_MAX_IMAGE_PIXELS {
            return Err(MediaError::InvalidLimits(format!(
                "max_image_pixels must be between 1 and {HARD_MAX_IMAGE_PIXELS}"
            )));
        }
        if self.max_duration_seconds == 0 || self.max_duration_seconds > HARD_MAX_DURATION_SECONDS {
            return Err(MediaError::InvalidLimits(format!(
                "max_duration_seconds must be between 1 and {HARD_MAX_DURATION_SECONDS}"
            )));
        }
        let duration_samples = AUDIO_SAMPLE_RATE as u64 * u64::from(self.max_duration_seconds);
        if self.max_pcm_samples == 0
            || self.max_pcm_samples > HARD_MAX_PCM_SAMPLES
            || self.max_pcm_samples > duration_samples
        {
            return Err(MediaError::InvalidLimits(format!(
                "max_pcm_samples must be between 1 and {duration_samples} for this duration"
            )));
        }
        if self.max_video_keyframes == 0 || self.max_video_keyframes > HARD_MAX_VIDEO_KEYFRAMES {
            return Err(MediaError::InvalidLimits(format!(
                "max_video_keyframes must be between 1 and {HARD_MAX_VIDEO_KEYFRAMES}"
            )));
        }
        if !(HARD_MIN_KEYFRAME_INTERVAL_MILLIS..=HARD_MAX_KEYFRAME_INTERVAL_MILLIS)
            .contains(&self.video_keyframe_interval_millis)
        {
            return Err(MediaError::InvalidLimits(format!(
                "video_keyframe_interval_millis must be between {HARD_MIN_KEYFRAME_INTERVAL_MILLIS} and {HARD_MAX_KEYFRAME_INTERVAL_MILLIS}"
            )));
        }
        if !(HARD_MIN_MEMORY_BYTES..=HARD_MAX_MEMORY_BYTES).contains(&self.worker_memory_bytes) {
            return Err(MediaError::InvalidLimits(format!(
                "worker_memory_bytes must be between {HARD_MIN_MEMORY_BYTES} and {HARD_MAX_MEMORY_BYTES}"
            )));
        }
        let timeout = Duration::from_millis(self.worker_timeout_millis);
        if !(HARD_MIN_TIMEOUT..=HARD_MAX_TIMEOUT).contains(&timeout) {
            return Err(MediaError::InvalidLimits(format!(
                "worker timeout must be between {} and {} milliseconds",
                HARD_MIN_TIMEOUT.as_millis(),
                HARD_MAX_TIMEOUT.as_millis()
            )));
        }
        Ok(self)
    }

    pub(crate) fn timeout(self) -> Duration {
        Duration::from_millis(self.worker_timeout_millis)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaStatus {
    Extracted,
    Limited,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureCode {
    InputLimit,
    PixelLimit,
    DurationLimit,
    OutputLimit,
    WorkerUnavailable,
    WorkerProtocol,
    DecoderRejected,
    MalformedMedia,
    InsufficientMedia,
    ResourceLimit,
    InternalWorker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlgorithmDescriptor {
    pub name: String,
    pub version: String,
    pub backend: String,
    pub config_digest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum MediaMetadata {
    Image(ImageMetadata),
    Audio(AudioMetadata),
    Video(VideoMetadata),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub pixel_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioMetadata {
    pub normalized_sample_rate_hz: u32,
    pub normalized_channels: u8,
    pub decoded_samples: u64,
    pub decoded_duration_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoMetadata {
    pub normalized_width: u32,
    pub normalized_height: u32,
    pub keyframe_interval_millis: u32,
    pub sampled_keyframes: u32,
    pub sampled_through_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum MediaFingerprint {
    Image(ImageFingerprint),
    Audio(AudioFingerprint),
    Video(VideoFingerprint),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageFingerprint {
    pub phash64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioFingerprint {
    /// Raw Chromaprint subfingerprints produced by the Test2 preset.
    pub subfingerprints: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoFingerprint {
    pub keyframes: Vec<VideoKeyframe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoKeyframe {
    pub index: u32,
    pub timestamp_millis: u64,
    pub phash64: String,
}

/// Serializable analysis contract. `automatic_action` is required to be
/// `false`, including when a result is deserialized from storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaAnalysis {
    pub contract_version: String,
    pub kind: MediaKind,
    pub status: MediaStatus,
    pub algorithm: AlgorithmDescriptor,
    pub metadata: Option<MediaMetadata>,
    pub fingerprint: Option<MediaFingerprint>,
    pub failure_code: Option<FailureCode>,
    pub note: Option<String>,
    #[serde(deserialize_with = "deserialize_false")]
    pub automatic_action: bool,
}

impl MediaAnalysis {
    pub(crate) fn new(
        kind: MediaKind,
        status: MediaStatus,
        algorithm: AlgorithmDescriptor,
        metadata: Option<MediaMetadata>,
        fingerprint: Option<MediaFingerprint>,
        failure_code: Option<FailureCode>,
        note: Option<&str>,
    ) -> Self {
        Self {
            contract_version: ANALYSIS_CONTRACT_VERSION.to_string(),
            kind,
            status,
            algorithm,
            metadata,
            fingerprint,
            failure_code,
            note: note.map(str::to_string),
            automatic_action: false,
        }
    }
}

fn deserialize_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if value {
        return Err(serde::de::Error::custom(
            "automatic_action must remain false",
        ));
    }
    Ok(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewRelation {
    ImagePerceptualMatch,
    AudioAcousticMatch,
    VideoPerceptualMatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum ComparisonEvidence {
    Image {
        hamming_distance: u32,
    },
    Audio {
        overlap_words: u32,
        shorter_fingerprint_words: u32,
        average_hamming_milli: u32,
    },
    Video {
        overlap_keyframes: u32,
        shorter_keyframes: u32,
        average_hamming_milli: u32,
    },
}

/// A non-destructive review proposal. It intentionally has no source path or
/// mutation primitive and cannot be deserialized with automatic action set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCandidate {
    pub relation: ReviewRelation,
    pub score_millionths: u32,
    pub evidence: ComparisonEvidence,
    #[serde(deserialize_with = "deserialize_false")]
    pub automatic_action: bool,
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("invalid media limits: {0}")]
    InvalidLimits(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MediaCompareError {
    #[error("cannot compare different media kinds")]
    KindMismatch,
    #[error("media analysis uses an unsupported contract version")]
    ContractMismatch,
    #[error("media analysis contains a mismatched fingerprint kind")]
    FingerprintMismatch,
    #[error("media fingerprint is malformed: {0}")]
    MalformedFingerprint(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_limit_ceiling_is_enforced() {
        assert!(MediaLimits {
            max_input_bytes: HARD_MAX_INPUT_BYTES + 1,
            ..MediaLimits::default()
        }
        .validate()
        .is_err());
        assert!(MediaLimits {
            max_pcm_samples: HARD_MAX_PCM_SAMPLES + 1,
            max_duration_seconds: HARD_MAX_DURATION_SECONDS,
            ..MediaLimits::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn automatic_action_true_is_rejected_on_deserialization() {
        let candidate = ReviewCandidate {
            relation: ReviewRelation::ImagePerceptualMatch,
            score_millionths: 1_000_000,
            evidence: ComparisonEvidence::Image {
                hamming_distance: 0,
            },
            automatic_action: false,
        };
        let mut value = serde_json::to_value(candidate).unwrap();
        value["automatic_action"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ReviewCandidate>(value).is_err());
    }
}
