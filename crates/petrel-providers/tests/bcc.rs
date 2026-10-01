//! Blind copies: on the envelope, in the sender's own copy, and nowhere else.
//!
//! A blind copy is a promise about who can see whom. The people in To and Cc
//! must never learn that anyone was blind-copied, and the person sending must
//! still be able to see whom they copied, in Sent and in Drafts. So one
//! message is rendered twice over: the wire copy names nobody in Bcc, and the
//! sender's copy is the same bytes with a Bcc header added.

use petrel_providers::smtp::Outgoing;

fn message() -> Outgoing {
    Outgoing {
        from_addr: "sam@example.com".into(),
        from_name: "Sam Ortiz".into(),
        to: vec!["Dana Wu <dana@example.com>".into()],
        cc: vec!["alex@example.com".into()],
        bcc: vec![
            "Priya Nair <priya@example.net>".into(),
            "board@example.org".into(),
        ],
        subject: "Quarterly numbers".into(),
        body_text: "The numbers are in.".into(),
        body_html: Some("<p>The numbers are in.</p>".into()),
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    }
}

/// The header block of a rendered message: everything before the first
/// blank line.
fn headers(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    text.split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .to_string()
}

fn header_names(raw: &[u8]) -> Vec<String> {
    headers(raw)
        .lines()
        .filter(|l| !l.starts_with([' ', '\t']))
        .filter_map(|l| {
            l.split_once(':')
                .map(|(n, _)| n.trim().to_ascii_lowercase())
        })
        .collect()
}

#[test]
fn the_wire_copy_names_no_blind_copy() {
    let (_, raw) = message().render("example.com");
    assert!(
        !header_names(&raw).iter().any(|n| n == "bcc"),
        "a Bcc header would tell everyone who was blind-copied: {}",
        headers(&raw)
    );
    let text = String::from_utf8_lossy(&raw);
    for hidden in ["priya@example.net", "board@example.org", "Priya Nair"] {
        assert!(!text.contains(hidden), "{hidden} reached the wire copy");
    }
    // The people who are meant to be seen still are.
    let parsed = petrel_mime::parse_message(&raw).unwrap();
    assert_eq!(
        parsed.to,
        vec![(Some("Dana Wu".to_string()), "dana@example.com".to_string())]
    );
    assert_eq!(parsed.cc, vec![(None, "alex@example.com".to_string())]);
    assert!(parsed.bcc.is_empty());
}

#[test]
fn the_senders_copy_names_every_blind_copy() {
    let m = message();
    let (id, wire) = m.render("example.com");
    let kept = m.sender_copy(&wire);
    let parsed = petrel_mime::parse_message(&kept).unwrap();
    assert_eq!(
        parsed.bcc,
        vec![
            (
                Some("Priya Nair".to_string()),
                "priya@example.net".to_string()
            ),
            (None, "board@example.org".to_string()),
        ]
    );
    // The same message otherwise: one Message-ID, one Date, one body.
    assert_eq!(parsed.message_id.as_deref(), Some(id.as_str()));
    let wire_text = String::from_utf8_lossy(&wire).into_owned();
    let kept_text = String::from_utf8_lossy(&kept).into_owned();
    let body = |t: &str| {
        t.split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default()
    };
    assert_eq!(
        body(&kept_text),
        body(&wire_text),
        "only the headers differ"
    );
    let wire_headers: Vec<&str> = wire_text
        .split("\r\n\r\n")
        .next()
        .unwrap()
        .lines()
        .collect();
    for line in wire_headers {
        assert!(kept_text.contains(line), "the sender's copy kept {line:?}");
    }
}

#[test]
fn with_no_blind_copy_the_senders_copy_is_the_wire_copy() {
    let mut m = message();
    m.bcc.clear();
    let (_, wire) = m.render("example.com");
    assert_eq!(m.sender_copy(&wire), wire);
}

#[test]
fn the_envelope_carries_every_blind_copy() {
    assert_eq!(
        message().recipients(),
        vec![
            "dana@example.com",
            "alex@example.com",
            "priya@example.net",
            "board@example.org"
        ]
    );
}

/// A half-typed blind copy refuses the send, as a half-typed To does. Left out
/// at the wire, the person would believe someone was copied who never was.
#[test]
fn a_blind_copy_that_is_not_an_address_is_named() {
    let mut m = message();
    m.bcc.push("not an address".into());
    assert_eq!(m.unsendable(), vec!["not an address".to_string()]);
}

/// Blind copies only, as when writing to a list of parents. RFC 5322 lets the
/// To field go unwritten, but a message with none at all is a spam signal, so
/// it is the empty group every client understands, as Thunderbird writes it.
#[test]
fn a_message_with_only_blind_copies_is_to_undisclosed_recipients() {
    let mut m = message();
    m.to.clear();
    m.cc.clear();
    let (_, wire) = m.render("example.com");
    let head = headers(&wire);
    assert!(
        head.lines().any(|l| l == "To: undisclosed-recipients: ;"),
        "{head}"
    );
    assert!(!header_names(&wire).iter().any(|n| n == "bcc"), "{head}");
    let parsed = petrel_mime::parse_message(&wire).unwrap();
    assert!(
        parsed.to.is_empty(),
        "nobody is named in To: {:?}",
        parsed.to
    );
    assert_eq!(
        m.recipients(),
        vec!["priya@example.net", "board@example.org"]
    );
}

/// A Bcc entry is a header value in the sender's copy, so it gets the same
/// scrubbing as To: one that would start a second line never becomes one.
#[test]
fn a_blind_copy_cannot_write_a_header_of_its_own() {
    let mut m = message();
    m.bcc = vec![
        "eve@example.com\r\nX-Injected: yes".into(),
        "ok@example.org".into(),
    ];
    let (_, wire) = m.render("example.com");
    let kept = m.sender_copy(&wire);
    let text = String::from_utf8_lossy(&kept);
    assert!(!text.contains("X-Injected"), "{text}");
    assert!(!m.recipients().iter().any(|r| r.contains("eve@")));
    assert_eq!(
        m.unsendable(),
        vec!["eve@example.com\r\nX-Injected: yes".to_string()]
    );
}
