use regex::Regex;

use crate::types::{sha256_hex, RedactionConfig, RedactionKind, RedactionRecord};

/// Deterministically redact configured privacy classes. Replacements contain
/// only a class and a short digest, so equal sensitive values remain
/// recognizable as equal without disclosing the value.
pub fn redact_text(text: &str, config: &RedactionConfig) -> (String, Vec<RedactionRecord>) {
    let mut visible = text.to_string();
    let mut records = Vec::new();

    let mut identifiers = config.identifiers.iter().collect::<Vec<_>>();
    identifiers
        .sort_unstable_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    for identifier in identifiers {
        if !identifier.is_empty() {
            let escaped = regex::escape(identifier);
            let matcher = Regex::new(&escaped).expect("escaped literal is a valid regex");
            visible = replace_matches(
                &visible,
                &matcher,
                RedactionKind::ExplicitIdentifier,
                |_| true,
                &mut records,
            );
        }
    }

    if config.redact_emails {
        let matcher = Regex::new(
            r"(?i)[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+",
        )
        .expect("static email regex is valid");
        visible = replace_matches(
            &visible,
            &matcher,
            RedactionKind::Email,
            |_| true,
            &mut records,
        );
    }

    if config.redact_phone_numbers {
        let matcher = Regex::new(r"\+?\d(?:[\d(). -]{5,}\d)").expect("static phone regex is valid");
        visible = replace_matches(
            &visible,
            &matcher,
            RedactionKind::Phone,
            |candidate| {
                let digits = candidate.bytes().filter(u8::is_ascii_digit).count();
                (7..=15).contains(&digits)
            },
            &mut records,
        );
    }

    if config.redact_paths {
        let matcher = Regex::new(
            r#"(?i)(?:[a-z]:[\\/]|\\\\)[^\s"'<>|?*]+|/(?:[a-z0-9._~-]+/)*[a-z0-9._~-]+"#,
        )
        .expect("static path regex is valid");
        visible = replace_matches(
            &visible,
            &matcher,
            RedactionKind::Path,
            |_| true,
            &mut records,
        );
    }

    (visible, records)
}

fn replace_matches(
    input: &str,
    matcher: &Regex,
    kind: RedactionKind,
    accept: impl Fn(&str) -> bool,
    records: &mut Vec<RedactionRecord>,
) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_until = 0;
    for matched in matcher.find_iter(input) {
        let original = matched.as_str();
        if !accept(original) {
            continue;
        }
        output.push_str(&input[copied_until..matched.start()]);
        let digest = sha256_hex(original.as_bytes());
        let replacement = format!("[REDACTED_{}:{}]", redaction_label(kind), &digest[..12]);
        let byte_start = output.len();
        output.push_str(&replacement);
        records.push(RedactionRecord {
            kind,
            byte_start,
            original_bytes: original.len(),
            original_sha256: digest,
            replacement,
        });
        copied_until = matched.end();
    }
    output.push_str(&input[copied_until..]);
    output
}

const fn redaction_label(kind: RedactionKind) -> &'static str {
    match kind {
        RedactionKind::ExplicitIdentifier => "IDENTIFIER",
        RedactionKind::Email => "EMAIL",
        RedactionKind::Phone => "PHONE",
        RedactionKind::Path => "PATH",
    }
}
