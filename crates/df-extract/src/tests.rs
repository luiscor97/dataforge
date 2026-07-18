use std::io::{Cursor, Write};

use chrono::{TimeZone, Utc};
use df_domain::{ContentId, ExtractionStatus, RepresentationId, TextSubjectKind};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;

fn zip(entries: &[(&str, &[u8], CompressionMethod)]) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut cursor);
        for (name, bytes, method) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(*method),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    cursor.into_inner()
}

fn minimal_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn run(
    name: &str,
    mime: Option<&str>,
    bytes: &[u8],
    limits: &ExtractionLimits,
) -> ExtractionBundle {
    let digest = limits.digest().unwrap();
    let source_sha256 = sha256_hex(bytes);
    extract(
        ExtractionRequest {
            content_id: ContentId::new(),
            representation_id: RepresentationId::new(),
            source_sha256: &source_sha256,
            source_size_bytes: u64::try_from(bytes.len()).unwrap(),
            display_name: name,
            mime_hint: mime,
            bytes,
            extractor_version: EXTRACTOR_VERSION,
            config_digest: &digest,
            created_at: Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap(),
        },
        limits,
    )
    .unwrap()
}

#[test]
fn text_bundle_is_canonical_segmented_domain_evidence() {
    let limits = ExtractionLimits {
        text_segment_chars: 3,
        ..ExtractionLimits::default()
    };
    let bundle = run(
        "notes.txt",
        Some("text/plain"),
        "A\u{301}\r\n B C".as_bytes(),
        &limits,
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Extracted);
    assert_eq!(bundle.representation.normalized_chars, 5);
    assert_eq!(bundle.text_subjects.len(), 1);
    assert_eq!(bundle.text_subjects[0].kind, TextSubjectKind::Document);
    assert_eq!(
        bundle
            .text_segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>(),
        "Á\nB C"
    );
    assert_eq!(
        bundle
            .text_segments
            .iter()
            .map(|segment| (segment.char_start, segment.char_end))
            .collect::<Vec<_>>(),
        vec![(0, 3), (3, 5)]
    );
}

#[test]
fn pdf_dispatch_uses_the_fixed_backend_boundary() {
    let bytes = minimal_pdf("Hello PDF");
    let bundle = run(
        "document.pdf",
        Some("application/pdf"),
        &bytes,
        &ExtractionLimits::default(),
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Limited);
    assert_eq!(bundle.representation.format, DocumentFormat::Pdf);
    assert!(bundle.text_segments.is_empty());
    assert_eq!(bundle.representation.metadata["in_process"], false);
    assert_eq!(bundle.representation.metadata["isolation_required"], true);
}

#[test]
fn oversized_input_is_limited_before_dispatch() {
    let limits = ExtractionLimits {
        max_input_bytes: 3,
        ..ExtractionLimits::default()
    };
    let bundle = run("notes.txt", None, b"four", &limits);
    assert_eq!(bundle.representation.status, ExtractionStatus::Limited);
    assert_eq!(bundle.text_subjects.len(), 1);
    assert_eq!(bundle.text_subjects[0].kind, TextSubjectKind::Document);
    assert_eq!(bundle.representation.metadata["read_bytes"], 4);
    assert_eq!(
        bundle.representation.metadata["input_sha256"],
        sha256_hex(b"four")
    );
}

#[test]
fn physical_reader_allocates_only_the_bounded_prefix() {
    let limits = ExtractionLimits {
        max_input_bytes: 3,
        ..ExtractionLimits::default()
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.txt");
    let source = b"0123456789";
    std::fs::write(&path, source).unwrap();
    let bounded = read_bounded_file(&path, &limits).unwrap();
    assert_eq!(bounded.bytes, b"0123");
    assert_eq!(bounded.source_size_bytes, 10);

    let digest = limits.digest().unwrap();
    let source_sha256 = sha256_hex(source);
    let bundle = extract(
        ExtractionRequest {
            content_id: ContentId::new(),
            representation_id: RepresentationId::new(),
            source_sha256: &source_sha256,
            source_size_bytes: bounded.source_size_bytes,
            display_name: "large.txt",
            mime_hint: Some("text/plain"),
            bytes: &bounded.bytes,
            extractor_version: EXTRACTOR_VERSION,
            config_digest: &digest,
            created_at: Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap(),
        },
        &limits,
    )
    .unwrap();
    assert_eq!(bundle.representation.status, ExtractionStatus::Limited);
    assert_eq!(bundle.source_sha256, source_sha256);
}

#[test]
fn in_limit_bytes_must_match_the_canonical_source_digest() {
    let limits = ExtractionLimits::default();
    let digest = limits.digest().unwrap();
    let error = extract(
        ExtractionRequest {
            content_id: ContentId::new(),
            representation_id: RepresentationId::new(),
            source_sha256: &sha256_hex(b"different"),
            source_size_bytes: 4,
            display_name: "x.txt",
            mime_hint: None,
            bytes: b"same",
            extractor_version: EXTRACTOR_VERSION,
            config_digest: &digest,
            created_at: Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap(),
        },
        &limits,
    )
    .unwrap_err();
    assert!(matches!(error, DfError::Conflict(_)));
}

#[test]
fn eml_captures_thread_fields_body_hash_and_attachment_text() {
    let raw = b"From: Alice <alice@example.test>\r\n\
To: Bob <bob@example.test>\r\n\
Cc: Carol <carol@example.test>\r\n\
Date: Fri, 17 Jul 2026 10:00:00 +0200\r\n\
Message-ID: <m2@example.test>\r\n\
In-Reply-To: <m1@example.test>\r\n\
References: <m0@example.test> <m1@example.test>\r\n\
Subject: Re: Proyecto\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=bound\r\n\r\n\
--bound\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nCuerpo\r\n\
--bound\r\nContent-Type: text/plain; name=nota.txt\r\n\
Content-Disposition: attachment; filename=nota.txt\r\n\
Content-Transfer-Encoding: base64\r\n\r\nQWRqdW50byB0ZXh0bw0K\r\n\
--bound--\r\n";
    let bundle = run(
        "message.eml",
        Some("message/rfc822"),
        raw,
        &ExtractionLimits::default(),
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Extracted);
    let message = bundle.mail_message.unwrap();
    assert_eq!(message.message_id.as_deref(), Some("m2@example.test"));
    assert_eq!(message.in_reply_to, ["m1@example.test"]);
    assert_eq!(message.normalized_subject.as_deref(), Some("Proyecto"));
    assert_eq!(bundle.mail_attachments.len(), 1);
    assert_eq!(bundle.mail_attachments[0].file_name, "nota.txt");
    assert_eq!(bundle.text_subjects.len(), 2);
    assert!(bundle
        .text_segments
        .iter()
        .any(|segment| segment.text.contains("Adjunto texto")));
}

#[test]
fn safe_zip_is_virtual_and_never_materialized() {
    let bytes = zip(&[
        ("folder/readme.txt", b"hello", CompressionMethod::Stored),
        ("image.bin", &[0, 1, 2], CompressionMethod::Stored),
    ]);
    let outside = tempfile::tempdir().unwrap();
    let before = std::fs::read_dir(outside.path()).unwrap().count();
    let bundle = run(
        "bundle.zip",
        Some("application/zip"),
        &bytes,
        &ExtractionLimits::default(),
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Extracted);
    assert_eq!(bundle.archive_entries.len(), 2);
    assert_eq!(bundle.archive_entries[0].virtual_path, "folder/readme.txt");
    assert_eq!(bundle.text_subjects.len(), 2);
    assert_eq!(bundle.text_subjects[0].kind, TextSubjectKind::Document);
    assert_eq!(bundle.text_subjects[1].kind, TextSubjectKind::ArchiveEntry);
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), before);
}

#[test]
fn zip_traversal_and_illegal_names_are_blocked_during_preflight() {
    for name in [
        "../escape.txt",
        "safe/../../escape.txt",
        "dir\\escape.txt",
        "CON.txt",
    ] {
        let bytes = zip(&[(name, b"never read", CompressionMethod::Stored)]);
        let bundle = run("hostile.zip", None, &bytes, &ExtractionLimits::default());
        assert_eq!(
            bundle.representation.status,
            ExtractionStatus::Limited,
            "{name}"
        );
        assert!(bundle.archive_entries.is_empty(), "{name}");
        assert_eq!(bundle.text_subjects.len(), 1, "{name}");
        assert_eq!(bundle.text_subjects[0].kind, TextSubjectKind::Document);
    }
}

#[test]
fn zip_count_size_depth_and_ratio_limits_are_enforced() {
    let two = zip(&[
        ("a.txt", b"a", CompressionMethod::Stored),
        ("b.txt", b"b", CompressionMethod::Stored),
    ]);
    let count_limits = ExtractionLimits {
        max_archive_entries: 1,
        ..ExtractionLimits::default()
    };
    assert_eq!(
        run("x.zip", None, &two, &count_limits)
            .representation
            .status,
        ExtractionStatus::Limited
    );

    let large = zip(&[("large.txt", b"1234", CompressionMethod::Stored)]);
    let size_limits = ExtractionLimits {
        max_archive_entry_bytes: 3,
        ..ExtractionLimits::default()
    };
    assert_eq!(
        run("x.zip", None, &large, &size_limits)
            .representation
            .status,
        ExtractionStatus::Limited
    );

    let deep = zip(&[("a/b/c.txt", b"x", CompressionMethod::Stored)]);
    let depth_limits = ExtractionLimits {
        max_archive_path_depth: 2,
        ..ExtractionLimits::default()
    };
    assert_eq!(
        run("x.zip", None, &deep, &depth_limits)
            .representation
            .status,
        ExtractionStatus::Limited
    );

    let repetitive = vec![b'x'; 20_000];
    let compressed = zip(&[("bomb.txt", &repetitive, CompressionMethod::Deflated)]);
    let ratio_limits = ExtractionLimits {
        max_archive_compression_ratio: 2,
        ..ExtractionLimits::default()
    };
    assert_eq!(
        run("x.zip", None, &compressed, &ratio_limits)
            .representation
            .status,
        ExtractionStatus::Limited
    );
}

#[test]
fn zip_crc_corruption_is_detected_during_bounded_read() {
    let mut bytes = zip(&[("x.txt", b"unique-payload", CompressionMethod::Stored)]);
    let offset = bytes
        .windows(b"unique-payload".len())
        .position(|window| window == b"unique-payload")
        .unwrap();
    bytes[offset] ^= 0x01;
    let bundle = run("corrupt.zip", None, &bytes, &ExtractionLimits::default());
    assert_eq!(bundle.representation.status, ExtractionStatus::Limited);
    assert_eq!(bundle.archive_entries.len(), 1);
    assert_eq!(
        bundle.archive_entries[0].extraction_status,
        ExtractionStatus::Failed
    );
    assert_eq!(bundle.text_subjects.len(), 1);
    assert_eq!(bundle.text_subjects[0].kind, TextSubjectKind::Document);
}

#[test]
fn nested_zip_recursion_stops_at_the_configured_depth() {
    let inner = zip(&[("inside.txt", b"inside", CompressionMethod::Stored)]);
    let outer = zip(&[("inner.zip", &inner, CompressionMethod::Stored)]);
    let limits = ExtractionLimits {
        max_archive_nesting_depth: 1,
        ..ExtractionLimits::default()
    };
    let bundle = run("outer.zip", None, &outer, &limits);
    assert_eq!(bundle.representation.status, ExtractionStatus::Limited);
    assert_eq!(bundle.archive_entries.len(), 1);
    assert_eq!(
        bundle.archive_entries[0].extraction_status,
        ExtractionStatus::Limited
    );
}

#[test]
fn config_digest_and_validation_are_stable() {
    let config = ExtractionLimits::default();
    assert_eq!(config.digest().unwrap(), config.digest().unwrap());
    assert_eq!(config.digest().unwrap().len(), 64);
    let mut invalid = config;
    invalid.text_segment_chars = 0;
    assert!(invalid.validate().is_err());
}

#[test]
fn every_configured_limit_has_a_non_overridable_hard_ceiling() {
    macro_rules! assert_rejected {
        ($field:ident, $value:expr) => {{
            let mut limits = ExtractionLimits::default();
            limits.$field = $value;
            let error = limits.validate().unwrap_err().to_string();
            assert!(
                error.contains(stringify!($field)),
                "unexpected validation error for {}: {error}",
                stringify!($field)
            );
        }};
    }

    assert_rejected!(max_input_bytes, u64::MAX);
    assert_rejected!(max_input_bytes, 64 * 1024 * 1024 + 1);
    assert_rejected!(max_text_chars, 4_000_001);
    assert_rejected!(max_total_text_chars, 8_000_001);
    assert_rejected!(text_segment_chars, 64_001);
    assert_rejected!(max_metadata_chars, 4_097);
    assert_rejected!(max_header_values, 257);
    assert_rejected!(max_mail_attachments, 257);
    assert_rejected!(max_attachment_bytes, 16 * 1024 * 1024 + 1);
    assert_rejected!(max_total_attachment_bytes, 64 * 1024 * 1024 + 1);
    assert_rejected!(max_archive_entries, 10_001);
    assert_rejected!(max_archive_entry_bytes, 16 * 1024 * 1024 + 1);
    assert_rejected!(max_archive_total_bytes, 128 * 1024 * 1024 + 1);
    assert_rejected!(max_archive_compression_ratio, 101);
    assert_rejected!(max_archive_nesting_depth, 5);
    assert_rejected!(max_archive_path_depth, 33);
    assert_rejected!(max_virtual_path_chars, 4_097);
    assert_rejected!(html_render_width, 121);
}

#[test]
fn bundle_adapts_to_one_atomic_database_input() {
    let bundle = run(
        "notes.txt",
        Some("text/plain"),
        b"database evidence",
        &ExtractionLimits::default(),
    );
    let input = bundle.into_db_input().unwrap();
    assert_eq!(input.subjects.len(), 1);
    assert_eq!(input.subjects[0].segments.len(), 1);
    assert_eq!(input.subjects[0].segments[0].text, "database evidence");
}

#[test]
fn unsupported_content_has_the_database_compatible_empty_shape() {
    let bundle = run(
        "image.bin",
        Some("application/octet-stream"),
        &[0, 1, 2, 3],
        &ExtractionLimits::default(),
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Unsupported);
    assert_eq!(bundle.representation.format, DocumentFormat::Unsupported);
    assert!(bundle.representation.error.is_none());
    assert!(bundle.text_subjects.is_empty());
}
