//! End-to-end M0.5 drive: create → scan → hash → analyze → media run →
//! report, with the real isolated image worker.
//!
//! Two JPEG encodings of the same synthetic photo have different SHA-256
//! but near-identical perceptual hashes: exactly the rendition case the
//! milestone exists for. A third, unrelated image must not join them.

#![cfg(windows)]

use std::path::PathBuf;

use df_domain::Actor;
use df_facade::{CreateProjectRequest, MediaProjectOptions, MediaSidecars};
use image::codecs::jpeg::JpegEncoder;
use image::{Rgb, RgbImage};

fn worker_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_df-media-worker"))
}

fn photo() -> RgbImage {
    RgbImage::from_fn(320, 240, |x, y| {
        let vignette = ((x as i32 - 160).pow(2) + (y as i32 - 120).pow(2)) / 200;
        Rgb([
            (x as i32 / 2 + vignette).clamp(0, 255) as u8,
            (y as i32 + vignette / 2).clamp(0, 255) as u8,
            ((x ^ y) % 200) as u8,
        ])
    })
}

fn unrelated() -> RgbImage {
    RgbImage::from_fn(320, 240, |x, y| {
        Rgb([
            if (x / 20 + y / 20) % 2 == 0 { 235 } else { 15 },
            (x % 251) as u8,
            (y % 249) as u8,
        ])
    })
}

fn jpeg_bytes(image: &RgbImage, quality: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(image)
        .expect("in-memory JPEG encoding");
    bytes
}

#[test]
fn media_run_relates_renditions_and_seals_its_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origen");
    std::fs::create_dir_all(origin.join("fotos")).unwrap();

    // Same photo, two renditions; SHA-256 differs, pHash should not.
    let original = photo();
    std::fs::write(origin.join("fotos/viaje.jpg"), jpeg_bytes(&original, 92)).unwrap();
    std::fs::write(
        origin.join("fotos/viaje-comprimida.jpg"),
        jpeg_bytes(&original, 35),
    )
    .unwrap();
    std::fs::write(origin.join("tablero.jpg"), jpeg_bytes(&unrelated(), 90)).unwrap();
    std::fs::write(origin.join("notas.txt"), b"no soy un medio").unwrap();

    let project_dir = tmp.path().join("proyecto");
    df_facade::create_project(
        &CreateProjectRequest {
            name: "Prueba media".to_string(),
            project_dir: project_dir.clone(),
            output_root: tmp.path().join("salida"),
            audit_root: None,
            source_roots: vec![origin],
            profile: Some("generic".to_string()),
        },
        Actor::Test,
    )
    .expect("create");
    df_facade::scan_project(&project_dir, Actor::Test).expect("scan");
    df_facade::hash_project(&project_dir, Actor::Test).expect("hash");
    df_facade::analyze_project(&project_dir, Actor::Test).expect("analyze");

    let options = MediaProjectOptions {
        sidecars: MediaSidecars::none().with_image_worker(worker_path()),
        ..MediaProjectOptions::default()
    };
    let outcome = df_facade::analyze_media_with_options(&project_dir, Actor::Test, &options)
        .expect("media analysis");

    assert_eq!(outcome.status, "COMPLETED");
    assert!(!outcome.cancelled);
    assert!(outcome.evidence_only);
    assert_eq!(
        outcome.contents_total, 3,
        "three unique image contents; the text file is out of scope"
    );
    assert_eq!(outcome.contents_analyzed, 3);
    assert_eq!(outcome.contents_failed, 0);
    assert_eq!(outcome.pairs_compared, 3, "three images form three pairs");
    assert!(!outcome.pair_cap_reached);
    assert_eq!(
        outcome.relations, 1,
        "the two renditions relate; the checkerboard joins nobody"
    );

    // The report exposes the sealed run with display paths on both sides.
    let report = df_facade::media_report(&project_dir).expect("report");
    assert!(report.evidence_only);
    assert_eq!(report.status.run_id, outcome.run_id);
    assert_eq!(report.status.counters.relations_total, 1);
    let relation = &report.status.relations[0];
    assert_eq!(relation.relation, "IMAGE_PERCEPTUAL_MATCH");
    assert!(relation
        .path_a
        .as_deref()
        .is_some_and(|p| p.contains("viaje")));
    assert!(relation
        .path_b
        .as_deref()
        .is_some_and(|p| p.contains("viaje")));

    // Same configuration → the sealed run is returned as-is, not re-run.
    let again = df_facade::analyze_media_with_options(&project_dir, Actor::Test, &options)
        .expect("sealed reuse");
    assert_eq!(again.run_id, outcome.run_id);
    assert_eq!(again.status, "COMPLETED");
    assert_eq!(again.relations, 1);

    // The audit trail records start and completion and stays verifiable.
    let audit = df_facade::verify_audit(&project_dir).expect("audit");
    assert!(audit.ledger_ok);
}

#[test]
fn missing_image_worker_fails_closed_as_explicit_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origen");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::write(origin.join("foto.jpg"), jpeg_bytes(&photo(), 90)).unwrap();

    let project_dir = tmp.path().join("proyecto");
    df_facade::create_project(
        &CreateProjectRequest {
            name: "Sin worker".to_string(),
            project_dir: project_dir.clone(),
            output_root: tmp.path().join("salida"),
            audit_root: None,
            source_roots: vec![origin],
            profile: Some("generic".to_string()),
        },
        Actor::Test,
    )
    .expect("create");
    df_facade::scan_project(&project_dir, Actor::Test).expect("scan");
    df_facade::hash_project(&project_dir, Actor::Test).expect("hash");
    df_facade::analyze_project(&project_dir, Actor::Test).expect("analyze");

    // No sidecars at all: the image must fail with explicit evidence, the
    // run must still seal, and nothing may pretend to have been analysed.
    let outcome = df_facade::analyze_media_with_options(
        &project_dir,
        Actor::Test,
        &MediaProjectOptions {
            sidecars: MediaSidecars::none(),
            ..MediaProjectOptions::default()
        },
    )
    .expect("media analysis without workers");
    assert_eq!(outcome.status, "COMPLETED");
    assert_eq!(outcome.contents_total, 1);
    assert_eq!(outcome.contents_analyzed, 0);
    assert_eq!(outcome.contents_failed, 1);
    assert_eq!(outcome.relations, 0);
}
