//! UTF-7, the way mail spells it (RFC 2152).
//!
//! A way of writing Unicode in seven bits, from before 8-bit transport could be
//! relied on. Outlook Express and Exchange sent it for years, so it is what a
//! long archive holds, and `encoding_rs` does not implement it: the Encoding
//! Standard dropped UTF-7 deliberately, because in a browser it lets an
//! attacker write `+ADw-script+AD4-` and have it become a tag after the filter
//! has looked. Nothing here renders markup, so that reasoning does not reach a
//! parser whose output is text; refusing it only loses the message. The
//! sanitizer still sees everything this produces.
//!
//! Text runs as ASCII until a `+`, which opens a run of modified base64
//! standing for UTF-16BE. The run closes at a `-`, which is absorbed, or at any
//! character that cannot be base64, which is not. `+-` is how a literal plus
//! sign is written.
//!
//! IMAP mailbox names use a near neighbour of this, RFC 3501's modified UTF-7,
//! and the provider crate decodes those. They are deliberately not shared code:
//! that one shifts on `&`, spells base64's `/` as `,`, and must close every run
//! explicitly. Folding two encodings that differ in their delimiters into one
//! function would serve neither.
//!
//! Hostile input is the norm here. Every loop advances on every branch and
//! allocates in proportion to its input, so there is no path that can panic or
//! fail to terminate.

/// Whether this label names UTF-7.
pub fn is_utf7(label: &str) -> bool {
    [
        "utf-7",
        "utf7",
        "unicode-1-1-utf-7",
        "csunicode11utf7",
        "x-unicode-2-0-utf-7",
    ]
    .iter()
    .any(|known| label.eq_ignore_ascii_case(known))
}

fn is_b64(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/'
}

fn b64_val(b: u8) -> Option<u32> {
    Some(match b {
        b'A'..=b'Z' => u32::from(b - b'A'),
        b'a'..=b'z' => u32::from(b - b'a') + 26,
        b'0'..=b'9' => u32::from(b - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

/// One `+…` run: modified base64 of UTF-16BE code units.
fn push_shifted(run: &[u8], out: &mut String) {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut units: Vec<u16> = Vec::with_capacity(run.len() * 3 / 8 + 1);
    for &c in run {
        let Some(v) = b64_val(c) else { continue };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 16 {
            bits -= 16;
            units.push((acc >> bits) as u16);
            acc &= (1u32 << bits) - 1;
        }
    }
    // Whatever is left is padding to the next octet and carries no character,
    // whether or not the sender zeroed it as the RFC asks.
    for r in char::decode_utf16(units) {
        // An unpaired surrogate is a real possibility from a broken encoder,
        // and is the one thing UTF-16 cannot hand back as a character.
        out.push(r.unwrap_or('\u{FFFD}'));
    }
}

/// Decodes UTF-7. Never fails: malformed input degrades to the text around it.
pub fn decode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'+' {
            // UTF-7 is seven-bit by construction, so a high byte is not
            // something this encoding can be carrying. Saying so beats
            // inventing a Latin-1 character for it.
            out.push(if b.is_ascii() { b as char } else { '\u{FFFD}' });
            i += 1;
            continue;
        }
        i += 1;
        if bytes.get(i) == Some(&b'-') {
            out.push('+');
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_b64(bytes[i]) {
            i += 1;
        }
        if i == start {
            // A plus sign that opens nothing. Well-formed UTF-7 would have
            // written `+-`, but keeping it reads better than dropping it and
            // cannot run away: the loop has already moved past the plus.
            out.push('+');
            continue;
        }
        push_shifted(&bytes[start..i], &mut out);
        // A dash closes the run and belongs to it. Anything else closes the
        // run and belongs to the text, so it is left for the next turn.
        if bytes.get(i) == Some(&b'-') {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_labels_are_recognised_and_nothing_else_is() {
        for yes in ["UTF-7", "utf-7", "UNICODE-1-1-UTF-7", "csUnicode11UTF7"] {
            assert!(is_utf7(yes), "{yes} is UTF-7");
        }
        for no in ["UTF-8", "UTF-16", "utf-70", "", "us-ascii"] {
            assert!(!is_utf7(no), "{no} is not UTF-7");
        }
    }

    #[test]
    fn ascii_passes_through_untouched() {
        assert_eq!(decode(b"Hello, world!"), "Hello, world!");
    }

    #[test]
    fn a_shift_run_reads() {
        // RFC 2152's own example: "Hi Mom -<WHITE SMILING FACE>-!"
        assert_eq!(decode(b"Hi Mom -+Jjo--!"), "Hi Mom -\u{263A}-!");
        // Japanese: "日本語"
        assert_eq!(decode(b"+ZeVnLIqe-"), "日本語");
    }

    #[test]
    fn a_literal_plus_is_written_as_plus_dash() {
        assert_eq!(decode(b"1 +- 1 = 2"), "1 + 1 = 2");
        assert_eq!(decode(b"+-"), "+");
    }

    /// The dash is part of the run; anything else that ends a run is part of
    /// the text and must survive.
    #[test]
    fn a_run_ended_by_something_other_than_a_dash_keeps_that_character() {
        assert_eq!(decode(b"+Jjo!"), "\u{263A}!");
        assert_eq!(decode(b"+Jjo and more"), "\u{263A} and more");
    }

    #[test]
    fn a_surrogate_pair_becomes_one_character() {
        // U+1F600, which needs two UTF-16 units.
        assert_eq!(decode(b"+2D3eAA-"), "\u{1F600}");
    }

    #[test]
    fn malformed_input_degrades_and_terminates() {
        // A plus at the very end, a plus opening nothing, an unpaired
        // surrogate, and a high byte that cannot be in seven-bit text.
        assert_eq!(decode(b"a+"), "a+");
        assert_eq!(decode(b"a+ b"), "a+ b");
        assert!(decode(b"+2D0-").contains('\u{FFFD}'));
        assert!(decode(b"a\xffb").contains('\u{FFFD}'));
        // Odd numbers of base64 characters leave padding bits over.
        let _ = decode(b"+A-");
        let _ = decode(b"+AAAAAAAAAAAAAAAAAAAA");
    }

    #[test]
    fn runs_can_follow_one_another() {
        assert_eq!(decode(b"+ZeU-+Zyw-"), "日本");
    }
}
