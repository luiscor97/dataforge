// The isolated-worker cases are Windows-only (Job Object); their helpers
// and imports would be dead code on POSIX, matching media_integration.rs.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::io::Write;
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::time::Duration;

use chrono::{TimeZone, Utc};
use df_domain::{ContentId, ExtractionStatus, RepresentationId, TextSubjectKind};
use df_extract::worker_protocol::{parse_response, write_request, WorkerStatus};
use df_extract::{
    extract_with_pdf_worker, ExtractionLimits, ExtractionRequest, PdfWorkerConfig,
    EXTRACTOR_VERSION,
};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn worker_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_df-extract-worker"))
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

fn extract_pdf(
    bytes: &[u8],
    limits: &ExtractionLimits,
    config: &PdfWorkerConfig,
) -> df_extract::ExtractionBundle {
    extract_named(
        "document.pdf",
        Some("application/pdf"),
        bytes,
        limits,
        config,
    )
}

fn extract_named(
    name: &str,
    mime: Option<&str>,
    bytes: &[u8],
    limits: &ExtractionLimits,
    config: &PdfWorkerConfig,
) -> df_extract::ExtractionBundle {
    let source_sha256 = format!("{:x}", Sha256::digest(bytes));
    let config_digest = limits.digest().unwrap();
    extract_with_pdf_worker(
        ExtractionRequest {
            content_id: ContentId::new(),
            representation_id: RepresentationId::new(),
            source_sha256: &source_sha256,
            source_size_bytes: u64::try_from(bytes.len()).unwrap(),
            display_name: name,
            mime_hint: mime,
            bytes,
            extractor_version: EXTRACTOR_VERSION,
            config_digest: &config_digest,
            created_at: Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap(),
        },
        limits,
        config,
    )
    .unwrap()
}

fn zip_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut output);
        for (name, bytes) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    output.into_inner()
}

fn eml_with_pdf(pdf: &[u8]) -> Vec<u8> {
    let mut message = b"From: sender@example.test\r\n\
To: receiver@example.test\r\n\
Subject: PDF attachment\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=dataforge-boundary\r\n\r\n\
--dataforge-boundary\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\n\
Mail body\r\n\
--dataforge-boundary\r\n\
Content-Type: application/pdf; name=inside.pdf\r\n\
Content-Disposition: attachment; filename=inside.pdf\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\n"
        .to_vec();
    message.extend_from_slice(pdf);
    message.extend_from_slice(b"\r\n--dataforge-boundary--\r\n");
    message
}

#[test]
fn standalone_worker_protocol_extracts_a_real_pdf() {
    let pdf = minimal_pdf("Hello isolated PDF");
    let mut child = Command::new(worker_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    write_request(child.stdin.take().unwrap(), &pdf, 1024 * 1024).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response = parse_response(&output.stdout, 1024 * 1024).unwrap();
    assert_eq!(response.status, WorkerStatus::Ok);
    assert!(String::from_utf8(response.payload)
        .unwrap()
        .contains("Hello isolated PDF"));
}

#[test]
fn standalone_worker_reports_rejection_and_output_limit_structurally() {
    let mut rejected = Command::new(worker_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    write_request(rejected.stdin.take().unwrap(), b"not a PDF", 1024).unwrap();
    let output = rejected.wait_with_output().unwrap();
    assert_eq!(
        parse_response(&output.stdout, 1024).unwrap().status,
        WorkerStatus::Rejected
    );

    let pdf = minimal_pdf("this output is longer than four bytes");
    let mut limited = Command::new(worker_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    write_request(limited.stdin.take().unwrap(), &pdf, 4).unwrap();
    let output = limited.wait_with_output().unwrap();
    assert_eq!(
        parse_response(&output.stdout, 4).unwrap().status,
        WorkerStatus::OutputLimit
    );
}

#[cfg(windows)]
#[test]
fn windows_client_extracts_under_a_job_object() {
    let bundle = extract_pdf(
        &minimal_pdf("Job Object PDF"),
        &ExtractionLimits::default(),
        &PdfWorkerConfig::new(worker_path()),
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Extracted);
    assert_eq!(bundle.representation.metadata["in_process"], false);
    assert_eq!(bundle.representation.metadata["isolated"], true);
    assert!(bundle
        .text_segments
        .iter()
        .any(|segment| segment.text.contains("Job Object PDF")));

    let malformed = extract_pdf(
        b"%PDF-malformed",
        &ExtractionLimits::default(),
        &PdfWorkerConfig::new(worker_path()),
    );
    assert_eq!(malformed.representation.status, ExtractionStatus::Failed);
    assert_eq!(
        malformed.representation.metadata["worker_status"],
        "REJECTED"
    );

    let tiny_limits = ExtractionLimits {
        max_text_chars: 1,
        text_segment_chars: 1,
        ..ExtractionLimits::default()
    };
    let output_limited = extract_pdf(
        &minimal_pdf("definitely longer than four bytes"),
        &tiny_limits,
        &PdfWorkerConfig::new(worker_path()),
    );
    assert_eq!(
        output_limited.representation.status,
        ExtractionStatus::Limited
    );
    assert_eq!(
        output_limited.representation.metadata["worker_status"],
        "OUTPUT_LIMIT"
    );
}

#[cfg(windows)]
#[test]
fn windows_client_isolates_pdf_inside_eml_and_zip() {
    let config = PdfWorkerConfig::new(worker_path());
    let pdf = minimal_pdf("Nested isolated PDF");
    let eml = extract_named(
        "mail.eml",
        Some("message/rfc822"),
        &eml_with_pdf(&pdf),
        &ExtractionLimits::default(),
        &config,
    );
    assert_eq!(eml.representation.status, ExtractionStatus::Extracted);
    assert_eq!(eml.mail_attachments.len(), 1);
    assert_eq!(
        eml.mail_attachments[0].extraction_status,
        ExtractionStatus::Extracted
    );
    assert!(eml
        .text_subjects
        .iter()
        .any(|subject| subject.kind == TextSubjectKind::MailAttachment
            && subject.metadata["worker_status"] == "OK"));
    assert!(eml
        .text_segments
        .iter()
        .any(|segment| segment.text.contains("Nested isolated PDF")));

    let archive_bytes = zip_entries(&[("inside.pdf", &pdf)]);
    let archive = extract_named(
        "bundle.zip",
        Some("application/zip"),
        &archive_bytes,
        &ExtractionLimits::default(),
        &config,
    );
    assert_eq!(archive.representation.status, ExtractionStatus::Extracted);
    assert_eq!(archive.archive_entries.len(), 1);
    assert_eq!(
        archive.archive_entries[0].extraction_status,
        ExtractionStatus::Extracted
    );
    assert!(archive
        .text_subjects
        .iter()
        .any(|subject| subject.kind == TextSubjectKind::ArchiveEntry
            && subject.metadata["worker_status"] == "OK"));
    assert!(archive
        .text_segments
        .iter()
        .any(|segment| segment.text.contains("Nested isolated PDF")));
}

#[cfg(windows)]
#[test]
fn windows_client_turns_timeout_and_bad_protocol_into_limited_evidence() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let timeout_worker = manifest.join("tests/fixtures/timeout-worker.cmd");
    let timed_out = extract_pdf(
        &minimal_pdf("deadline"),
        &ExtractionLimits::default(),
        &PdfWorkerConfig::new(timeout_worker).with_timeout(Duration::from_millis(100)),
    );
    assert_eq!(timed_out.representation.status, ExtractionStatus::Limited);
    assert!(timed_out
        .representation
        .error
        .as_deref()
        .is_some_and(|error| error.contains("deadline")));

    let invalid_worker = manifest.join("tests/fixtures/invalid-response-worker.cmd");
    let invalid = extract_pdf(
        &minimal_pdf("protocol"),
        &ExtractionLimits::default(),
        &PdfWorkerConfig::new(invalid_worker),
    );
    assert_eq!(invalid.representation.status, ExtractionStatus::Limited);
    assert!(invalid
        .representation
        .error
        .as_deref()
        .is_some_and(|error| error.contains("response")));

    let pdf = minimal_pdf("only nested PDF fails");
    let invalid_config =
        PdfWorkerConfig::new(manifest.join("tests/fixtures/invalid-response-worker.cmd"));
    let mail = extract_named(
        "mail.eml",
        Some("message/rfc822"),
        &eml_with_pdf(&pdf),
        &ExtractionLimits::default(),
        &invalid_config,
    );
    assert_eq!(mail.representation.status, ExtractionStatus::Limited);
    assert_eq!(
        mail.mail_attachments[0].extraction_status,
        ExtractionStatus::Limited
    );
    assert!(mail
        .text_subjects
        .iter()
        .any(|subject| subject.kind == TextSubjectKind::MailAttachment
            && subject.metadata["worker_status"] == "UNAVAILABLE"));
    assert!(mail
        .text_segments
        .iter()
        .any(|segment| segment.text.contains("Mail body")));

    let archive_bytes = zip_entries(&[("kept.txt", b"survives"), ("inside.pdf", &pdf)]);
    let archive = extract_named(
        "mixed.zip",
        Some("application/zip"),
        &archive_bytes,
        &ExtractionLimits::default(),
        &invalid_config,
    );
    assert_eq!(archive.representation.status, ExtractionStatus::Limited);
    assert_eq!(
        archive.archive_entries[0].extraction_status,
        ExtractionStatus::Extracted
    );
    assert_eq!(
        archive.archive_entries[1].extraction_status,
        ExtractionStatus::Limited
    );
    assert!(archive
        .text_subjects
        .iter()
        .any(|subject| subject.kind == TextSubjectKind::ArchiveEntry
            && subject.metadata["worker_status"] == "UNAVAILABLE"));
    assert!(archive
        .text_segments
        .iter()
        .any(|segment| segment.text.contains("survives")));

    let exhausted_limits = ExtractionLimits {
        max_text_chars: 8,
        max_total_text_chars: 8,
        text_segment_chars: 8,
        ..ExtractionLimits::default()
    };
    let exhausted_bytes = zip_entries(&[("first.txt", b"12345678"), ("inside.pdf", &pdf)]);
    let exhausted = extract_named(
        "budget.zip",
        Some("application/zip"),
        &exhausted_bytes,
        &exhausted_limits,
        &PdfWorkerConfig::new(worker_path()),
    );
    assert_eq!(exhausted.representation.status, ExtractionStatus::Limited);
    assert!(exhausted.text_subjects.iter().any(|subject| {
        subject.kind == TextSubjectKind::ArchiveEntry
            && subject.metadata["worker_status"] == "TEXT_BUDGET_EXHAUSTED"
    }));
}

#[cfg(not(windows))]
#[test]
fn client_fails_closed_without_a_supported_isolation_backend() {
    let bundle = extract_pdf(
        &minimal_pdf("never parsed"),
        &ExtractionLimits::default(),
        &PdfWorkerConfig::new(worker_path()),
    );
    assert_eq!(bundle.representation.status, ExtractionStatus::Limited);
    assert!(bundle.text_segments.is_empty());
}
