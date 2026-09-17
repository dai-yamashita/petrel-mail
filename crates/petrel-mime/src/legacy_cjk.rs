//! The four CJK charsets the Encoding Standard refuses to decode.
//!
//! `encoding_rs` implements the WHATWG Encoding Standard, which maps
//! ISO-2022-KR, ISO-2022-CN, ISO-2022-CN-EXT and HZ-GB-2312 to an encoding
//! named "replacement": whatever the input, the output is one U+FFFD. That is
//! deliberate and correct for a browser, where these encodings let an attacker
//! smuggle markup past a filter that read the bytes as something else. A mail
//! client is not a browser. It has to render mail that was archived before the
//! decision was made, and ISO-2022-KR was the registered charset for Korean
//! mail for years. Refusing it does not protect anyone here; it erases the
//! message.
//!
//! So these four are decoded here instead. Not by carrying character tables —
//! all three of the scripts involved are already in `encoding_rs`, in their
//! eight-bit forms. Each of these encodings is the *same* table shifted into
//! seven bits with escape sequences to say where the shifted stretches are. So
//! the work is a state machine over the escapes, and each double-byte pair is
//! handed to `EUC_KR` or `GBK` with the high bits put back.
//!
//! What is not carried is CNS 11643, the Traditional Chinese set ISO-2022-CN
//! can name. There is no table for it here and it is not worth adding for an
//! encoding this rare: those stretches become U+FFFD, one per character, while
//! the ASCII and GB2312 parts of the same message still read. Losing part of a
//! message beats losing all of it.
//!
//! Every byte is hostile input. The machines below index only through `get`
//! and slice patterns, advance on every branch, and allocate in proportion to
//! the input — there is no path that can panic or fail to terminate.

/// Whether this is a charset the Encoding Standard answers with U+FFFD.
pub fn is_replacement_charset(label: &str) -> bool {
    let label = label.trim().trim_matches('"');
    // The language tag RFC 2231 allows on a charset is not part of the name.
    let label = label.split('*').next().unwrap_or(label);
    [
        "iso-2022-kr",
        "iso2022-kr",
        "iso2022kr",
        "csiso2022kr",
        "iso-2022-cn",
        "iso2022-cn",
        "iso-2022-cn-ext",
        "csiso2022cn",
        "hz-gb-2312",
        "hz-gb2312",
        "hz",
    ]
    .iter()
    .any(|known| label.eq_ignore_ascii_case(known))
}

/// Decodes one of those four, or `None` when the label is not one of them.
pub fn decode(label: &str, bytes: &[u8]) -> Option<String> {
    if !is_replacement_charset(label) {
        return None;
    }
    let label = label.trim().trim_matches('"');
    let label = label.split('*').next().unwrap_or(label);
    if label.eq_ignore_ascii_case("hz-gb-2312")
        || label.eq_ignore_ascii_case("hz-gb2312")
        || label.eq_ignore_ascii_case("hz")
    {
        Some(decode_hz(bytes))
    } else if label.to_ascii_lowercase().contains("kr") {
        Some(decode_iso2022_kr(bytes))
    } else {
        Some(decode_iso2022_cn(bytes))
    }
}

/// A byte outside a shifted stretch. Only ASCII is meaningful there; anything
/// with the high bit set is a byte this encoding cannot be carrying, and
/// saying so beats inventing a Latin-1 character for it.
fn push_unshifted(out: &mut String, b: u8) {
    if b.is_ascii() {
        out.push(b as char);
    } else {
        out.push('\u{FFFD}');
    }
}

/// Seven-bit pairs, with their high bits put back, through a real table.
fn flush(run: &mut Vec<u8>, out: &mut String, encoding: &'static encoding_rs::Encoding) {
    if run.is_empty() {
        return;
    }
    let (text, _, _) = encoding.decode(run);
    out.push_str(&text);
    run.clear();
}

fn is_septet(b: u8) -> bool {
    (0x21..=0x7E).contains(&b)
}

/// ISO-2022-KR, RFC 1557.
///
/// `ESC $ ) C` announces KS X 1001 once, then SO and SI switch in and out of
/// it. The announcement is traditionally in the first header of the message and
/// need not be repeated, so its absence is not treated as a reason to refuse:
/// SO means the same thing either way.
fn decode_iso2022_kr(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut run: Vec<u8> = Vec::new();
    let mut shifted = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x1B => {
                // The one designator this encoding defines; any other escape
                // is not ours to act on, and is dropped rather than shown.
                if bytes[i..].starts_with(b"\x1b$)C") {
                    i += 4;
                } else {
                    i += 1;
                }
            }
            0x0E => {
                shifted = true;
                i += 1;
            }
            0x0F => {
                flush(&mut run, &mut out, encoding_rs::EUC_KR);
                shifted = false;
                i += 1;
            }
            b'\r' | b'\n' => {
                // A line ends in ASCII. Folded headers rely on it.
                flush(&mut run, &mut out, encoding_rs::EUC_KR);
                shifted = false;
                out.push(b as char);
                i += 1;
            }
            _ if shifted => match (is_septet(b), bytes.get(i + 1).copied()) {
                (true, Some(next)) if is_septet(next) => {
                    run.push(b | 0x80);
                    run.push(next | 0x80);
                    i += 2;
                }
                _ => {
                    flush(&mut run, &mut out, encoding_rs::EUC_KR);
                    push_unshifted(&mut out, b);
                    i += 1;
                }
            },
            _ => {
                push_unshifted(&mut out, b);
                i += 1;
            }
        }
    }
    flush(&mut run, &mut out, encoding_rs::EUC_KR);
    out
}

/// HZ-GB-2312, RFC 1843.
///
/// `~{` and `~}` bracket the GB2312 stretches, `~~` is a literal tilde, and a
/// tilde before a newline is a line continuation that carries no text.
fn decode_hz(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut run: Vec<u8> = Vec::new();
    let mut gb = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'~' {
            match bytes.get(i + 1) {
                Some(b'{') if !gb => {
                    gb = true;
                    i += 2;
                }
                Some(b'}') if gb => {
                    flush(&mut run, &mut out, encoding_rs::GBK);
                    gb = false;
                    i += 2;
                }
                Some(b'~') => {
                    flush(&mut run, &mut out, encoding_rs::GBK);
                    out.push('~');
                    i += 2;
                }
                Some(b'\n') => i += 2,
                _ => {
                    // A tilde that begins nothing. In ASCII it is a tilde; in a
                    // GB stretch it cannot be, so it is dropped rather than
                    // shifting the pairs that follow it by one byte.
                    if !gb {
                        out.push('~');
                    }
                    i += 1;
                }
            }
            continue;
        }
        if !gb {
            push_unshifted(&mut out, b);
            i += 1;
            continue;
        }
        if b == b'\r' || b == b'\n' {
            flush(&mut run, &mut out, encoding_rs::GBK);
            gb = false;
            out.push(b as char);
            i += 1;
            continue;
        }
        match (is_septet(b), bytes.get(i + 1).copied()) {
            (true, Some(next)) if is_septet(next) => {
                run.push(b | 0x80);
                run.push(next | 0x80);
                i += 2;
            }
            _ => {
                flush(&mut run, &mut out, encoding_rs::GBK);
                push_unshifted(&mut out, b);
                i += 1;
            }
        }
    }
    flush(&mut run, &mut out, encoding_rs::GBK);
    out
}

/// Which set a shifted stretch is carrying.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    /// GB2312, which is GBK with the high bits off.
    Gb2312,
    /// CNS 11643 or ISO-IR-165: announced, understood, and not carried here.
    Untabled,
}

/// ISO-2022-CN and ISO-2022-CN-EXT, RFC 1922.
///
/// `ESC $ ) x` designates a set into G1, which SO and SI shift in and out of;
/// `ESC $ * H` and `ESC $ + x` designate G2 and G3, reached one character at a
/// time by SS2 (`ESC N`) and SS3 (`ESC O`). Only the GB2312 designation has a
/// table here; the rest keep their structure so the message's ASCII survives.
fn decode_iso2022_cn(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut run: Vec<u8> = Vec::new();
    let mut g1: Option<Set> = None;
    let mut shifted = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1B {
            let rest = &bytes[i..];
            if rest.starts_with(b"\x1b$)A") {
                flush(&mut run, &mut out, encoding_rs::GBK);
                g1 = Some(Set::Gb2312);
                i += 4;
                continue;
            }
            if rest.starts_with(b"\x1b$)G") || rest.starts_with(b"\x1b$)E") {
                flush(&mut run, &mut out, encoding_rs::GBK);
                g1 = Some(Set::Untabled);
                i += 4;
                continue;
            }
            // G2 and G3 designations: noted and skipped, since what they name
            // has no table here either way.
            if rest.starts_with(b"\x1b$*") || rest.starts_with(b"\x1b$+") {
                i += 4.min(rest.len());
                continue;
            }
            // SS2 and SS3: one character from a set we cannot draw.
            if rest.starts_with(b"\x1bN") || rest.starts_with(b"\x1bO") {
                flush(&mut run, &mut out, encoding_rs::GBK);
                out.push('\u{FFFD}');
                i += 4.min(rest.len());
                continue;
            }
            i += 1;
            continue;
        }
        match b {
            0x0E => {
                shifted = true;
                i += 1;
            }
            0x0F => {
                flush(&mut run, &mut out, encoding_rs::GBK);
                shifted = false;
                i += 1;
            }
            b'\r' | b'\n' => {
                flush(&mut run, &mut out, encoding_rs::GBK);
                // A line ends in ASCII, and the designation does not survive
                // it either — RFC 1922 requires it announced again.
                shifted = false;
                g1 = None;
                out.push(b as char);
                i += 1;
            }
            _ if shifted => match (is_septet(b), bytes.get(i + 1).copied()) {
                (true, Some(next)) if is_septet(next) => {
                    match g1 {
                        Some(Set::Gb2312) => {
                            run.push(b | 0x80);
                            run.push(next | 0x80);
                        }
                        _ => {
                            flush(&mut run, &mut out, encoding_rs::GBK);
                            out.push('\u{FFFD}');
                        }
                    }
                    i += 2;
                }
                _ => {
                    flush(&mut run, &mut out, encoding_rs::GBK);
                    push_unshifted(&mut out, b);
                    i += 1;
                }
            },
            _ => {
                push_unshifted(&mut out, b);
                i += 1;
            }
        }
    }
    flush(&mut run, &mut out, encoding_rs::GBK);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_are_recognised_and_nothing_else_is() {
        for yes in [
            "ISO-2022-KR",
            "iso-2022-kr",
            "csISO2022KR",
            "ISO-2022-CN",
            "ISO-2022-CN-EXT",
            "HZ-GB-2312",
            "hz-gb2312",
            "\"ISO-2022-KR\"",
            "ISO-2022-KR*ko",
        ] {
            assert!(is_replacement_charset(yes), "{yes} should be ours");
        }
        for no in [
            "ISO-2022-JP",
            "EUC-KR",
            "Big5",
            "GBK",
            "GB18030",
            "UTF-8",
            "Shift_JIS",
            "windows-1252",
            "",
        ] {
            assert!(!is_replacement_charset(no), "{no} is not ours to decode");
        }
    }

    #[test]
    fn iso_2022_kr_reads() {
        // ESC $ ) C announces KS X 1001; SO shifts in, SI shifts back.
        let raw = b"\x1b$)CHello \x0eGQ1[\x0f!";
        assert_eq!(decode("ISO-2022-KR", raw).unwrap(), "Hello 한글!");
    }

    /// The announcement belongs to the message, not to every field, so a
    /// header carrying only the shifted stretch still has to read.
    #[test]
    fn iso_2022_kr_reads_without_the_announcement() {
        assert_eq!(decode("ISO-2022-KR", b"\x0eGQ1[\x0f").unwrap(), "한글");
    }

    #[test]
    fn hz_reads_including_its_escapes() {
        assert_eq!(decode("HZ-GB-2312", b"Hi ~{VPND~}!").unwrap(), "Hi 中文!");
        // A doubled tilde is one tilde; a tilde before a newline is a fold.
        assert_eq!(decode("HZ-GB-2312", b"a~~b").unwrap(), "a~b");
        assert_eq!(decode("HZ-GB-2312", b"a~\nb").unwrap(), "ab");
    }

    #[test]
    fn iso_2022_cn_reads_its_gb_half() {
        let raw = b"\x1b$)A\x0eVPND\x0f ok";
        assert_eq!(decode("ISO-2022-CN", raw).unwrap(), "中文 ok");
    }

    /// The Traditional set has no table here. What it costs is those
    /// characters, not the message around them.
    #[test]
    fn iso_2022_cn_keeps_what_it_can_of_the_untabled_half() {
        let raw = b"\x1b$)G\x0eVPND\x0f plain text";
        let out = decode("ISO-2022-CN", raw).unwrap();
        assert!(
            out.ends_with(" plain text"),
            "the ASCII was lost too: {out:?}"
        );
        assert!(
            out.contains('\u{FFFD}'),
            "the untabled run should be marked"
        );
    }

    #[test]
    fn a_line_ends_in_ascii() {
        // Shift state does not cross a newline, which is what folded headers
        // and quoted-printable soft breaks depend on.
        let out = decode("ISO-2022-KR", b"\x0eGQ\nplain").unwrap();
        assert!(
            out.ends_with("\nplain"),
            "state leaked across the line: {out:?}"
        );
    }

    #[test]
    fn something_not_ours_is_left_alone() {
        assert!(decode("UTF-8", b"hello").is_none());
        assert!(decode("ISO-2022-JP", b"hello").is_none());
    }

    /// Every byte here is a stranger's. The machines must not panic, hang, or
    /// run away with memory on anything at all.
    #[test]
    fn no_input_can_break_it() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let interesting: &[u8] =
            b"\x1b$)C\x1b$)A\x1b$)G\x1b$*H\x1b$+I\x1bN\x1bO\x0e\x0f~{~}~~\r\n!~";
        for case in 0..4000 {
            let len = (next() % 64) as usize;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                let r = next();
                bytes.push(if r % 3 == 0 {
                    interesting[(r >> 8) as usize % interesting.len()]
                } else {
                    (r >> 16) as u8
                });
            }
            for label in ["ISO-2022-KR", "HZ-GB-2312", "ISO-2022-CN"] {
                let out = decode(label, &bytes).expect("ours to decode");
                assert!(
                    out.len() <= bytes.len() * 4 + 8,
                    "case {case}: {label} grew {} bytes into {}",
                    bytes.len(),
                    out.len()
                );
            }
        }
    }

    /// Truncation is the ordinary shape of a damaged message: every prefix of
    /// something valid has to come back as something.
    #[test]
    fn every_prefix_of_a_good_message_decodes() {
        let full = b"\x1b$)CHello \x0eGQ1[\x0f world";
        for n in 0..=full.len() {
            let _ = decode("ISO-2022-KR", &full[..n]).expect("ours");
        }
        let hz = b"Hi ~{VPND~} there";
        for n in 0..=hz.len() {
            let _ = decode("HZ-GB-2312", &hz[..n]).expect("ours");
        }
    }
}
