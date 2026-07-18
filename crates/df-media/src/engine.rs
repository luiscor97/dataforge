use std::ffi::OsString;
use std::panic::{catch_unwind, AssertUnwindSafe};

use df_process_safety::{run_isolated, ProcessLimits, ProcessSafetyError};
use rusty_chromaprint::{Configuration, Fingerprinter};
use sha2::{Digest, Sha256};

use crate::fingerprint::{parse_phash, phash_luma32};
use crate::types::{
    AlgorithmDescriptor, AudioFingerprint, AudioMetadata, FailureCode, ImageFingerprint,
    ImageMetadata, MediaAnalysis, MediaError, MediaFingerprint, MediaKind, MediaLimits,
    MediaMetadata, MediaRequest, MediaSidecars, MediaStatus, VideoFingerprint, VideoKeyframe,
    VideoMetadata, AUDIO_ALGORITHM_VERSION, AUDIO_SAMPLE_RATE, IMAGE_ALGORITHM_VERSION,
    VIDEO_ALGORITHM_VERSION, VIDEO_FRAME_BYTES, VIDEO_FRAME_SIDE,
};
use crate::worker_protocol::{
    encode_request, parse_response, ImageWorkerErrorCode, ImageWorkerResponse,
    IMAGE_WORKER_PROTOCOL_VERSION, MAX_IMAGE_WORKER_STDIN_BYTES, MAX_IMAGE_WORKER_STDOUT_BYTES,
};

const FFMPEG_VERSION_OUTPUT_LIMIT: u64 = 32 * 1024;

pub fn analyze_media(
    request: MediaRequest<'_>,
    limits: MediaLimits,
    sidecars: &MediaSidecars,
) -> Result<MediaAnalysis, MediaError> {
    let limits = limits.validate()?;
    if request.bytes.is_empty() {
        return Ok(failure(
            request.kind,
            descriptor_for(request.kind, limits, "not-run"),
            FailureCode::MalformedMedia,
            "media input is empty",
        ));
    }
    if u64::try_from(request.bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
        return Ok(limited(
            request.kind,
            descriptor_for(request.kind, limits, "not-run"),
            FailureCode::InputLimit,
            "media input exceeds the configured byte limit",
        ));
    }

    Ok(match request.kind {
        MediaKind::Image => analyze_image(request.bytes, limits, sidecars),
        MediaKind::Audio => analyze_audio(request.bytes, limits, sidecars),
        MediaKind::Video => analyze_video(request.bytes, limits, sidecars),
    })
}

fn analyze_image(input: &[u8], limits: MediaLimits, sidecars: &MediaSidecars) -> MediaAnalysis {
    let algorithm = descriptor_for(MediaKind::Image, limits, "image-0.25.10-sidecar");
    let Some(worker) = sidecars.image_worker() else {
        return failure(
            MediaKind::Image,
            algorithm,
            FailureCode::WorkerUnavailable,
            "the explicit image worker is not configured",
        );
    };
    let request = match encode_request(input, limits.max_image_pixels) {
        Ok(request) => request,
        Err(_) => {
            return limited(
                MediaKind::Image,
                algorithm,
                FailureCode::InputLimit,
                "the image worker request exceeds its absolute byte limit",
            )
        }
    };
    let response = match run_isolated(
        worker,
        &[],
        &request,
        ProcessLimits {
            timeout: limits.timeout(),
            memory_bytes: limits.worker_memory_bytes,
            max_stdin_bytes: MAX_IMAGE_WORKER_STDIN_BYTES,
            max_stdout_bytes: MAX_IMAGE_WORKER_STDOUT_BYTES,
        },
    ) {
        Ok(response) => response,
        Err(error) => return isolated_failure(MediaKind::Image, algorithm, &error),
    };
    match parse_response(&response) {
        Ok(ImageWorkerResponse::Ok {
            protocol_version,
            format,
            width,
            height,
            pixel_count,
            phash64,
        }) if protocol_version == IMAGE_WORKER_PROTOCOL_VERSION => {
            let dimensions_match = u64::from(width)
                .checked_mul(u64::from(height))
                .is_some_and(|expected| expected == pixel_count);
            if width == 0
                || height == 0
                || !dimensions_match
                || pixel_count > limits.max_image_pixels
                || !matches!(format.as_str(), "png" | "jpeg" | "webp")
                || parse_phash(&phash64).is_none()
            {
                return failure(
                    MediaKind::Image,
                    algorithm,
                    FailureCode::WorkerProtocol,
                    "the image worker returned values outside the requested contract",
                );
            }
            MediaAnalysis::new(
                MediaKind::Image,
                MediaStatus::Extracted,
                algorithm,
                Some(MediaMetadata::Image(ImageMetadata {
                    format,
                    width,
                    height,
                    pixel_count,
                })),
                Some(MediaFingerprint::Image(ImageFingerprint { phash64 })),
                None,
                None,
            )
        }
        Ok(ImageWorkerResponse::Error {
            protocol_version,
            code,
        }) if protocol_version == IMAGE_WORKER_PROTOCOL_VERSION => match code {
            ImageWorkerErrorCode::PixelLimit => limited(
                MediaKind::Image,
                algorithm,
                FailureCode::PixelLimit,
                "image dimensions exceed the configured pixel limit",
            ),
            ImageWorkerErrorCode::UnsupportedFormat => failure(
                MediaKind::Image,
                algorithm,
                FailureCode::DecoderRejected,
                "image format is unsupported",
            ),
            ImageWorkerErrorCode::MalformedImage => failure(
                MediaKind::Image,
                algorithm,
                FailureCode::MalformedMedia,
                "image decoder rejected malformed input",
            ),
            ImageWorkerErrorCode::InvalidRequest => failure(
                MediaKind::Image,
                algorithm,
                FailureCode::WorkerProtocol,
                "image worker rejected the versioned request",
            ),
            ImageWorkerErrorCode::Internal => failure(
                MediaKind::Image,
                algorithm,
                FailureCode::InternalWorker,
                "image worker failed internally",
            ),
        },
        Ok(_) | Err(_) => failure(
            MediaKind::Image,
            algorithm,
            FailureCode::WorkerProtocol,
            "image worker response violates the versioned protocol",
        ),
    }
}

fn analyze_audio(input: &[u8], limits: MediaLimits, sidecars: &MediaSidecars) -> MediaAnalysis {
    let Some(ffmpeg) = sidecars.ffmpeg() else {
        return failure(
            MediaKind::Audio,
            descriptor_for(MediaKind::Audio, limits, "not-run"),
            FailureCode::WorkerUnavailable,
            "the explicit FFmpeg sidecar is not configured",
        );
    };
    let backend = match ffmpeg_backend_version(ffmpeg, limits) {
        Ok(version) => format!("{version};rusty-chromaprint-0.3.0"),
        Err(error) => {
            return isolated_failure(
                MediaKind::Audio,
                descriptor_for(MediaKind::Audio, limits, "unavailable"),
                &error,
            )
        }
    };
    let algorithm = descriptor_for(MediaKind::Audio, limits, &backend);
    let sample_cap = limits.max_pcm_samples;
    let guarded_sample_cap = sample_cap.saturating_add(1);
    let filter = format!("aresample={AUDIO_SAMPLE_RATE},atrim=end_sample={guarded_sample_cap}");
    let arguments = os_args(&[
        "-hide_banner",
        "-loglevel",
        "error",
        "-xerror",
        "-err_detect",
        "explode",
        "-nostdin",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-protocol_whitelist",
        "cache,pipe",
        "-i",
        "cache:pipe:0",
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-ac",
        "1",
        "-af",
        &filter,
        "-c:a",
        "pcm_s16le",
        "-f",
        "s16le",
        "pipe:1",
    ]);
    let output_limit = guarded_sample_cap.saturating_mul(2);
    let pcm = match run_isolated(
        ffmpeg,
        &arguments,
        input,
        ProcessLimits {
            timeout: limits.timeout(),
            memory_bytes: limits.worker_memory_bytes,
            max_stdin_bytes: limits.max_input_bytes,
            max_stdout_bytes: output_limit,
        },
    ) {
        Ok(pcm) => pcm,
        Err(error) => return isolated_failure(MediaKind::Audio, algorithm, &error),
    };
    if pcm.is_empty() || pcm.len() % 2 != 0 {
        return failure(
            MediaKind::Audio,
            algorithm,
            FailureCode::MalformedMedia,
            "audio decoder returned no complete PCM samples",
        );
    }
    let mut samples = pcm
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    if u64::try_from(samples.len()).unwrap_or(u64::MAX) > guarded_sample_cap {
        return limited(
            MediaKind::Audio,
            algorithm,
            FailureCode::OutputLimit,
            "decoded audio exceeds the PCM sample ceiling",
        );
    }
    let hit_limit = samples.len() as u64 > sample_cap;
    samples.truncate(sample_cap as usize);
    let fingerprint = catch_unwind(AssertUnwindSafe(|| chromaprint(&samples)));
    let fingerprint = match fingerprint {
        Ok(Some(fingerprint)) if !fingerprint.is_empty() => fingerprint,
        Ok(_) => {
            return limited(
                MediaKind::Audio,
                algorithm,
                FailureCode::InsufficientMedia,
                "audio is too short to produce a Chromaprint fingerprint",
            )
        }
        Err(_) => {
            return failure(
                MediaKind::Audio,
                algorithm,
                FailureCode::InternalWorker,
                "bounded Chromaprint processing failed",
            )
        }
    };
    let decoded_samples = samples.len() as u64;
    let duration_millis = decoded_samples.saturating_mul(1_000) / u64::from(AUDIO_SAMPLE_RATE);
    MediaAnalysis::new(
        MediaKind::Audio,
        if hit_limit {
            MediaStatus::Limited
        } else {
            MediaStatus::Extracted
        },
        algorithm,
        Some(MediaMetadata::Audio(AudioMetadata {
            normalized_sample_rate_hz: AUDIO_SAMPLE_RATE,
            normalized_channels: 1,
            decoded_samples,
            decoded_duration_millis: duration_millis,
        })),
        Some(MediaFingerprint::Audio(AudioFingerprint {
            subfingerprints: fingerprint,
        })),
        hit_limit.then_some(FailureCode::DurationLimit),
        hit_limit.then_some("audio reached the configured duration/sample ceiling"),
    )
}

fn analyze_video(input: &[u8], limits: MediaLimits, sidecars: &MediaSidecars) -> MediaAnalysis {
    let Some(ffmpeg) = sidecars.ffmpeg() else {
        return failure(
            MediaKind::Video,
            descriptor_for(MediaKind::Video, limits, "not-run"),
            FailureCode::WorkerUnavailable,
            "the explicit FFmpeg sidecar is not configured",
        );
    };
    let backend = match ffmpeg_backend_version(ffmpeg, limits) {
        Ok(version) => version,
        Err(error) => {
            return isolated_failure(
                MediaKind::Video,
                descriptor_for(MediaKind::Video, limits, "unavailable"),
                &error,
            )
        }
    };
    let algorithm = descriptor_for(MediaKind::Video, limits, &backend);
    let filter = format!(
        "fps=1000/{interval},scale={side}:{side}:force_original_aspect_ratio=decrease:flags=bicubic,pad={side}:{side}:(ow-iw)/2:(oh-ih)/2:black",
        interval = limits.video_keyframe_interval_millis,
        side = VIDEO_FRAME_SIDE,
    );
    let duration = limits.max_duration_seconds.to_string();
    let guarded_frame_cap = limits.max_video_keyframes.saturating_add(1);
    let frames = guarded_frame_cap.to_string();
    let arguments = os_args(&[
        "-hide_banner",
        "-loglevel",
        "error",
        "-xerror",
        "-err_detect",
        "explode",
        "-nostdin",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-protocol_whitelist",
        "cache,pipe",
        "-i",
        "cache:pipe:0",
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
        "-t",
        &duration,
        "-vf",
        &filter,
        "-frames:v",
        &frames,
        "-pix_fmt",
        "gray",
        "-f",
        "rawvideo",
        "pipe:1",
    ]);
    let output_limit = u64::from(guarded_frame_cap).saturating_mul(VIDEO_FRAME_BYTES as u64);
    let raw = match run_isolated(
        ffmpeg,
        &arguments,
        input,
        ProcessLimits {
            timeout: limits.timeout(),
            memory_bytes: limits.worker_memory_bytes,
            max_stdin_bytes: limits.max_input_bytes,
            max_stdout_bytes: output_limit,
        },
    ) {
        Ok(raw) => raw,
        Err(error) => return isolated_failure(MediaKind::Video, algorithm, &error),
    };
    if raw.is_empty() || raw.len() % VIDEO_FRAME_BYTES != 0 {
        return failure(
            MediaKind::Video,
            algorithm,
            FailureCode::MalformedMedia,
            "video decoder returned no complete normalized frames",
        );
    }
    let frame_count = raw.len() / VIDEO_FRAME_BYTES;
    if frame_count > guarded_frame_cap as usize {
        return limited(
            MediaKind::Video,
            algorithm,
            FailureCode::OutputLimit,
            "decoded video exceeds the keyframe ceiling",
        );
    }
    let retained_frame_count = frame_count.min(limits.max_video_keyframes as usize);
    let mut keyframes = Vec::with_capacity(retained_frame_count);
    for (index, frame) in raw
        .chunks_exact(VIDEO_FRAME_BYTES)
        .take(retained_frame_count)
        .enumerate()
    {
        let Some(phash64) = phash_luma32(frame) else {
            return failure(
                MediaKind::Video,
                algorithm,
                FailureCode::InternalWorker,
                "normalized video frame has an invalid size",
            );
        };
        keyframes.push(VideoKeyframe {
            index: index as u32,
            timestamp_millis: (index as u64)
                .saturating_mul(u64::from(limits.video_keyframe_interval_millis)),
            phash64,
        });
    }
    let hit_frame_cap = frame_count > limits.max_video_keyframes as usize;
    let hit_duration_cap = match probe_video_duration(ffmpeg, input, limits) {
        Ok(hit) => hit,
        Err(error) => return isolated_failure(MediaKind::Video, algorithm, &error),
    };
    let hit_limit = hit_frame_cap || hit_duration_cap;
    let frame_count_u32 = retained_frame_count as u32;
    let sampled_through = keyframes
        .last()
        .map_or(0, |keyframe| keyframe.timestamp_millis);
    let (failure_code, note) = match (hit_frame_cap, hit_duration_cap) {
        (true, true) => (
            Some(FailureCode::OutputLimit),
            Some("video exceeds both duration and keyframe ceilings"),
        ),
        (true, false) => (
            Some(FailureCode::OutputLimit),
            Some("video exceeds the configured keyframe ceiling"),
        ),
        (false, true) => (
            Some(FailureCode::DurationLimit),
            Some("video exceeds the configured duration ceiling"),
        ),
        (false, false) => (None, None),
    };
    MediaAnalysis::new(
        MediaKind::Video,
        if hit_limit {
            MediaStatus::Limited
        } else {
            MediaStatus::Extracted
        },
        algorithm,
        Some(MediaMetadata::Video(VideoMetadata {
            normalized_width: VIDEO_FRAME_SIDE as u32,
            normalized_height: VIDEO_FRAME_SIDE as u32,
            keyframe_interval_millis: limits.video_keyframe_interval_millis,
            sampled_keyframes: frame_count_u32,
            sampled_through_millis: sampled_through,
        })),
        Some(MediaFingerprint::Video(VideoFingerprint { keyframes })),
        failure_code,
        note,
    )
}

fn probe_video_duration(
    ffmpeg: &std::path::Path,
    input: &[u8],
    limits: MediaLimits,
) -> Result<bool, ProcessSafetyError> {
    let duration = limits.max_duration_seconds.to_string();
    let filter = format!(
        "scale={side}:{side}:force_original_aspect_ratio=decrease:flags=bicubic,pad={side}:{side}:(ow-iw)/2:(oh-ih)/2:black",
        side = VIDEO_FRAME_SIDE,
    );
    let arguments = os_args(&[
        "-hide_banner",
        "-loglevel",
        "error",
        "-xerror",
        "-err_detect",
        "explode",
        "-nostdin",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-protocol_whitelist",
        "cache,pipe",
        "-i",
        "cache:pipe:0",
        "-ss",
        &duration,
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
        "-vf",
        &filter,
        "-frames:v",
        "1",
        "-pix_fmt",
        "gray",
        "-f",
        "rawvideo",
        "pipe:1",
    ]);
    let raw = run_isolated(
        ffmpeg,
        &arguments,
        input,
        ProcessLimits {
            timeout: limits.timeout(),
            memory_bytes: limits.worker_memory_bytes,
            max_stdin_bytes: limits.max_input_bytes,
            max_stdout_bytes: VIDEO_FRAME_BYTES as u64,
        },
    )?;
    match raw.len() {
        0 => Ok(false),
        VIDEO_FRAME_BYTES => Ok(true),
        _ => Err(ProcessSafetyError::Io(
            "FFmpeg duration probe returned a partial frame".to_string(),
        )),
    }
}

fn chromaprint(samples: &[i16]) -> Option<Vec<u32>> {
    let configuration = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&configuration);
    printer.start(AUDIO_SAMPLE_RATE, 1).ok()?;
    printer.consume(samples);
    printer.finish();
    Some(printer.fingerprint().to_vec())
}

fn ffmpeg_backend_version(
    executable: &std::path::Path,
    limits: MediaLimits,
) -> Result<String, ProcessSafetyError> {
    let output = run_isolated(
        executable,
        &os_args(&["-hide_banner", "-version"]),
        &[],
        ProcessLimits {
            timeout: limits.timeout(),
            memory_bytes: limits.worker_memory_bytes,
            max_stdin_bytes: 1,
            max_stdout_bytes: FFMPEG_VERSION_OUTPUT_LIMIT,
        },
    )?;
    let output = String::from_utf8(output)
        .map_err(|_| ProcessSafetyError::Io("FFmpeg version output is not UTF-8".to_string()))?;
    let first_line = output.lines().next().unwrap_or_default().trim();
    if !first_line.starts_with("ffmpeg version ")
        || first_line.len() > 256
        || !first_line.is_ascii()
    {
        return Err(ProcessSafetyError::Io(
            "FFmpeg version output is invalid".to_string(),
        ));
    }
    Ok(first_line.to_string())
}

pub(crate) fn descriptor_for(
    kind: MediaKind,
    limits: MediaLimits,
    backend: &str,
) -> AlgorithmDescriptor {
    let (name, version, canonical) = match kind {
        MediaKind::Image => (
            "DataForge DCT perceptual image hash",
            IMAGE_ALGORITHM_VERSION,
            format!(
                "kind=image;normalize=triangle-luma32;dct=fixed-q14-low8;max_pixels={}",
                limits.max_image_pixels
            ),
        ),
        MediaKind::Audio => (
            "Chromaprint Test2 acoustic fingerprint",
            AUDIO_ALGORITHM_VERSION,
            format!(
                "kind=audio;input=cache-pipe;decode=pcm_s16le;rate={AUDIO_SAMPLE_RATE};channels=1;preset=test2;max_samples={}",
                limits.max_pcm_samples
            ),
        ),
        MediaKind::Video => (
            "DataForge sampled video perceptual hash",
            VIDEO_ALGORITHM_VERSION,
            format!(
                "kind=video;input=cache-pipe;normalize=bicubic-pad-gray32;interval_ms={};max_frames={};max_duration_s={}",
                limits.video_keyframe_interval_millis,
                limits.max_video_keyframes,
                limits.max_duration_seconds
            ),
        ),
    };
    let mut digest = Sha256::new();
    digest.update(canonical.as_bytes());
    AlgorithmDescriptor {
        name: name.to_string(),
        version: version.to_string(),
        backend: backend.to_string(),
        config_digest_sha256: format!("{:x}", digest.finalize()),
    }
}

fn isolated_failure(
    kind: MediaKind,
    algorithm: AlgorithmDescriptor,
    error: &ProcessSafetyError,
) -> MediaAnalysis {
    match error {
        ProcessSafetyError::Timeout(_) | ProcessSafetyError::OutputLimit(_) => limited(
            kind,
            algorithm,
            FailureCode::ResourceLimit,
            "isolated decoder exceeded its resource contract",
        ),
        ProcessSafetyError::InvalidConfiguration(_)
        | ProcessSafetyError::UnsupportedPlatform(_)
        | ProcessSafetyError::Launch { .. }
        | ProcessSafetyError::Isolation(_) => failure(
            kind,
            algorithm,
            FailureCode::WorkerUnavailable,
            "explicit isolated decoder is unavailable",
        ),
        ProcessSafetyError::Exit(_) => failure(
            kind,
            algorithm,
            FailureCode::DecoderRejected,
            "isolated decoder rejected the media",
        ),
        ProcessSafetyError::Io(_) => failure(
            kind,
            algorithm,
            FailureCode::InternalWorker,
            "isolated decoder I/O failed",
        ),
    }
}

fn failure(
    kind: MediaKind,
    algorithm: AlgorithmDescriptor,
    code: FailureCode,
    note: &'static str,
) -> MediaAnalysis {
    MediaAnalysis::new(
        kind,
        MediaStatus::Failed,
        algorithm,
        None,
        None,
        Some(code),
        Some(note),
    )
}

fn limited(
    kind: MediaKind,
    algorithm: AlgorithmDescriptor,
    code: FailureCode,
    note: &'static str,
) -> MediaAnalysis {
    MediaAnalysis::new(
        kind,
        MediaStatus::Limited,
        algorithm,
        None,
        None,
        Some(code),
        Some(note),
    )
}

fn os_args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_and_missing_sidecars_fail_closed() {
        let limits = MediaLimits {
            max_input_bytes: 4,
            ..MediaLimits::default()
        };
        let overflow = analyze_media(
            MediaRequest::new(MediaKind::Image, b"12345"),
            limits,
            &MediaSidecars::none(),
        )
        .unwrap();
        assert_eq!(overflow.status, MediaStatus::Limited);
        assert_eq!(overflow.failure_code, Some(FailureCode::InputLimit));
        assert!(!overflow.automatic_action);

        let missing = analyze_media(
            MediaRequest::new(MediaKind::Audio, b"data"),
            MediaLimits::default(),
            &MediaSidecars::none(),
        )
        .unwrap();
        assert_eq!(missing.status, MediaStatus::Failed);
        assert_eq!(missing.failure_code, Some(FailureCode::WorkerUnavailable));
    }

    #[test]
    fn relative_sidecar_is_rejected_without_path_lookup() {
        let result = analyze_media(
            MediaRequest::new(MediaKind::Video, b"not-video"),
            MediaLimits::default(),
            &MediaSidecars::none().with_ffmpeg("ffmpeg.exe"),
        )
        .unwrap();
        assert_eq!(result.status, MediaStatus::Failed);
        assert_eq!(result.failure_code, Some(FailureCode::WorkerUnavailable));
    }
}
