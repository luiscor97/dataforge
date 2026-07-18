use std::io::Cursor;
use std::path::Path;

use df_domain::DocumentFormat;
use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{json, Value};

use crate::zip_safety::ValidatedZip;
use crate::ExtractionLimits;

#[derive(Debug)]
pub(crate) struct ExtractedPayload {
    pub text: String,
    pub mime: String,
    pub metadata: Value,
    /// Conservative container expansion charged to the physical input.
    pub expanded_bytes: u64,
    /// Container directory entries inspected to produce this payload.
    pub container_entries: u64,
}

pub fn detect_format(display_name: &str, mime_hint: Option<&str>, bytes: &[u8]) -> DocumentFormat {
    let extension = extension(display_name);
    let mime = normalized_mime_hint(mime_hint);

    if bytes.starts_with(b"%PDF-") {
        return DocumentFormat::Pdf;
    }
    if is_zip_magic(bytes) {
        if extension == "docx"
            || mime.as_deref()
                == Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        {
            return DocumentFormat::Docx;
        }
        return DocumentFormat::Zip;
    }
    if extension == "pdf" || mime.as_deref() == Some("application/pdf") {
        return DocumentFormat::Pdf;
    }
    if extension == "docx" {
        return DocumentFormat::Docx;
    }
    if extension == "eml" || mime.as_deref() == Some("message/rfc822") {
        return DocumentFormat::Eml;
    }
    if matches!(extension.as_str(), "html" | "htm")
        || mime.as_deref() == Some("text/html")
        || looks_like_html(bytes)
    {
        return DocumentFormat::Html;
    }
    if matches!(
        extension.as_str(),
        "txt" | "text" | "md" | "csv" | "tsv" | "log" | "json" | "xml"
    ) || mime
        .as_deref()
        .is_some_and(|value| value.starts_with("text/"))
    {
        return DocumentFormat::Text;
    }
    DocumentFormat::Unsupported
}

pub(crate) fn extract_text_payload(
    format: DocumentFormat,
    display_name: &str,
    mime_hint: Option<&str>,
    bytes: &[u8],
    limits: &ExtractionLimits,
) -> Result<ExtractedPayload, String> {
    match format {
        DocumentFormat::Text => extract_plain_text(bytes, mime_hint),
        DocumentFormat::Html => extract_html(bytes, limits),
        DocumentFormat::Docx => extract_docx(bytes, limits),
        DocumentFormat::Pdf => Err(
            "PDF payload requires a resource-isolated worker; in-process parsing is disabled"
                .to_string(),
        ),
        DocumentFormat::Eml | DocumentFormat::Zip | DocumentFormat::Unsupported => Err(format!(
            "format {} is not a standalone text payload for `{}`",
            format.as_str(),
            bounded_label(display_name)
        )),
    }
}

fn extract_plain_text(bytes: &[u8], mime_hint: Option<&str>) -> Result<ExtractedPayload, String> {
    let (text, encoding) = decode_text(bytes)?;
    Ok(ExtractedPayload {
        text,
        mime: normalized_mime_hint(mime_hint).unwrap_or_else(|| "text/plain".to_string()),
        metadata: json!({"encoding": encoding}),
        expanded_bytes: 0,
        container_entries: 0,
    })
}

fn extract_html(bytes: &[u8], limits: &ExtractionLimits) -> Result<ExtractedPayload, String> {
    let (decoded, encoding) = decode_text(bytes)?;
    let width = usize::try_from(limits.html_render_width)
        .map_err(|_| "HTML render width does not fit this platform".to_string())?;
    let text = html2text::from_read(decoded.as_bytes(), width)
        .map_err(|error| format!("HTML rendering failed: {error}"))?;
    Ok(ExtractedPayload {
        text,
        mime: "text/html".to_string(),
        metadata: json!({"encoding": encoding, "render_width": limits.html_render_width}),
        expanded_bytes: 0,
        container_entries: 0,
    })
}

fn extract_docx(bytes: &[u8], limits: &ExtractionLimits) -> Result<ExtractedPayload, String> {
    if !is_zip_magic(bytes) {
        return Err("DOCX does not contain a ZIP package signature".to_string());
    }
    let mut package = ValidatedZip::open(
        bytes,
        limits,
        limits.max_archive_entries,
        limits.max_archive_total_bytes,
    )?;
    let document = package
        .entries
        .iter()
        .find(|entry| entry.name == "word/document.xml")
        .cloned()
        .ok_or_else(|| "DOCX package has no word/document.xml".to_string())?;
    if document.encrypted || document.directory {
        return Err("DOCX word/document.xml is not a readable file".to_string());
    }
    let xml = package.read_entry(&document)?;
    let text = extract_wordprocessing_text(&xml)?;
    Ok(ExtractedPayload {
        text,
        mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        metadata: json!({
            "package_entries": package.entries.len(),
            "package_declared_bytes": package.declared_bytes,
        }),
        expanded_bytes: package.declared_bytes,
        container_entries: u64::try_from(package.entries.len()).unwrap_or(u64::MAX),
    })
}

fn extract_wordprocessing_text(xml: &[u8]) -> Result<String, String> {
    let mut reader = Reader::from_reader(Cursor::new(xml));
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut output = String::new();
    let mut in_text = false;
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("DOCX XML is malformed: {error}"))?
        {
            Event::Start(start) => {
                let name = start.name();
                let local = local_name(name.as_ref());
                if local == b"t" {
                    in_text = true;
                } else if local == b"tab" {
                    output.push('\t');
                } else if local == b"br" || local == b"cr" {
                    output.push('\n');
                }
            }
            Event::Empty(empty) => {
                let name = empty.name();
                let local = local_name(name.as_ref());
                if local == b"tab" {
                    output.push('\t');
                } else if local == b"br" || local == b"cr" {
                    output.push('\n');
                }
            }
            Event::Text(text) if in_text => {
                let decoded = text
                    .xml10_content()
                    .map_err(|error| format!("DOCX text encoding is invalid: {error}"))?;
                let unescaped = quick_xml::escape::unescape(&decoded)
                    .map_err(|error| format!("DOCX text entity is invalid: {error}"))?;
                output.push_str(&unescaped);
            }
            Event::CData(text) if in_text => {
                output.push_str(
                    &text
                        .xml10_content()
                        .map_err(|error| format!("DOCX CDATA encoding is invalid: {error}"))?,
                );
            }
            Event::GeneralRef(reference) if in_text => {
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| format!("DOCX character reference is invalid: {error}"))?
                {
                    output.push(character);
                } else {
                    let name = reference
                        .decode()
                        .map_err(|error| format!("DOCX entity encoding is invalid: {error}"))?;
                    let value = quick_xml::escape::resolve_predefined_entity(&name)
                        .ok_or_else(|| "DOCX contains a non-predefined entity".to_string())?;
                    output.push_str(value);
                }
            }
            Event::End(end) => {
                let name = end.name();
                let local = local_name(name.as_ref());
                if local == b"t" {
                    in_text = false;
                } else if matches!(local, b"p" | b"tr") {
                    output.push('\n');
                }
            }
            Event::DocType(_) => return Err("DOCX XML document types are forbidden".to_string()),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(output)
}

fn decode_text(bytes: &[u8]) -> Result<(String, &'static str), String> {
    if let Some(without_bom) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(without_bom)
            .map(|text| (text.to_string(), "UTF-8"))
            .map_err(|_| "text declares UTF-8 BOM but contains invalid UTF-8".to_string());
    }
    if let Some(without_bom) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_without_replacement(UTF_16LE, without_bom, "UTF-16LE");
    }
    if let Some(without_bom) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_without_replacement(UTF_16BE, without_bom, "UTF-16BE");
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok((text.to_string(), "UTF-8"));
    }
    decode_without_replacement(WINDOWS_1252, bytes, "windows-1252")
}

fn decode_without_replacement(
    encoding: &'static encoding_rs::Encoding,
    bytes: &[u8],
    label: &'static str,
) -> Result<(String, &'static str), String> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|text| (text.into_owned(), label))
        .ok_or_else(|| format!("text is not valid {label}"))
}

pub(crate) fn canonical_mime(
    format: DocumentFormat,
    hint: Option<&str>,
    display_name: &str,
) -> String {
    normalized_mime_hint(hint).unwrap_or_else(|| match format {
        DocumentFormat::Pdf => "application/pdf".to_string(),
        DocumentFormat::Docx => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string()
        }
        DocumentFormat::Text => mime_from_extension(display_name),
        DocumentFormat::Html => "text/html".to_string(),
        DocumentFormat::Eml => "message/rfc822".to_string(),
        DocumentFormat::Zip => "application/zip".to_string(),
        DocumentFormat::Unsupported => "application/octet-stream".to_string(),
    })
}

pub(crate) fn mime_from_extension(name: &str) -> String {
    match extension(name).as_str() {
        "html" | "htm" => "text/html",
        "txt" | "text" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "eml" => "message/rfc822",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn normalized_mime_hint(hint: Option<&str>) -> Option<String> {
    let value = hint?.split(';').next()?.trim().to_ascii_lowercase();
    let (type_, subtype) = value.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    };
    (valid(type_) && valid(subtype)).then_some(value)
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub(crate) fn is_zip_magic(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..4),
        Some(b"PK\x03\x04") | Some(b"PK\x05\x06") | Some(b"PK\x07\x08")
    )
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(512)];
    let ascii = String::from_utf8_lossy(prefix).to_ascii_lowercase();
    let trimmed = ascii.trim_start_matches(|character: char| character.is_ascii_whitespace());
    trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<head")
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn bounded_label(value: &str) -> String {
    value.chars().take(128).collect()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    use super::*;

    #[test]
    fn format_detection_prefers_container_magic() {
        assert_eq!(
            detect_format("renamed.txt", Some("text/plain"), b"%PDF-1.7\n"),
            DocumentFormat::Pdf
        );
        assert_eq!(
            detect_format("report.docx", None, b"PK\x03\x04rest"),
            DocumentFormat::Docx
        );
    }

    #[test]
    fn text_decoder_supports_utf16_bom_without_replacement() {
        let bytes = [0xFF, 0xFE, b'H', 0, b'i', 0];
        let payload = extract_plain_text(&bytes, None).unwrap();
        assert_eq!(payload.text, "Hi");
        assert_eq!(payload.metadata["encoding"], "UTF-16LE");
    }

    #[test]
    fn html_is_rendered_to_text() {
        let payload = extract_html(
            b"<html><body><h1>Title</h1><p>Body</p></body></html>",
            &ExtractionLimits::default(),
        )
        .unwrap();
        assert!(payload.text.contains("Title"));
        assert!(payload.text.contains("Body"));
    }

    #[test]
    fn docx_reads_only_the_virtual_document_xml() {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut output);
            let options = SimpleFileOptions::default();
            writer.start_file("[Content_Types].xml", options).unwrap();
            writer.write_all(b"<Types/>").unwrap();
            writer.start_file("word/document.xml", options).unwrap();
            writer
                .write_all(br#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>A&amp;B</w:t></w:r><w:br/><w:r><w:t>fin</w:t></w:r></w:p></w:body></w:document>"#)
                .unwrap();
            writer.finish().unwrap();
        }
        let payload = extract_docx(output.get_ref(), &ExtractionLimits::default()).unwrap();
        assert_eq!(payload.text, "A&B\nfin\n");
    }
}
