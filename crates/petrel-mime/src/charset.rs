//! Which charsets we decode ourselves, and what to do with a name nobody
//! recognises.
//!
//! `encoding_rs` implements the WHATWG Encoding Standard, which is a browser
//! specification. A mail client inherits two problems from that.
//!
//! The first is refusal. ISO-2022-KR, ISO-2022-CN and HZ-GB-2312 are mapped to
//! an encoding named "replacement" that answers any input with a single
//! U+FFFD, because in a browser they can smuggle markup past a filter. UTF-7
//! was dropped from the standard for the same reason. Mail already archived in
//! them still has to be readable, so those are decoded here, in
//! [`crate::legacy_cjk`] and [`crate::utf7`].
//!
//! The second is names. The Standard resolves labels from a fixed list, and the
//! vendor spellings Windows mailers actually send are not on it: CP932 for
//! Japanese, CP949 and UHC for Korean, CP936 and CP950 for Chinese. These need
//! no decoder, only the right name, because the Standard's indexes *are* the
//! Microsoft ones. [`alias`] maps them, and refuses to answer for any label
//! that already resolves, so it can only fill a gap and never override a
//! decision made correctly elsewhere.
//!
//! What is left is a label nobody can place at all: `unknown-8bit`, which is a
//! sender admitting it does not know, and the long tail of encodings with no
//! table here. An unresolvable label is not an error further down — the bytes
//! are read as UTF-8 and lossily, which is the worst of the options, because it
//! replaces what it cannot read and the original is then gone from the index
//! and the snippet alike. [`fallback`] makes that decision better.

use crate::legacy_cjk;
use crate::utf7;

/// The charset name itself, without the quoting a header may put round it or
/// the language tag RFC 2231 allows after a `*`. Neither is part of the name.
pub(crate) fn name(label: &str) -> &str {
    let label = label.trim().trim_matches('"');
    label.split('*').next().unwrap_or(label).trim()
}

/// Whether the Encoding Standard can resolve this label at all.
///
/// A label it cannot resolve is read as UTF-8 by everything downstream,
/// whatever the bytes actually are, so this is the line between mail that
/// decodes by itself and mail that needs us.
pub fn resolvable(label: &str) -> bool {
    encoding_rs::Encoding::for_label(name(label).as_bytes()).is_some()
}

/// The encoding a label means when the Standard knows it under another name,
/// or `None` when there is nothing to fix.
///
/// The guard on the first line is the important part: a label `encoding_rs`
/// can resolve is left entirely alone, so no message that decodes correctly
/// today can change behaviour because of a mapping added here.
///
/// Every mapping is exact rather than approximate, because the Encoding
/// Standard adopted the Microsoft indexes wholesale. The cases that would
/// expose a difference decode clean: CP932's NEC vendor rows through
/// `SHIFT_JIS`, and UHC's extended lead bytes through `EUC_KR`.
///
/// The ISO-2022-JP extensions are the one approximation. Each adds character
/// sets to the base one — JIS X 0212, GB2312 and KSC5601 for `-2`, JIS X 0213
/// for `-3` and `-2004` — and the tables for those extra designations are not
/// here. Their common case is ASCII and JIS X 0208, which is exactly
/// ISO-2022-JP, so this reads the parts it knows and marks the rest.
pub fn alias(label: &str) -> Option<&'static encoding_rs::Encoding> {
    let label = name(label);
    if encoding_rs::Encoding::for_label(label.as_bytes()).is_some() {
        return None;
    }
    let lower = label.to_ascii_lowercase();
    Some(match lower.as_str() {
        "cp932" | "x-ms-cp932" => encoding_rs::SHIFT_JIS,
        "cp949" | "uhc" | "x-uhc" | "x-windows-949" | "x-euc-kr" | "ks_c_5601" => {
            encoding_rs::EUC_KR
        }
        "cp936" | "windows-936" | "ms936" | "ms_936" => encoding_rs::GBK,
        "cp950" | "windows-950" | "ms950" | "x-big5" => encoding_rs::BIG5,
        "iso-2022-jp-1" | "iso-2022-jp-2" | "iso-2022-jp-3" | "iso-2022-jp-2004"
        | "iso-2022-jp-ms" => encoding_rs::ISO_2022_JP,
        _ => return None,
    })
}

/// Whether this charset has to be decoded here rather than left to the parser.
///
/// Either because the Standard refuses it, or because it cannot place the name
/// and would read the bytes as UTF-8 regardless of what they are.
pub fn decodes_here(label: &str) -> bool {
    legacy_cjk::is_replacement_charset(label) || !resolvable(label)
}

/// Decodes a charset the parser would get wrong, or `None` when it would not.
pub fn decode(label: &str, bytes: &[u8]) -> Option<String> {
    let cleaned = name(label);
    // A vendor spelling of a table we already ship. A lookup, nothing more.
    if let Some(encoding) = alias(label) {
        return Some(encoding.decode(bytes).0.into_owned());
    }
    // One of the ones the Standard answers with a single U+FFFD.
    if let Some(text) = legacy_cjk::decode(label, bytes) {
        return Some(text);
    }
    if utf7::is_utf7(cleaned) {
        return Some(utf7::decode(bytes));
    }
    // Anything the Standard can resolve is its business, not ours.
    if resolvable(label) {
        return None;
    }
    Some(fallback(bytes))
}

/// What to read bytes as when the label is no help at all.
///
/// The rule downstream is to read them as UTF-8 and replace whatever will not
/// decode. That is right for the commonest case by far, which is a sender who
/// emitted UTF-8 and labelled it something strange, and wrong for every other
/// case, because U+FFFD is not recoverable: a Latin-1 message arrives as
/// `caf? na?ve` and neither the reader nor the search index ever sees the
/// original byte again.
///
/// So: valid UTF-8 is UTF-8. Otherwise the question is whether this is UTF-8
/// with damage or text that was never UTF-8, and those two look quite
/// different. Genuine UTF-8 that lost a byte still decodes into real
/// multi-byte characters everywhere else, with one bad spot. Text in a
/// single-byte encoding has a high byte go wrong nearly every time one
/// appears, and yields no multi-byte characters at all, because Latin-1
/// accents do not happen to form valid UTF-8 sequences.
///
/// Where it is not UTF-8, windows-1252 is the answer: it is what mail clients
/// have always fallen back to, it decodes every possible byte so nothing is
/// replaced, and it is reversible, so a reader who sees mojibake can still get
/// at the original through View Source and the bytes stay in the index.
pub fn fallback(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let lossy = String::from_utf8_lossy(bytes);
    if damaged_utf8(&lossy) {
        return lossy.into_owned();
    }
    encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned()
}

/// Whether this looks like UTF-8 that lost a byte rather than text that was
/// never UTF-8.
///
/// Two conditions, and the first does most of the work. Real multi-byte
/// characters surviving the decode means the bytes were UTF-8, since a
/// single-byte encoding produces none: its high bytes are invalid sequences
/// and come back replaced. The proportion is the backstop, for the rare text
/// whose high bytes happen to pair into something valid.
fn damaged_utf8(lossy: &str) -> bool {
    let mut total = 0usize;
    let mut bad = 0usize;
    let mut multibyte = false;
    for c in lossy.chars() {
        total += 1;
        if c == '\u{FFFD}' {
            bad += 1;
        } else if !c.is_ascii() {
            multibyte = true;
        }
    }
    // Two percent, which separated a clipped body from Latin-1 text by a wide
    // margin when measured on both.
    multibyte && total > 0 && bad * 50 <= total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vendor_spellings_map_to_the_tables_they_mean() {
        for (label, want) in [
            ("CP932", encoding_rs::SHIFT_JIS),
            ("cp932", encoding_rs::SHIFT_JIS),
            ("x-ms-cp932", encoding_rs::SHIFT_JIS),
            ("CP949", encoding_rs::EUC_KR),
            ("UHC", encoding_rs::EUC_KR),
            ("x-windows-949", encoding_rs::EUC_KR),
            ("ks_c_5601", encoding_rs::EUC_KR),
            ("CP936", encoding_rs::GBK),
            ("windows-936", encoding_rs::GBK),
            ("CP950", encoding_rs::BIG5),
            ("windows-950", encoding_rs::BIG5),
            ("x-big5", encoding_rs::BIG5),
            ("ISO-2022-JP-2", encoding_rs::ISO_2022_JP),
            ("iso-2022-jp-2004", encoding_rs::ISO_2022_JP),
        ] {
            assert_eq!(alias(label), Some(want), "{label} maps wrong");
            assert!(decodes_here(label), "{label} should be decoded here");
        }
    }

    /// The guard that makes this safe. Anything the standard decoder can
    /// resolve is left to it, so no message that reads correctly today can
    /// change because of a mapping added here.
    #[test]
    fn a_name_the_standard_decoder_knows_is_left_alone() {
        for already_fine in [
            "UTF-8",
            "Shift_JIS",
            "windows-31j",
            "EUC-KR",
            "ks_c_5601-1987",
            "GBK",
            "GB18030",
            "Big5",
            "big5-hkscs",
            "ISO-2022-JP",
            "windows-1252",
            "windows-1258",
            "iso-8859-1",
        ] {
            assert_eq!(alias(already_fine), None, "{already_fine} was overridden");
            assert!(resolvable(already_fine), "{already_fine} resolves");
            assert!(
                !decodes_here(already_fine),
                "{already_fine} is not ours to decode"
            );
            assert_eq!(
                decode(already_fine, b"hello"),
                None,
                "{already_fine} must be left to the parser"
            );
        }
    }

    #[test]
    fn quoting_and_a_language_tag_are_not_part_of_the_name() {
        assert_eq!(alias("\"CP932\""), Some(encoding_rs::SHIFT_JIS));
        assert_eq!(alias("CP949*ko"), Some(encoding_rs::EUC_KR));
        assert_eq!(alias("  cp950  "), Some(encoding_rs::BIG5));
        assert!(resolvable("\"UTF-8\""));
        assert!(resolvable("UTF-8*en"));
    }

    /// Exact, not approximate. These are the ranges that only exist in the
    /// vendor encoding, so if the Encoding Standard had adopted anything other
    /// than the Microsoft index they would be where it showed.
    #[test]
    fn the_vendor_only_ranges_decode_rather_than_approximate() {
        // CP932's NEC special row, which plain JIS X 0208 does not have.
        assert_eq!(
            decode("CP932", b"\x87\x40\x87\x54").unwrap(),
            "\u{2460}\u{2160}"
        );
        // A lead byte in UHC's extension, outside the EUC-KR ones.
        let korean = decode("UHC", b"\x81\x41\x81\x61").unwrap();
        assert!(
            !korean.contains('\u{FFFD}'),
            "UHC extension lost: {korean:?}"
        );
    }

    #[test]
    fn the_ordinary_ranges_read_too() {
        assert_eq!(decode("CP932", b"\x82\xb1\x82\xea").unwrap(), "これ");
        assert_eq!(decode("CP949", b"\xc7\xd1\xb1\xdb").unwrap(), "한글");
        assert_eq!(decode("CP936", b"\xd6\xd0\xce\xc4").unwrap(), "中文");
        assert_eq!(decode("CP950", b"\xa4\xa4\xa4\xe5").unwrap(), "中文");
    }

    /// The extensions carry sets we have no table for, so the bargain is the
    /// same one ISO-2022-CN makes: read what is known, mark the rest.
    #[test]
    fn an_iso_2022_jp_extension_reads_its_base_repertoire() {
        assert_eq!(
            decode("ISO-2022-JP-2", b"\x1b$B$3$l\x1b(B ok").unwrap(),
            "これ ok"
        );
    }

    #[test]
    fn utf7_is_decoded_here_since_the_standard_dropped_it() {
        assert!(decodes_here("UTF-7"));
        assert_eq!(decode("UTF-7", b"Hi Mom -+Jjo--!").unwrap(), "Hi Mom -☺-!");
        assert_eq!(decode("utf-7", b"+ZeVnLIqe-").unwrap(), "日本語");
    }

    #[test]
    fn the_four_refused_charsets_still_go_to_their_own_decoders() {
        assert_eq!(decode("ISO-2022-KR", b"\x0eGQ1[\x0f").unwrap(), "한글");
        assert_eq!(decode("HZ-GB-2312", b"Hi ~{VPND~}!").unwrap(), "Hi 中文!");
    }
}

/// The name that is no name: what to do when nothing can place the label.
#[cfg(test)]
mod fallback_rule {
    use super::*;

    #[test]
    fn valid_utf8_is_left_exactly_as_it_is() {
        // Much the commonest case: a sender who emitted UTF-8 and labelled it
        // something nobody has heard of.
        assert_eq!(fallback("café ☕".as_bytes()), "café ☕");
        assert_eq!(fallback(b"plain ascii"), "plain ascii");
        assert_eq!(fallback(b""), "");
        assert_eq!(
            decode("unknown-8bit", "日本語".as_bytes()).unwrap(),
            "日本語"
        );
    }

    /// The row that matters. This is not mojibake, it is the right text, and
    /// Western mail under an odd label is far commoner than any CJK case.
    #[test]
    fn latin1_text_comes_back_correct_instead_of_replaced() {
        assert_eq!(fallback(b"caf\xe9 na\xefve"), "café naïve");
        assert_eq!(fallback(b"Gr\xfc\xdfe aus M\xfcnchen"), "Grüße aus München");
        assert_eq!(fallback(b"Hello \xa9 world"), "Hello © world");
    }

    /// Nothing is replaced, so nothing is lost: the reader can still get at
    /// the original through View Source, and the bytes stay in the index.
    #[test]
    fn what_cannot_be_read_is_at_least_not_destroyed() {
        let shift_jis = b"\x82\xb1\x82\xea";
        let out = fallback(shift_jis);
        assert!(!out.contains('\u{FFFD}'), "bytes were destroyed: {out:?}");
        let back = encoding_rs::WINDOWS_1252.encode(&out).0.into_owned();
        assert_eq!(back.as_slice(), shift_jis, "not reversible");
    }

    /// The one regression the rule has to avoid: a body that really is UTF-8
    /// and lost a byte must not be thrown wholesale at windows-1252.
    #[test]
    fn utf8_with_one_damaged_character_stays_utf8() {
        let mut bytes = "Here is the quarterly summary, written in full."
            .as_bytes()
            .to_vec();
        bytes.extend_from_slice(b"\xe2\x80"); // a clipped em dash
        bytes.extend_from_slice(" The rest of it reads perfectly well — ask Renée.".as_bytes());
        let out = fallback(&bytes);
        assert!(out.contains("quarterly summary"), "{out:?}");
        assert!(out.contains("Renée"), "the good text was mangled: {out:?}");
        assert!(out.contains('\u{FFFD}'), "the damage should still show");
        assert!(!out.contains("â€"), "fell through to windows-1252: {out:?}");
    }

    /// And the other side of that line: text with no valid multi-byte
    /// character anywhere was never UTF-8, however little of it is wrong.
    #[test]
    fn a_long_mostly_ascii_latin1_body_is_not_mistaken_for_damaged_utf8() {
        let mut bytes = "x".repeat(4000).into_bytes();
        bytes.extend_from_slice(b" caf\xe9");
        let out = fallback(&bytes);
        assert!(out.ends_with("café"), "{:?}", &out[out.len() - 20..]);
    }

    #[test]
    fn an_unplaceable_label_is_decoded_here_rather_than_left_to_be_replaced() {
        for label in ["unknown-8bit", "x-unknown", "euc-tw", "viscii", "tcvn-5712"] {
            assert!(decodes_here(label), "{label} should be ours");
            assert_eq!(
                decode(label, b"caf\xe9").as_deref(),
                Some("café"),
                "{label} lost the byte"
            );
        }
    }
}
