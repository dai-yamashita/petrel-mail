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

/// Whether the parser would fail to place this charset and read the bytes as
/// UTF-8 whatever they are.
fn charset_needs_mapping(charset: &[u8]) -> bool {
    std::str::from_utf8(charset_token(charset))
        .ok()
        .is_some_and(|label| !crate::charset::resolvable(label))
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
    iso2022_scan(data, 0, Iso2022State::Ascii)
}

/// The shift state at the end of `data`, reading from `from` in state
/// `start`: how a group's state is carried as words join it.
fn iso2022_scan(data: &[u8], from: usize, start: Iso2022State) -> Iso2022State {
    let mut i = from;
    let mut state = start;
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

fn iso2022_should_cut(prev_end: Iso2022State, next: &[u8]) -> bool {
    prev_end != Iso2022State::Jis && next.first() == Some(&0x1B)
}

/// The payloads of a run, joined where a word split a character and kept
/// apart where each carries its own shifts (see the module notes).
///
/// Each group keeps the state its bytes end in as words join it. Asked of
/// the whole group for every word, the state cost a rescan each time, so a
/// long run cost time in the square of its length: a crafted header of
/// ISO-2022-JP words froze the window for seconds. A word that joins is read
/// from three bytes before the join, the most of an escape sequence the
/// group can end in, so a sequence split across it reads as it did whole.
fn iso2022_groups(payloads: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut groups: Vec<(Vec<u8>, Iso2022State)> = Vec::new();
    for payload in payloads {
        match groups.last_mut() {
            Some((prev, end)) if !iso2022_should_cut(*end, &payload) => {
                let from = prev.len().saturating_sub(3);
                prev.extend(payload);
                *end = iso2022_scan(prev, from, *end);
            }
            _ => {
                let end = iso2022_end_state(&payload);
                groups.push((payload, end));
            }
        }
    }
    groups.into_iter().map(|(group, _)| group).collect()
}

struct ParsedWord<'a> {
    start: usize,
    end: usize,
    charset: &'a [u8],
    encoding: u8,
    payload: &'a [u8],
}

/// The encoded word that starts at `at`, if one does.
///
/// `last` is where the block's last `?=` starts, found once for the whole
/// block. A payload ends at the first `?=` after it, so one that starts past
/// `last` has no end, and is refused here instead of by a scan to the end of
/// the block. That scan, from every `=?x?Q?` in turn, was quadratic: a header
/// made of such fragments with no `?=` among them froze the window for 3.8 s
/// at 320 KB, every time the message was opened. A payload that has an end
/// finds the same `?=` it always did, so nothing decodes differently.
fn try_parse_encoded_word(data: &[u8], at: usize, last: Option<usize>) -> Option<ParsedWord<'_>> {
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
    let last = last.filter(|&l| l >= payload_start)?;
    while i <= last {
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
    let payloads = words
        .iter()
        .map(|w| decode_word_payload(w.encoding, w.payload))
        .collect::<Option<Vec<_>>>()?;
    let mut text = String::new();
    for group in &iso2022_groups(payloads) {
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
    // Writing the run back under a name the parser cannot resolve would leave
    // it to be read as UTF-8 further down, which turns the legacy bytes we
    // just carefully rejoined into replacement characters. Where the name is
    // one we can map, decode here and write UTF-8 back instead. Rejoining
    // first and decoding once is what makes this safe: a character split
    // across two encoded words is whole again by the time it is read.
    if let Ok(label) = std::str::from_utf8(charset)
        && !crate::charset::resolvable(label)
        && let Some(text) = crate::charset::decode(label, &merged)
    {
        return Some(utf8_encoded_word(&text));
    }
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
    // Where the block's last `?=` is; see `try_parse_encoded_word`.
    let last = headers.windows(2).rposition(|w| w == b"?=");

    while pos < headers.len() {
        if let Some(first) = try_parse_encoded_word(headers, pos, last) {
            let mut run = vec![first];
            let mut scan = run[0].end;
            loop {
                scan = skip_fws(headers, scan);
                match try_parse_encoded_word(headers, scan, last) {
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
            // Two or more words may have split a character between them, so
            // they are rejoined. A single word normally needs nothing — but if
            // its charset is a name the parser cannot resolve, leaving it
            // alone means it is read as UTF-8 and garbled, so it goes through
            // the same door to come back out as UTF-8.
            if (run.len() >= 2 || charset_needs_mapping(run[0].charset))
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

    /// The quadratic header: every `=?x?Q?` fragment scanned the rest of the
    /// block for a `?=` that is not there. 320 KB took 3.8 s in a release
    /// build, and about 640 KB froze the window for 15 s on every open.
    #[test]
    fn a_header_of_unterminated_words_is_read_in_one_pass() {
        let fragments = "=?x?Q?a".repeat(320 * 1024 / 7);
        let raw = format!("Subject: {fragments}\r\n\r\nbody\r\n");
        let started = std::time::Instant::now();
        let out = merge_adjacent_encoded_words(raw.as_bytes());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "nothing to merge, nothing copied"
        );
    }

    /// A run of ISO-2022-JP words in one folded header: each word used to
    /// rescan everything joined before it. Both shapes: plain words, and words
    /// that open in JIS, which never cut and so make one long group.
    #[test]
    fn a_long_run_of_iso_2022_jp_words_is_read_in_one_pass() {
        for word in [
            "=?ISO-2022-JP?Q?aaaaaaaaaaaaaaaaaa?=",
            "=?ISO-2022-JP?Q?=1B$B$\"?=",
        ] {
            let words = vec![word; 640 * 1024 / (word.len() + 3)].join("\r\n ");
            let raw = format!("Subject: {words}\r\n\r\nbody\r\n");
            let started = std::time::Instant::now();
            let out = merge_adjacent_encoded_words(raw.as_bytes());
            assert!(
                started.elapsed() < std::time::Duration::from_secs(1),
                "{word}: took {:?}",
                started.elapsed()
            );
            assert!(String::from_utf8_lossy(&out).contains("=?UTF-8?B?"));
        }
    }

    /// The groups kept as they grow are the groups a rescan of each would
    /// give, byte for byte: over many runs cut into words at random, from
    /// pieces that put escape sequences across the joins.
    #[test]
    fn carrying_the_shift_state_groups_as_rescanning_did() {
        fn by_rescan(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
            let mut groups: Vec<Vec<u8>> = Vec::new();
            for payload in payloads {
                match groups.last_mut() {
                    Some(prev) if !iso2022_should_cut(iso2022_end_state(prev), payload) => {
                        prev.extend(payload)
                    }
                    _ => groups.push(payload.clone()),
                }
            }
            groups
        }
        let pieces: [&[u8]; 14] = [
            b"\x1b",
            b"$",
            b"(",
            b"B",
            b"@",
            b"A",
            b"D",
            b"J",
            b"I",
            b"a",
            b"$B",
            b"(B",
            b"$(D",
            b"\x1b$B$\"",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..20_000 {
            let words: Vec<Vec<u8>> = (0..1 + next(6))
                .map(|_| {
                    (0..next(5))
                        .flat_map(|_| pieces[next(pieces.len())].to_vec())
                        .collect()
                })
                .collect();
            assert_eq!(
                iso2022_groups(words.clone()),
                by_rescan(&words),
                "{words:?}"
            );
        }
    }

    #[test]
    fn a_word_ending_at_the_last_terminator_still_parses() {
        // The last `?=` closes the second word; the fragment after it has no
        // end and is left as text, as it always was.
        let block = b"=?utf-8?B?5p2x5Lqs?= =?utf-8?B?6KiI55S7?= =?utf-8?B?tail";
        let last = block.windows(2).rposition(|w| w == b"?=");
        let second = try_parse_encoded_word(block, 21, last).expect("the second word");
        assert_eq!(second.payload, b"6KiI55S7");
        assert!(try_parse_encoded_word(block, 42, last).is_none());
        let empty = b"=?utf-8?B??=";
        let last = empty.windows(2).rposition(|w| w == b"?=");
        assert_eq!(
            try_parse_encoded_word(empty, 0, last).map(|w| w.payload.len()),
            Some(0)
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
