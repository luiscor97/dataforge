#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::f64::consts::PI;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use df_media::{
    analyze_media, compare_media, FailureCode, MediaKind, MediaLimits, MediaRequest, MediaSidecars,
    MediaStatus,
};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use tempfile::TempDir;

fn worker_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_df-media-worker"))
}

fn ffmpeg_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        eprintln!("SKIP: df-process-safety currently isolates media workers only on Windows");
        return None;
    }
    let output = Command::new(r"C:\Windows\System32\where.exe")
        .arg("ffmpeg.exe")
        .output()
        .ok()?;
    if !output.status.success() {
        eprintln!("SKIP: ffmpeg.exe is not installed; audio/video integration test not run");
        return None;
    }
    let line = String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .to_string();
    let path = fs::canonicalize(line).ok()?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() {
        eprintln!("SKIP: discovered ffmpeg path is not a plain file");
        return None;
    }
    Some(path)
}

fn synthetic_image(variant: u8) -> RgbImage {
    RgbImage::from_fn(320, 240, |x, y| {
        let checker = (((x / 24) + (y / 24)) % 2) as u8;
        match variant {
            0 => Rgb([
                (x.wrapping_mul(3).wrapping_add(y)) as u8,
                (y.wrapping_mul(5).wrapping_add(x / 2)) as u8,
                checker.saturating_mul(170).saturating_add(40),
            ]),
            _ => Rgb([
                ((x ^ y).wrapping_mul(7)) as u8,
                if (x as i32 - 160).pow(2) + (y as i32 - 120).pow(2) < 3_600 {
                    250
                } else {
                    10
                },
                (255_u32.saturating_sub(y)) as u8,
            ]),
        }
    })
}

fn png_bytes(image: &RgbImage) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut cursor, ImageFormat::Png)
        .unwrap();
    cursor.into_inner()
}

fn resized_jpeg_bytes(image: &RgbImage) -> Vec<u8> {
    let resized = DynamicImage::ImageRgb8(image.clone()).resize_exact(
        176,
        132,
        image::imageops::FilterType::Lanczos3,
    );
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 58)
        .encode_image(&resized)
        .unwrap();
    bytes
}

#[test]
fn resized_recompressed_image_is_related_but_a_distinct_image_is_not() {
    if !cfg!(windows) {
        eprintln!("SKIP: image sidecar isolation is currently Windows-only");
        return;
    }
    let original = synthetic_image(0);
    let original_bytes = png_bytes(&original);
    let transformed_bytes = resized_jpeg_bytes(&original);
    let different_bytes = png_bytes(&synthetic_image(1));
    let sidecars = MediaSidecars::none().with_image_worker(worker_path());
    let limits = MediaLimits::default();

    let first = analyze_media(
        MediaRequest::new(MediaKind::Image, &original_bytes),
        limits,
        &sidecars,
    )
    .unwrap();
    let transformed = analyze_media(
        MediaRequest::new(MediaKind::Image, &transformed_bytes),
        limits,
        &sidecars,
    )
    .unwrap();
    let different = analyze_media(
        MediaRequest::new(MediaKind::Image, &different_bytes),
        limits,
        &sidecars,
    )
    .unwrap();

    assert_eq!(first.status, MediaStatus::Extracted);
    assert_eq!(transformed.status, MediaStatus::Extracted);
    assert_eq!(different.status, MediaStatus::Extracted);
    assert!(compare_media(&first, &transformed).unwrap().is_some());
    assert!(compare_media(&first, &different).unwrap().is_none());
    assert!(
        !compare_media(&first, &transformed)
            .unwrap()
            .unwrap()
            .automatic_action
    );

    let repeated = analyze_media(
        MediaRequest::new(MediaKind::Image, &original_bytes),
        limits,
        &sidecars,
    )
    .unwrap();
    assert_eq!(first, repeated, "image analysis must be deterministic");
}

#[test]
fn image_limits_missing_worker_and_malformed_input_fail_closed() {
    if !cfg!(windows) {
        eprintln!("SKIP: image sidecar isolation is currently Windows-only");
        return;
    }
    let bytes = png_bytes(&synthetic_image(0));
    let sidecars = MediaSidecars::none().with_image_worker(worker_path());
    let limited = analyze_media(
        MediaRequest::new(MediaKind::Image, &bytes),
        MediaLimits {
            max_image_pixels: 100,
            ..MediaLimits::default()
        },
        &sidecars,
    )
    .unwrap();
    assert_eq!(limited.status, MediaStatus::Limited);
    assert_eq!(limited.failure_code, Some(FailureCode::PixelLimit));

    let missing_path = TempDir::new().unwrap().path().join("missing-worker.exe");
    let missing = analyze_media(
        MediaRequest::new(MediaKind::Image, &bytes),
        MediaLimits::default(),
        &MediaSidecars::none().with_image_worker(missing_path),
    )
    .unwrap();
    assert_eq!(missing.status, MediaStatus::Failed);
    assert_eq!(missing.failure_code, Some(FailureCode::WorkerUnavailable));

    let malformed = analyze_media(
        MediaRequest::new(MediaKind::Image, b"\x89PNG\r\ntruncated"),
        MediaLimits::default(),
        &sidecars,
    )
    .unwrap();
    assert_eq!(malformed.status, MediaStatus::Failed);
    assert!(matches!(
        malformed.failure_code,
        Some(FailureCode::MalformedMedia | FailureCode::DecoderRejected)
    ));
}

#[test]
fn wav_and_transcoded_audio_share_a_real_chromaprint() {
    let Some(ffmpeg) = ffmpeg_path() else {
        return;
    };
    let temp = TempDir::new().unwrap();
    let wav = temp.path().join("source.wav");
    let mp3 = temp.path().join("transcoded.mp3");
    fs::write(&wav, synthetic_wav(18)).unwrap();
    assert!(run_ffmpeg(
        &ffmpeg,
        &[
            "-i",
            path_str(&wav),
            "-c:a",
            "libmp3lame",
            "-b:a",
            "80k",
            path_str(&mp3)
        ],
    ));
    let wav_bytes = fs::read(&wav).unwrap();
    let mp3_bytes = fs::read(&mp3).unwrap();
    let sidecars = MediaSidecars::none().with_ffmpeg(ffmpeg);

    let original = analyze_media(
        MediaRequest::new(MediaKind::Audio, &wav_bytes),
        MediaLimits::default(),
        &sidecars,
    )
    .unwrap();
    let transcoded = analyze_media(
        MediaRequest::new(MediaKind::Audio, &mp3_bytes),
        MediaLimits::default(),
        &sidecars,
    )
    .unwrap();
    assert_eq!(original.status, MediaStatus::Extracted);
    assert_eq!(transcoded.status, MediaStatus::Extracted);
    let candidate = compare_media(&original, &transcoded).unwrap().unwrap();
    assert!(candidate.score_millionths > 650_000);
    assert!(!candidate.automatic_action);
}

#[test]
fn generated_and_recompressed_video_share_sampled_keyframes() {
    let Some(ffmpeg) = ffmpeg_path() else {
        return;
    };
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source.mp4");
    let recompressed = temp.path().join("recompressed.mp4");
    assert!(run_ffmpeg(
        &ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=192x128:rate=12:duration=12",
            "-c:v",
            "mpeg4",
            "-q:v",
            "3",
            path_str(&source),
        ],
    ));
    assert!(run_ffmpeg(
        &ffmpeg,
        &[
            "-i",
            path_str(&source),
            "-vf",
            "scale=144:96",
            "-c:v",
            "mpeg4",
            "-q:v",
            "15",
            path_str(&recompressed),
        ],
    ));
    let source_bytes = fs::read(source).unwrap();
    let recompressed_bytes = fs::read(recompressed).unwrap();
    let sidecars = MediaSidecars::none().with_ffmpeg(ffmpeg);
    let first = analyze_media(
        MediaRequest::new(MediaKind::Video, &source_bytes),
        MediaLimits::default(),
        &sidecars,
    )
    .unwrap();
    let second = analyze_media(
        MediaRequest::new(MediaKind::Video, &recompressed_bytes),
        MediaLimits::default(),
        &sidecars,
    )
    .unwrap();
    assert_eq!(first.status, MediaStatus::Extracted);
    assert_eq!(second.status, MediaStatus::Extracted);
    let candidate = compare_media(&first, &second).unwrap().unwrap();
    assert!(candidate.score_millionths > 650_000);
    assert!(!candidate.automatic_action);
}

#[test]
fn malformed_audio_and_video_are_failed_not_interpreted_in_process() {
    let Some(ffmpeg) = ffmpeg_path() else {
        return;
    };
    let sidecars = MediaSidecars::none().with_ffmpeg(ffmpeg);
    for kind in [MediaKind::Audio, MediaKind::Video] {
        let result = analyze_media(
            MediaRequest::new(kind, b"attacker-controlled malformed bytes"),
            MediaLimits::default(),
            &sidecars,
        )
        .unwrap();
        assert_eq!(result.status, MediaStatus::Failed);
        assert_eq!(result.failure_code, Some(FailureCode::DecoderRejected));
        assert!(result.fingerprint.is_none());
        assert!(!result.automatic_action);
    }
}

#[test]
fn playlist_cannot_escape_the_cache_and_pipe_protocol_allowlist() {
    let Some(ffmpeg) = ffmpeg_path() else {
        return;
    };
    let playlist = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nhttp://127.0.0.1:9/forbidden.ts\n#EXT-X-ENDLIST\n";
    let result = analyze_media(
        MediaRequest::new(MediaKind::Video, playlist),
        MediaLimits::default(),
        &MediaSidecars::none().with_ffmpeg(ffmpeg),
    )
    .unwrap();
    assert_eq!(result.status, MediaStatus::Failed);
    assert_eq!(result.failure_code, Some(FailureCode::DecoderRejected));
    assert!(result.fingerprint.is_none());
}

fn run_ffmpeg(ffmpeg: &Path, arguments: &[&str]) -> bool {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(arguments)
        .env_clear()
        .output()
        .expect("explicit ffmpeg fixture command must launch");
    if !output.status.success() {
        eprintln!(
            "ffmpeg fixture generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output.status.success()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("temporary test path must be UTF-8")
}

fn synthetic_wav(duration_seconds: u32) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 44_100;
    let samples = SAMPLE_RATE * duration_seconds;
    let data_bytes = samples * 2;
    let mut wav = Vec::with_capacity(data_bytes as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());

    let notes = [220.0, 277.18, 329.63, 392.0, 493.88, 440.0, 349.23, 261.63];
    for index in 0..samples {
        let time = f64::from(index) / f64::from(SAMPLE_RATE);
        let note_index = (index / (SAMPLE_RATE / 2)) as usize % notes.len();
        let frequency = notes[note_index];
        let beat_phase = f64::from(index % (SAMPLE_RATE / 4)) / f64::from(SAMPLE_RATE / 4);
        let beat = (-8.0 * beat_phase).exp() * (2.0 * PI * 90.0 * time).sin();
        let value = 0.48 * (2.0 * PI * frequency * time).sin()
            + 0.20 * (2.0 * PI * frequency * 1.5 * time).sin()
            + 0.12 * beat;
        let sample = (value.clamp(-1.0, 1.0) * f64::from(i16::MAX)) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}
