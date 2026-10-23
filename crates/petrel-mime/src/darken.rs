//! Light-only mail, recolored for a dark reading pane.
//!
//! Most HTML mail hard-codes a white world. A sender who declared dark
//! support keeps their own colors (`declares_dark`, elsewhere); everything
//! else rendered light-on-light as a white island in a dark app. This pass
//! rewrites the colors the sanitizer let through — style attributes and
//! `bgcolor` — flipping lightness and keeping hue: white grounds go dark,
//! near-black text goes light, a brand's teal stays that brand's teal.
//! Images are bytes, not declarations, and are never touched. It runs on
//! the sanitizer's own serialized output, whose attribute shapes are ours.

/// Rewrites every recolorable declaration for a dark ground.
///
/// One forward pass. Looking for each attribute name separately meant
/// searching the whole remaining document for `bgcolor="` on every element
/// that had none — 620 KB of styled mail took nine seconds and 1.5 MB took
/// fifty-five, on the thread drawing the window. Here the scan only ever moves
/// forward: find the next `="`, look at the few bytes in front of it to see
/// which attribute it belongs to, and carry on from the end of its value.
pub fn recolor_for_dark(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    loop {
        let Some(eq) = rest.find("=\"") else {
            out.push_str(rest);
            return out;
        };
        // Attribute starts are the serializer's own: a lowercase name, then
        // `="`. Nothing else it emits ends that way.
        let name = &rest[..eq];
        let attr = if name.ends_with("style") {
            Attr::Style
        } else if name.ends_with("bgcolor") {
            Attr::Bgcolor
        } else {
            Attr::Other
        };
        let start = eq + 2;
        if attr == Attr::Other {
            out.push_str(&rest[..start]);
            rest = &rest[start..];
            continue;
        }
        let Some(len) = rest[start..].find('"') else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        let value = &rest[start..start + len];
        match attr {
            Attr::Bgcolor => {
                out.push_str(&recolor_token(value).unwrap_or_else(|| value.to_string()))
            }
            _ => out.push_str(&recolor_style(value)),
        }
        rest = &rest[start + len..];
    }
}

/// Which of the two attributes carrying colors this one is, if either.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Attr {
    Style,
    Bgcolor,
    Other,
}

/// Properties whose values may carry colors, after the sanitizer's filter.
const COLOR_PROPS: &[&str] = &[
    "color",
    "background-color",
    // Admitted only with a picture in it, and a color can ride along with one:
    // left as written, that box stayed white in a dark pane.
    "background",
    "border",
    "border-top",
    "border-bottom",
    "border-left",
    "border-right",
    "border-color",
];

fn recolor_style(style: &str) -> String {
    declarations(style)
        .into_iter()
        .map(|decl| {
            let Some((prop, value)) = decl.split_once(':') else {
                return decl.to_string();
            };
            if !COLOR_PROPS.contains(&prop.trim().to_ascii_lowercase().as_str()) {
                return decl.to_string();
            }
            // Border shorthands mix widths and styles with a color; each
            // whitespace-separated token is tried on its own and non-colors
            // pass through unchanged.
            let rewritten = value
                .split_whitespace()
                .map(|tok| recolor_token(tok).unwrap_or_else(|| tok.to_string()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{prop}: {rewritten}", prop = prop.trim())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The declarations of a serialized style value, cut where CSS cuts them.
///
/// The value is the serializer's, so a `;` can also close a character
/// reference — `&quot;` around a font name, `&amp;` in an image address — or
/// sit inside a bracket, as in `url(…)`. Neither ends a declaration. Cut there,
/// the pieces were joined back with a space, and the font name or the address
/// came back with one inside it: `" Helvetica Neue"` names no font, and an
/// image's `&h=400` became `& h=400`.
fn declarations(style: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    // The markers are ASCII, so every index found is a character boundary.
    for (i, byte) in style.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b';' if depth == 0 && !ends_reference(&style[start..i]) => {
                out.push(&style[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&style[start..]);
    out
}

/// Whether the text ends inside a character reference — `&amp`, `&#39`,
/// `&#x27` — so the `;` after it closes the reference, not a declaration.
fn ends_reference(text: &str) -> bool {
    let Some(amp) = text.rfind('&') else {
        return false;
    };
    let body = &text[amp + 1..];
    let named = !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric());
    let numeric = body.strip_prefix('#').is_some_and(|n| {
        let (digits, hex) = match n.strip_prefix(['x', 'X']) {
            Some(h) => (h, true),
            None => (n, false),
        };
        !digits.is_empty()
            && digits.chars().all(|c| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            })
    });
    named || numeric
}

/// One CSS color token flipped, or None when it is not a color we read.
fn recolor_token(tok: &str) -> Option<String> {
    let t = tok.trim();
    if let Some(hex) = t.strip_prefix('#') {
        // Hex digits are ASCII, so anything else is not a colour — and the
        // byte slices below would land inside a multi-byte character and
        // panic, from inside the message handler, on `color: #€€`.
        if !hex.is_ascii() {
            return None;
        }
        let (r, g, b, a) = match hex.len() {
            3 => {
                let v: Vec<u8> = hex
                    .chars()
                    .map(|c| c.to_digit(16).map(|d| (d * 17) as u8))
                    .collect::<Option<_>>()?;
                (v[0], v[1], v[2], None)
            }
            6 | 8 => {
                let b = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
                    .collect::<Option<Vec<u8>>>()?;
                (b[0], b[1], b[2], b.get(3).copied())
            }
            _ => return None,
        };
        let (r, g, b) = flip(r, g, b);
        return Some(match a {
            Some(a) => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
            None => format!("#{r:02x}{g:02x}{b:02x}"),
        });
    }
    let lower = t.to_ascii_lowercase();
    if let Some(args) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if parts.len() < 3 {
            return None;
        }
        let ch = |s: &str| -> Option<u8> {
            if let Some(p) = s.strip_suffix('%') {
                Some(((p.trim().parse::<f32>().ok()? / 100.0) * 255.0).round() as u8)
            } else {
                s.parse::<f32>()
                    .ok()
                    .map(|v| v.round().clamp(0.0, 255.0) as u8)
            }
        };
        let (r, g, b) = flip(ch(parts[0])?, ch(parts[1])?, ch(parts[2])?);
        return Some(match parts.get(3) {
            Some(a) => format!("rgba({r}, {g}, {b}, {a})"),
            None => format!("rgb({r}, {g}, {b})"),
        });
    }
    // The named colors mail actually uses; anything unrecognized is left
    // alone rather than guessed at.
    let named: &[(&str, (u8, u8, u8))] = &[
        ("white", (255, 255, 255)),
        ("black", (0, 0, 0)),
        ("gray", (128, 128, 128)),
        ("grey", (128, 128, 128)),
        ("silver", (192, 192, 192)),
        ("whitesmoke", (245, 245, 245)),
        ("ghostwhite", (248, 248, 255)),
        ("lightgray", (211, 211, 211)),
        ("lightgrey", (211, 211, 211)),
        ("darkgray", (169, 169, 169)),
        ("darkgrey", (169, 169, 169)),
        ("dimgray", (105, 105, 105)),
        ("dimgrey", (105, 105, 105)),
        ("gainsboro", (220, 220, 220)),
        ("red", (255, 0, 0)),
        ("green", (0, 128, 0)),
        ("blue", (0, 0, 255)),
        ("navy", (0, 0, 128)),
        ("yellow", (255, 255, 0)),
        ("orange", (255, 165, 0)),
        ("purple", (128, 0, 128)),
        ("teal", (0, 128, 128)),
        ("maroon", (128, 0, 0)),
    ];
    let (r, g, b) = named.iter().find(|(n, _)| *n == lower)?.1;
    let (r, g, b) = flip(r, g, b);
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// Lightness flipped, hue and saturation held (RGB → HSL → RGB).
fn flip(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON);
        let h = if max == rf {
            ((gf - bf) / d).rem_euclid(6.0)
        } else if max == gf {
            (bf - rf) / d + 2.0
        } else {
            (rf - gf) / d + 4.0
        } * 60.0;
        (h, s.min(1.0))
    };
    // The flip itself, eased off pure extremes: absolute black grounds and
    // absolute white text read harsher than a dark app's own surfaces.
    let l = (1.0 - l).clamp(0.04, 0.96);
    // HSL → RGB.
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (rf, gf, bf) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((rf + m) * 255.0).round() as u8,
        ((gf + m) * 255.0).round() as u8,
        ((bf + m) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    /// `#€€` is six bytes and two characters. Slicing it by byte for the
    /// hex pairs panicked inside the message handler, so opening the message
    /// in dark mode took the app down. A colour we cannot read is left alone.
    #[test]
    fn a_non_ascii_colour_token_is_left_alone_rather_than_panicking() {
        let html = "<p style=\"color: #€€; background: #ffffff\">hi</p>";
        let out = super::recolor_for_dark(html);
        assert!(out.contains("#€€"), "{out}");
    }

    use super::*;

    #[test]
    fn a_background_keeps_its_picture_and_flips_its_color() {
        // The shorthand arrives only with a picture in it. Its color turns
        // with the rest of the page; its address comes out exactly as it went
        // in, `&amp;` and all — cut at that `;`, the query came back as
        // `& h=400` and asked for some other image.
        let html = r#"<td style="background: #ffffff url(&quot;https://cdn.example/bg.png?w=600&amp;h=400&quot;) top center / cover no-repeat; color: #111111">x</td>"#;
        let out = recolor_for_dark(html);
        assert!(
            out.contains(
                "url(&quot;https://cdn.example/bg.png?w=600&amp;h=400&quot;) top center / cover no-repeat"
            ),
            "{out}"
        );
        assert!(out.contains("background: #0a0a0a"), "{out}");
        assert!(out.contains("color: #eeeeee"), "{out}");
    }

    #[test]
    fn a_quoted_font_name_comes_through_whole() {
        // `&quot;` ends in a `;` too. Cut there, the name came back as
        // `" Helvetica Neue"`, which names no font, and the text fell back.
        let html =
            r#"<p style="font-family: &quot;Helvetica Neue&quot;, Arial; color: #111111">x</p>"#;
        let out = recolor_for_dark(html);
        assert!(
            out.contains("font-family: &quot;Helvetica Neue&quot;, Arial"),
            "{out}"
        );
        assert!(out.contains("color: #eeeeee"), "{out}");
    }

    #[test]
    fn declarations_part_where_css_parts_them() {
        assert_eq!(
            declarations("color: red; margin: 0"),
            ["color: red", " margin: 0"]
        );
        assert_eq!(
            declarations("background: url(&quot;a;b&quot;); color: red"),
            ["background: url(&quot;a;b&quot;)", " color: red"]
        );
        assert_eq!(
            declarations("content: &#39;x&#x27;; a: b"),
            ["content: &#39;x&#x27;", " a: b"]
        );
        // Past the end of a reference, a `;` divides again: the serializer
        // writes a text `&` as `&amp;`, and what follows it is ordinary text.
        assert_eq!(declarations("a: R&amp;D; b: c"), ["a: R&amp;D", " b: c"]);
    }

    #[test]
    fn light_flips_dark_and_hue_survives() {
        // White ground goes near-black; black text goes near-white.
        let html = r#"<td style="background-color: #ffffff; color: #111111">x</td>"#;
        let out = recolor_for_dark(html);
        assert!(out.contains("background-color: #0a0a0a"), "{out}");
        assert!(out.contains("color: #eeeeee"), "{out}");
        // A saturated brand blue keeps its hue: red stays low, blue high.
        let out = recolor_for_dark(r#"<a style="color: #1155cc">y</a>"#);
        let hex = out.split("color: #").nth(1).unwrap()[..6].to_string();
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
        assert!(b > r, "hue held: {out}");
    }

    #[test]
    fn bgcolor_named_and_rgba_forms() {
        let out =
            recolor_for_dark(r#"<table bgcolor="white"><tr style="color: rgba(20, 20, 20, 0.9)">"#);
        assert!(out.contains(r##"bgcolor="#0a0a0a""##), "{out}");
        assert!(out.contains("rgba(23") || out.contains("rgba(2"), "{out}");
        assert!(out.contains(", 0.9)"), "alpha kept: {out}");
    }

    /// The scan has to stay linear. Searching the rest of the document for
    /// each attribute name separately made a styled 620 KB message take nine
    /// seconds in dark mode, and 1.5 MB take fifty-five — with the reading
    /// pane empty for all of it.
    #[test]
    fn a_large_styled_message_is_recolored_promptly() {
        // 20,000 styled elements, and one bgcolor right at the end: the shape
        // that made the old scan quadratic, because every element without one
        // searched the whole remaining document for it.
        let html = format!(
            "{}<table bgcolor=\"white\"><tr><td>x</td></tr></table>",
            "<p style=\"color: #333333\">x</p>".repeat(20_000)
        );
        let started = std::time::Instant::now();
        let out = recolor_for_dark(&html);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(
            out.matches("color: #cccccc").count(),
            20_000,
            "all recolored"
        );
        assert!(out.contains(r##"bgcolor="#0a0a0a""##), "including the last");
    }

    #[test]
    fn non_colors_pass_untouched() {
        let html = r#"<img src="cid:x"><td style="width: 600px; border: 1px solid #dddddd">z</td>"#;
        let out = recolor_for_dark(html);
        assert!(out.contains(r#"src="cid:x""#));
        assert!(out.contains("width: 600px"));
        assert!(out.contains("1px solid #"), "{out}");
        assert!(!out.contains("#dddddd"), "border color flipped: {out}");
        // Unknown named colors and keywords stay.
        let out = recolor_for_dark(r#"<td style="background-color: transparent">x</td>"#);
        assert!(out.contains("transparent"));
    }
}
