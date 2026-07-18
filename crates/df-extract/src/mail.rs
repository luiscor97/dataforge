use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use crate::formats::mime_from_extension;
use crate::normalize::sha256_hex;
use crate::ExtractionLimits;

#[derive(Debug)]
pub(crate) struct ParsedMail {
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub sent_at: Option<String>,
    pub subject: Option<String>,
    pub normalized_subject: Option<String>,
    pub body: String,
    pub attachments: Vec<MailAttachmentDraft>,
    pub limited: bool,
    pub metadata: Value,
}

#[derive(Debug)]
pub(crate) struct MailAttachmentDraft {
    pub ordinal: u32,
    pub file_name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// Absent when a configured byte budget blocks content extraction. Hash
    /// and inventory evidence are still retained.
    pub extractable_bytes: Option<Vec<u8>>,
}

pub(crate) fn parse_mail(bytes: &[u8], limits: &ExtractionLimits) -> Result<ParsedMail, String> {
    let message = MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| "EML parser rejected the message".to_string())?;
    let mut limited = false;
    let message_id = bounded_optional(message.message_id(), limits, &mut limited);
    let in_reply_to = bounded_header_list(message.in_reply_to(), limits, &mut limited);
    let references = bounded_header_list(message.references(), limits, &mut limited);
    let from = bounded_addresses(message.from(), limits, &mut limited);
    let to = bounded_addresses(message.to(), limits, &mut limited);
    let cc = bounded_addresses(message.cc(), limits, &mut limited);
    let sent_at = message.date().map(|date| date.to_rfc3339());
    let subject = bounded_optional(message.subject(), limits, &mut limited);
    let normalized_subject = bounded_optional(message.thread_name(), limits, &mut limited);
    let body = message
        .body_text(0)
        .map(|body| body.into_owned())
        .unwrap_or_default();
    if message
        .text_bodies()
        .chain(message.html_bodies())
        .any(|part| part.is_encoding_problem)
    {
        limited = true;
    }

    let attachment_count = u64::try_from(message.attachment_count()).unwrap_or(u64::MAX);
    if attachment_count > limits.max_mail_attachments {
        limited = true;
    }
    let accepted = usize::try_from(attachment_count.min(limits.max_mail_attachments))
        .map_err(|_| "mail attachment limit does not fit this platform".to_string())?;
    let mut attachment_bytes = 0_u64;
    let mut attachments = Vec::with_capacity(accepted);
    for (index, part) in message.attachments().take(accepted).enumerate() {
        let ordinal = u32::try_from(index).map_err(|_| "mail attachment ordinal overflow")?;
        let fallback = format!("attachment-{}", ordinal + 1);
        let raw_name = part.attachment_name().unwrap_or(&fallback);
        let file_name = bounded_value(raw_name, limits.max_metadata_chars, &mut limited);
        let mime = part
            .content_type()
            .map(|content_type| {
                format!(
                    "{}/{}",
                    content_type.ctype().to_ascii_lowercase(),
                    content_type
                        .subtype()
                        .unwrap_or("octet-stream")
                        .to_ascii_lowercase()
                )
            })
            .unwrap_or_else(|| mime_from_extension(&file_name));
        let contents = part.contents();
        let size_bytes = u64::try_from(contents.len())
            .map_err(|_| "mail attachment size overflow".to_string())?;
        let sha256 = sha256_hex(contents);
        let next_total = attachment_bytes.checked_add(size_bytes);
        let within_budget = !part.is_encoding_problem
            && size_bytes <= limits.max_attachment_bytes
            && next_total.is_some_and(|total| total <= limits.max_total_attachment_bytes);
        let extractable_bytes = if within_budget {
            attachment_bytes = next_total.expect("checked above");
            Some(contents.to_vec())
        } else {
            limited = true;
            None
        };
        attachments.push(MailAttachmentDraft {
            ordinal,
            file_name,
            mime,
            size_bytes,
            sha256,
            extractable_bytes,
        });
    }

    Ok(ParsedMail {
        message_id,
        in_reply_to,
        references,
        from,
        to,
        cc,
        sent_at,
        subject,
        normalized_subject,
        body,
        attachments,
        limited,
        metadata: json!({
            "attachment_count": attachment_count,
            "retained_attachments": accepted,
            "retained_attachment_bytes": attachment_bytes,
        }),
    })
}

fn bounded_optional(
    value: Option<&str>,
    limits: &ExtractionLimits,
    limited: &mut bool,
) -> Option<String> {
    value.map(|value| bounded_value(value, limits.max_metadata_chars, limited))
}

fn bounded_header_list(
    value: &HeaderValue<'_>,
    limits: &ExtractionLimits,
    limited: &mut bool,
) -> Vec<String> {
    let values = value.as_text_list().unwrap_or_default();
    if u64::try_from(values.len()).unwrap_or(u64::MAX) > limits.max_header_values {
        *limited = true;
    }
    values
        .iter()
        .take(usize::try_from(limits.max_header_values).expect("validated"))
        .map(|value| bounded_value(value, limits.max_metadata_chars, limited))
        .filter(|value| !value.is_empty())
        .collect()
}

fn bounded_addresses(
    addresses: Option<&Address<'_>>,
    limits: &ExtractionLimits,
    limited: &mut bool,
) -> Vec<String> {
    let Some(addresses) = addresses else {
        return Vec::new();
    };
    let count = addresses.iter().count();
    if u64::try_from(count).unwrap_or(u64::MAX) > limits.max_header_values {
        *limited = true;
    }
    addresses
        .iter()
        .take(usize::try_from(limits.max_header_values).expect("validated"))
        .filter_map(|address| match (address.name(), address.address()) {
            (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
            (None, Some(email)) => Some(email.to_string()),
            (Some(name), None) => Some(name.to_string()),
            (None, None) => None,
        })
        .map(|value| bounded_value(&value, limits.max_metadata_chars, limited))
        .filter(|value| !value.is_empty())
        .collect()
}

fn bounded_value(value: &str, max_chars: u64, limited: &mut bool) -> String {
    let maximum = usize::try_from(max_chars).expect("validated");
    let mut output = String::new();
    let mut pending_space = false;
    let mut count = 0_usize;
    for character in value.nfc() {
        if character.is_control() {
            continue;
        }
        let character = if character.is_whitespace() {
            pending_space = count > 0;
            continue;
        } else {
            character
        };
        if pending_space {
            if count == maximum {
                *limited = true;
                break;
            }
            output.push(' ');
            count += 1;
            pending_space = false;
        }
        if count == maximum {
            *limited = true;
            break;
        }
        output.push(character);
        count += 1;
    }
    output
}
