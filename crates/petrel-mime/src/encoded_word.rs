//! RFC 2047 encoded-word runs split mid-octet — except ISO-2022-JP.
//!
//! mail-parser decodes each `=?charset?B|Q?…?=` independently. When a UTF-8
//! character is folded across two adjacent Base64 words, the halves decode to
//! U+FFFD. Thunderbird concatenates the octets first; we rewrite the header
//! block the same way before handing bytes to mail-parser.
//!
//! ISO-2022-JP is the other way around. A Japanese mailer folds on a character
//! boundary and wraps each fragment as its own `ESC $ B` … `ESC ( B` run.
//! Concatenating those payloads puts `ESC ( B ESC $ B` in one stream. The
//! WHATWG decoder mail-parser uses (`encoding_rs`) treats an empty ASCII
//! stretch between escapes as an error and inserts U+FFFD at every join.
//! Those runs are decoded independently, then the Unicode is written back as
//! one UTF-8 word. A fold that actually splits a JIS character — the second
//! word has no designation — still concatenates octets first.

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, Engine};
use std::borrow::Cow;

const STANDARD_INDIF: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

fn is_fws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn charset_token(charset: &[u8]) -> &[u8] {
    charset.split(|&c| c == b'*').next().unwrap_or(charset)
}

fn charset_eq(a: &[u8], b: &[u8]) -> bool {
    charset_token(a).eq_ignore_ascii_case(charset_token(b))
}

fn is_iso2022_jp_family(charset: &[u8]) -> bool {
    let c = charset_token(charset);
    c.eq_ignore_ascii_case(b"ISO-2022-JP")
        || c.eq_ignore_ascii_case(b"ISO-2022-JP-1")
        || c.eq_ignore_ascii_case(b"ISO-2022-JP-2")
        || c.eq_ignore_ascii_case(b"ISO-2022-JP-3")
        || c.eq_ignore_ascii_case(b"ISO-2022-JP-2004")
        || c.eq_ignore_ascii_case(b"ISO-2022-JP-MS")
        || c.eq_ignore_ascii_case(b"CSISO2022JP")
        || c.eq_ignore_ascii_case(b"ISO2022-JP")
        || c.eq_ignore_ascii_case(b"ISO2022JP")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Iso2022State {
    Ascii,
    Roman,
    Kana,
    Jis,
}

fn iso2022_end_state(data: &[u8]) -> Iso2022State {
    let mut i = 0;
    let mut state = Iso2022State::Ascii;
    while i < data.len() {
        if data[i] != 0x1B {
            i += 1;
            continue;
        }
        let rest = &data[i..];
        if rest.starts_with(b"\x1b$B") || rest.starts_with(b"\x1b$@") || rest.starts_with(b"\x1b$A")
        {
            state = Iso2022State::Jis;
            i += 3;
            continue;
        }
        if rest.starts_with(b"\x1b$(D")
            || rest.starts_with(b"\x1b$(O")
            || rest.starts_with(b"\x1b$(P")
        {
            state = Iso2022State::Jis;
            i += 4;
            continue;
        }
        if rest.starts_with(b"\x1b(B") {
            state = Iso2022State::Ascii;
            i += 3;
            continue;
        }
        if rest.starts_with(b"\x1b(J") || rest.starts_with(b"\x1b(H") {
            state = Iso2022State::Roman;
            i += 3;
            continue;
        }
        if rest.starts_with(b"\x1b(I") {
            state = Iso2022State::Kana;
            i += 3;
            continue;
        }
        i += 1;
    }
    state
}

fn iso2022_should_cut(prev: &[u8], next: &[u8]) -> bool {
    iso2022_end_state(prev) != Iso2022State::Jis && next.first() == Some(&0x1B)
}

struct ParsedWord<'a> {
    start: usize,
    end: usize,
    charset: &'a [u8],
    encoding: u8,
    payload: &'a [u8],
}

fn try_parse_encoded_word(data: &[u8], at: usize) -> Option<ParsedWord<'_>> {
    if at + 2 > data.len() || &data[at..at + 2] != b"=?" {
        return None;
    }
    let mut i = at + 2;
    let charset_start = i;
    while i < data.len() && data[i] != b'?' {
        i += 1;
    }
    if i >= data.len() {
        return None;
    }
    let charset = &data[charset_start..i];
    if charset.is_empty() {
        return None;
    }
    i += 1;
    if i >= data.len() {
        return None;
    }
    let encoding = data[i].to_ascii_uppercase();
    if encoding != b'B' && encoding != b'Q' {
        return None;
    }
    i += 1;
    if i >= data.len() || data[i] != b'?' {
        return None;
    }
    i += 1;
    let payload_start = i;
    while i + 1 < data.len() {
        if data[i] == b'?' && data[i + 1] == b'=' {
            return Some(ParsedWord {
                start: at,
                end: i + 2,
                charset,
                encoding,
                payload: &data[payload_start..i],
            });
        }
        i += 1;
    }
    None
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decode_q_payload(payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < payload.len() {
        match payload[i] {
            b'_' => {
                out.push(0x20);
                i += 1;
            }
            b'=' if i + 2 < payload.len() => {
                let hi = hex_val(payload[i + 1])?;
                let lo = hex_val(payload[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'=' => return None,
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(out)
}

fn decode_word_payload(encoding: u8, payload: &[u8]) -> Option<Vec<u8>> {
    match encoding.to_ascii_uppercase() {
        b'B' => STANDARD_INDIF.decode(payload).ok(),
        b'Q' => decode_q_payload(payload),
        _ => None,
    }
}

fn skip_fws(data: &[u8], mut pos: usize) -> usize {
    while pos < data.len() && is_fws(data[pos]) {
        pos += 1;
    }
    pos
}

fn utf8_encoded_word(text: &str) -> Vec<u8> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = Vec::with_capacity(10 + b64.len());
    out.extend_from_slice(b"=?UTF-8?B?");
    out.extend_from_slice(b64.as_bytes());
    out.extend_from_slice(b"?=");
    out
}

fn merge_iso2022_jp_run(words: &[ParsedWord]) -> Option<Vec<u8>> {
    let mut groups: Vec<Vec<u8>> = Vec::new();
    for w in words {
        let payload = decode_word_payload(w.encoding, w.payload)?;
        match groups.last_mut() {
            Some(prev) if !iso2022_should_cut(prev, &payload) => prev.extend(payload),
            _ => groups.push(payload),
        }
    }
    let mut text = String::new();
    for group in &groups {
        let (cow, _, _) = encoding_rs::ISO_2022_JP.decode(group);
        text.push_str(&cow);
    }
    Some(utf8_encoded_word(&text))
}

fn merge_stateless_run(words: &[ParsedWord]) -> Option<Vec<u8>> {
    let mut merged = Vec::new();
    for w in words {
        merged.extend(decode_word_payload(w.encoding, w.payload)?);
    }
    let charset = words[0].charset;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&merged);
    let mut out = Vec::with_capacity(2 + charset.len() + 4 + b64.len() + 2);
    out.extend_from_slice(b"=?");
    out.extend_from_slice(charset);
    out.extend_from_slice(b"?B?");
    out.extend_from_slice(b64.as_bytes());
    out.extend_from_slice(b"?=");
    Some(out)
}

/// Decodes a run whose charset the Encoding Standard refuses, and writes the
/// text back as one UTF-8 word.
///
/// Each word is decoded on its own and the results joined. These encodings
/// announce their state per field in practice, and a word that carries its own
/// shift is complete; concatenating payloads first would be the ISO-2022-JP
/// mistake in another alphabet.
fn replacement_charset_run(words: &[ParsedWord]) -> Option<Vec<u8>> {
    let first = words.first()?;
    let label = std::str::from_utf8(first.charset).ok()?;
    if !crate::legacy_cjk::is_replacement_charset(label) {
        return None;
    }
    let mut text = String::new();
    for w in words {
        let payload = decode_word_payload(w.encoding, w.payload)?;
        let label = std::str::from_utf8(w.charset).ok()?;
        text.push_str(&crate::legacy_cjk::decode(label, &payload)?);
    }
    Some(utf8_encoded_word(&text))
}

fn merge_run(words: &[ParsedWord]) -> Option<Vec<u8>> {
    if words.is_empty() {
        return None;
    }
    if is_iso2022_jp_family(words[0].charset) {
        merge_iso2022_jp_run(words)
    } else {
        merge_stateless_run(words)
    }
}

fn rewrite_header_block(headers: &[u8]) -> Cow<'_, [u8]> {
    // Most messages have no split encoded-word run. Do not copy the header
    // block until a merge actually happens — ingest and reindex hit this on
    // every row.
    let mut out: Option<Vec<u8>> = None;
    let mut pos = 0;

    while pos < headers.len() {
        if let Some(first) = try_parse_encoded_word(headers, pos) {
            let mut run = vec![first];
            let mut scan = run[0].end;
            loop {
                scan = skip_fws(headers, scan);
                match try_parse_encoded_word(headers, scan) {
                    Some(next) if charset_eq(next.charset, run[0].charset) => {
                        scan = next.end;
                        run.push(next);
                    }
                    _ => break,
                }
            }

            let span_start = run[0].start;
            let span_end = run[run.len() - 1].end;
            // A charset the Encoding Standard answers with a single U+FFFD.
            // Decoded here and written back as UTF-8, because mail-parser is
            // about to hand these bytes to that same refusal. One word is
            // enough — unlike a split run, there is nothing to merge, the
            // whole field is simply lost without this.
            if let Some(rewritten) = replacement_charset_run(&run) {
                let buf = out.get_or_insert_with(|| headers[..span_start].to_vec());
                buf.extend_from_slice(&rewritten);
                pos = span_end;
                continue;
            }
            if run.len() >= 2
                && let Some(merged) = merge_run(&run)
            {
                let buf = out.get_or_insert_with(|| headers[..span_start].to_vec());
                buf.extend_from_slice(&merged);
                pos = span_end;
                continue;
            }
            if let Some(buf) = out.as_mut() {
                buf.extend_from_slice(&headers[span_start..span_end]);
            }
            pos = span_end;
            continue;
        }
        if let Some(buf) = out.as_mut() {
            buf.push(headers[pos]);
        }
        pos += 1;
    }

    match out {
        Some(buf) => Cow::Owned(buf),
        None => Cow::Borrowed(headers),
    }
}

/// Split mid-octet encoded-word runs in the top-level header block only.
pub(crate) fn merge_adjacent_encoded_words(raw: &[u8]) -> Cow<'_, [u8]> {
    let header_len = header_body_split(raw);
    let headers = &raw[..header_len];

    match rewrite_header_block(headers) {
        Cow::Borrowed(_) => Cow::Borrowed(raw),
        Cow::Owned(new_headers) => {
            let mut out = Vec::with_capacity(new_headers.len() + raw.len() - header_len);
            out.extend_from_slice(&new_headers);
            out.extend_from_slice(&raw[header_len..]);
            Cow::Owned(out)
        }
    }
}

fn header_body_split(raw: &[u8]) -> usize {
    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        return pos;
    }
    if let Some(pos) = raw.windows(2).position(|w| w == b"\n\n") {
        return pos;
    }
    raw.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_leaves_body_literal() {
        let raw = b"Subject: ok\r\n\r\n=?utf-8?B?5p2x5Lqs?=";
        let out = merge_adjacent_encoded_words(raw);
        assert!(matches!(out, Cow::Borrowed(_)), "no header merge, no copy");
        assert!(out.ends_with(b"=?utf-8?B?5p2x5Lqs?="));
    }

    #[test]
    fn adjacent_utf8_subject_words_become_one() {
        let raw = b"From: =?utf-8?B?5p2x5Lqs?= <tokyo@example.com>\r\n\
Subject: =?utf-8?B?5p2x5Lqs?= =?utf-8?B?6KiI55S7?=\r\n\r\n\
body\r\n";
        let out = merge_adjacent_encoded_words(raw);
        let text = String::from_utf8_lossy(&out);
        assert_eq!(
            text.matches("=?utf-8?B?5p2x5Lqs6KiI55S7?=").count(),
            1,
            "merged subject word appears once: {text}"
        );
        assert!(
            !text.contains("=?utf-8?B?6KiI55S7?="),
            "second subject word must be gone: {text}"
        );
    }

    #[test]
    fn self_contained_iso_2022_jp_words_become_one_utf8_word() {
        // Three complete JIS runs. Concatenating their payloads would put
        // ESC ( B ESC $ B in one stream and encoding_rs would insert U+FFFD.
        let raw = b"Subject: =?ISO-2022-JP?B?GyRCJDMkbCRPGyhC?=\r\n\
 =?ISO-2022-JP?B?GyRCJUYlOSVIGyhC?=\r\n\
 =?ISO-2022-JP?B?GyRCJEckORsoQg==?=\r\n\r\n\
body\r\n";
        let out = merge_adjacent_encoded_words(raw);
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("=?UTF-8?B?"),
            "self-contained JIS runs must be rewritten as UTF-8: {text}"
        );
        assert!(
            !text.contains("ISO-2022-JP"),
            "original JIS words must be gone: {text}"
        );
    }
}
