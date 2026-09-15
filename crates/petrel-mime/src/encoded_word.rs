//! RFC 2047 encoded-word runs split mid-octet.
//!
//! mail-parser decodes each `=?charset?B|Q?…?=` independently. When a UTF-8
//! character is folded across two adjacent Base64 words, the halves decode to
//! U+FFFD. Thunderbird concatenates the octets first; we rewrite the header
//! block the same way before handing bytes to mail-parser.

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

fn charset_eq(a: &[u8], b: &[u8]) -> bool {
    let a = a.split(|&c| c == b'*').next().unwrap_or(a);
    let b = b.split(|&c| c == b'*').next().unwrap_or(b);
    a.eq_ignore_ascii_case(b)
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

fn merge_run(words: &[ParsedWord]) -> Option<Vec<u8>> {
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
}
