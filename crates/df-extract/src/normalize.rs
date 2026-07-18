use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Canonical text plus evidence indicating whether the configured character
/// cap discarded output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedText {
    pub text: String,
    pub chars: u64,
    pub sha256: String,
    pub truncated: bool,
}

/// A database-ready segment using Unicode scalar offsets, never byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSegmentDraft {
    pub ordinal: u32,
    pub char_start: u64,
    pub char_end: u64,
    pub text: String,
    pub sha256: String,
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

/// Normalize extracted text to NFC and LF, remove controls, turn every other
/// whitespace class into ASCII space, collapse horizontal whitespace, trim
/// lines and retain at most one empty line between paragraphs.
///
/// The character limit applies to the canonical output. The function keeps at
/// most `max_chars` scalars plus constant state even when the input is larger.
pub fn normalize_text(input: &str, max_chars: u64) -> NormalizedText {
    debug_assert!(max_chars > 0);
    let mut output = String::new();
    let reserve = usize::try_from(max_chars.min(input.len() as u64)).unwrap_or(0);
    output.reserve(reserve);

    let mut stored = 0_u64;
    let mut truncated = false;
    let mut pending_space = false;
    let mut pending_newlines = 0_u8;
    let mut previous_was_cr = false;

    for character in input.nfc() {
        // CRLF is one LF; a bare CR is also LF.
        if character == '\n' && previous_was_cr {
            previous_was_cr = false;
            continue;
        }
        previous_was_cr = character == '\r';

        let canonical = match character {
            '\r' | '\n' => Some('\n'),
            '\t' => Some(' '),
            value if value.is_control() => None,
            value if value.is_whitespace() => Some(' '),
            value => Some(value),
        };
        let Some(canonical) = canonical else {
            continue;
        };

        if canonical == ' ' {
            if stored > 0 && pending_newlines == 0 {
                pending_space = true;
            }
            continue;
        }
        if canonical == '\n' {
            pending_space = false;
            if stored > 0 {
                pending_newlines = 2.min(pending_newlines.saturating_add(1));
            }
            continue;
        }

        if pending_newlines > 0 {
            for _ in 0..pending_newlines {
                if stored == max_chars {
                    truncated = true;
                    break;
                }
                output.push('\n');
                stored += 1;
            }
            pending_newlines = 0;
            pending_space = false;
        } else if pending_space {
            if stored == max_chars {
                truncated = true;
            } else {
                output.push(' ');
                stored += 1;
            }
            pending_space = false;
        }
        if stored == max_chars {
            truncated = true;
            break;
        }
        output.push(canonical);
        stored += 1;
    }

    // A boundary can leave only separators at the end. They carry no
    // searchable information and trimming them is part of the canonical form.
    while output.ends_with([' ', '\n']) {
        output.pop();
        stored -= 1;
    }

    NormalizedText {
        sha256: sha256_hex(output.as_bytes()),
        text: output,
        chars: stored,
        truncated,
    }
}

/// Split canonical UTF-8 at Unicode scalar boundaries and record exact scalar
/// offsets. Concatenating all segment text reconstructs the canonical input.
pub fn segment_text(text: &str, max_segment_chars: u64) -> Vec<TextSegmentDraft> {
    debug_assert!(max_segment_chars > 0);
    if text.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut segment_byte_start = 0_usize;
    let mut segment_char_start = 0_u64;
    let mut char_offset = 0_u64;

    for (byte_offset, _) in text.char_indices() {
        if char_offset > segment_char_start && char_offset - segment_char_start == max_segment_chars
        {
            let value = &text[segment_byte_start..byte_offset];
            segments.push(TextSegmentDraft {
                ordinal: u32::try_from(segments.len()).expect("validated segment count"),
                char_start: segment_char_start,
                char_end: char_offset,
                text: value.to_string(),
                sha256: sha256_hex(value.as_bytes()),
            });
            segment_byte_start = byte_offset;
            segment_char_start = char_offset;
        }
        char_offset += 1;
    }

    let value = &text[segment_byte_start..];
    segments.push(TextSegmentDraft {
        ordinal: u32::try_from(segments.len()).expect("validated segment count"),
        char_start: segment_char_start,
        char_end: char_offset,
        text: value.to_string(),
        sha256: sha256_hex(value.as_bytes()),
    });
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_nfc_line_endings_controls_and_spaces() {
        let text = normalize_text("  A\u{301}\r\n\tB\u{0000}\u{0085}  C\r\r\rD  ", 1_000);
        assert_eq!(text.text, "Á\nB C\n\nD");
        assert_eq!(text.chars, 8);
        assert!(!text.truncated);
        assert_eq!(text.sha256, sha256_hex(text.text.as_bytes()));
    }

    #[test]
    fn cap_is_in_unicode_scalars_and_output_is_trimmed() {
        let text = normalize_text("á β  γ", 4);
        assert_eq!(text.text, "á β");
        assert_eq!(text.chars, 3);
        assert!(text.truncated);
    }

    #[test]
    fn segmentation_never_splits_utf8_and_reconstructs_text() {
        let segments = segment_text("áβ🎉cd", 2);
        assert_eq!(
            segments
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "áβ🎉cd"
        );
        assert_eq!(
            segments
                .iter()
                .map(|part| (part.char_start, part.char_end))
                .collect::<Vec<_>>(),
            vec![(0, 2), (2, 4), (4, 5)]
        );
    }
}
