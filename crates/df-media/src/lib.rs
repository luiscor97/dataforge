//! Bounded, review-only media intelligence for M0.5.
//!
//! The library never decodes attacker-controlled media in the caller. Images
//! are sent to the explicit `df-media-worker` sidecar; audio and video are
//! decoded by an explicit FFmpeg executable inside `df-process-safety`.
//! `PATH`, environment-variable discovery and in-process fallbacks are absent
//! by design.

#![forbid(unsafe_code)]

mod compare;
mod engine;
mod fingerprint;
mod types;
pub mod worker_protocol;

pub use compare::compare_media;
pub use engine::analyze_media;
pub use types::{
    AlgorithmDescriptor, AudioFingerprint, AudioMetadata, ComparisonEvidence, FailureCode,
    ImageFingerprint, ImageMetadata, MediaAnalysis, MediaCompareError, MediaError,
    MediaFingerprint, MediaKind, MediaLimits, MediaMetadata, MediaRequest, MediaSidecars,
    MediaStatus, ReviewCandidate, ReviewRelation, VideoFingerprint, VideoKeyframe, VideoMetadata,
    ANALYSIS_CONTRACT_VERSION, AUDIO_ALGORITHM_VERSION, IMAGE_ALGORITHM_VERSION,
    VIDEO_ALGORITHM_VERSION,
};
