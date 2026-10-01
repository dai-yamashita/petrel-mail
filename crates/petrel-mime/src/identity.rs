//! What makes a message the message it is: the test for whether two messages
//! that share a Message-ID are one message or two (docs/25 #78).
//!
//! A Message-ID is no secret. Anyone who has seen one can send a different
//! message under it, so sharing it proves nothing. And one message reaches a
//! mailbox as different bytes: the copy delivered to you carries trace headers
//! (Received, DKIM, Authentication-Results) the sender's copy in Sent does not,
//! a mailing list adds its List-* headers, a server re-encodes a body from
//! 8bit to quoted-printable or folds a header elsewhere. The identity reads
//! what a person reads and nothing a transport adds: who it is from and to,
//! the subject, the date, the ids that place it in a conversation, the words,
//! and the attachments. And the two addressing headers that decide who a
//! message speaks for and where its replies go: Reply-To, which a reply is
//! addressed to, and Sender, the mailbox that actually sent it on someone's
//! behalf. A copy that changes either changes what answering it does, so it
//! is another message (docs/25 review).
//!
//! List-Post, List-Unsubscribe and the other List-* headers stay out. A list
//! adds them to every copy it relays without changing a word anyone reads,
//! and with them in, the list's copy of your own post would no longer be the
//! message in your Sent folder.
//!
//! Bcc is left out on purpose. The sender's own copy keeps its Bcc header and
//! what was delivered does not, and the two are the same message.

use mail_parser::{MessageParser, MimeHeaders};

/// What reading the identity learned besides the digest the caller made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityFacts {
    /// Whether the message names blind copies: the sender's own copy of what
    /// they sent, where the Bcc header was kept.
    pub has_bcc: bool,
}

/// Feeds a message's identity to `feed`, field by field in a fixed order, so
/// the caller can hash it with whatever it hashes with. Each field goes as a
/// tag, a length and the bytes, so no two messages feed the same stream by
/// moving a boundary. `None` when the bytes do not parse.
pub fn feed_identity(raw: &[u8], feed: &mut dyn FnMut(&[u8])) -> Option<IdentityFacts> {
    let parsed = crate::parse_message(raw)?;
    // The attachments' bytes, which the parsed view does not keep. Parsed
    // from the same rewritten bytes `parse_message` reads, so the parts are
    // the ones it listed.
    let rewritten = crate::encoded_word::merge_adjacent_encoded_words(raw);
    let msg = MessageParser::default().parse(rewritten.as_ref())?;

    let mut field = |tag: &[u8], value: &[u8]| {
        feed(tag);
        feed(&(value.len() as u64).to_le_bytes());
        feed(value);
    };
    field(
        b"from-name",
        words(parsed.from_display.as_deref().unwrap_or("")).as_bytes(),
    );
    field(
        b"from-addr",
        address(parsed.from_addr.as_deref().unwrap_or("")).as_bytes(),
    );
    for (tag, list) in [(&b"to"[..], &parsed.to), (&b"cc"[..], &parsed.cc)] {
        field(tag, &(list.len() as u64).to_le_bytes());
        for (name, addr) in list {
            field(b"name", words(name.as_deref().unwrap_or("")).as_bytes());
            field(b"addr", address(addr).as_bytes());
        }
    }
    // Where a reply goes, as `parse_message` reads it for the composer.
    field(b"reply-to", &(parsed.reply_to.len() as u64).to_le_bytes());
    for (name, addr) in &parsed.reply_to {
        field(b"name", words(name.as_deref().unwrap_or("")).as_bytes());
        field(b"addr", address(addr).as_bytes());
    }
    // Who actually sent it, where that is not the author.
    let sender: Vec<(String, String)> = msg
        .sender()
        .map(|a| {
            a.iter()
                .map(|addr| {
                    (
                        words(addr.name().unwrap_or("")),
                        address(addr.address().unwrap_or("")),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    field(b"sender", &(sender.len() as u64).to_le_bytes());
    for (name, addr) in &sender {
        field(b"name", name.as_bytes());
        field(b"addr", addr.as_bytes());
    }
    field(
        b"subject",
        words(parsed.subject.as_deref().unwrap_or("")).as_bytes(),
    );
    match parsed.date_ms {
        Some(ms) => field(b"date", &ms.to_le_bytes()),
        None => field(b"no-date", b""),
    }
    field(
        b"message-id",
        parsed.message_id.as_deref().unwrap_or("").trim().as_bytes(),
    );
    // References, then In-Reply-To where References did not already name it:
    // what `parse_message` gathers as the message's place in a conversation.
    field(
        b"references",
        &(parsed.references.len() as u64).to_le_bytes(),
    );
    for id in &parsed.references {
        field(b"ref", id.trim().as_bytes());
    }
    field(b"text", body(&parsed.body_text).as_bytes());
    match &parsed.body_html {
        Some(html) => field(b"html", body(html).as_bytes()),
        None => field(b"no-html", b""),
    }
    let parts: Vec<_> = msg.attachments().collect();
    field(b"attachments", &(parts.len() as u64).to_le_bytes());
    for part in parts {
        field(
            b"attachment-name",
            part.attachment_name().unwrap_or("").as_bytes(),
        );
        field(b"attachment", part.contents());
    }
    Some(IdentityFacts {
        has_bcc: !parsed.bcc.is_empty(),
    })
}

/// A header's words, however it was folded: runs of whitespace are one space.
fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Addresses compare as mail does: without regard to case.
fn address(s: &str) -> String {
    s.trim().to_lowercase()
}

/// A body however it travelled: line endings as `\n`, and no trailing
/// whitespace on a line or at the end, which re-encoding in transit adds or
/// drops and nobody reads.
fn body(s: &str) -> String {
    let unified = s.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = unified.lines().map(str::trim_end).collect();
    lines.join("\n").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(raw: &[u8]) -> Option<(Vec<u8>, IdentityFacts)> {
        let mut stream = Vec::new();
        let facts = feed_identity(raw, &mut |b| stream.extend_from_slice(b))?;
        Some((stream, facts))
    }

    const SENT: &[u8] = b"From: Me <me@example.com>\r\nTo: Dana <dana@example.com>\r\n\
Subject: Lunch on Friday\r\nDate: Thu, 1 Oct 2026 08:00:00 +0000\r\n\
Message-ID: <lunch@example.com>\r\nIn-Reply-To: <ask@example.com>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\nThe caf\xc3\xa9 at noon?\r\n";

    /// The copy a recipient's server stores: trace headers, another header
    /// order, a folded subject, quoted-printable, and a trailing space.
    const DELIVERED: &[u8] = b"Received: from mx.example.com by imap.example.com\r\n\
Return-Path: <me@example.com>\r\nDelivered-To: dana@example.com\r\n\
DKIM-Signature: v=1; d=example.com; b=AAAA\r\n\
Authentication-Results: mx.example.com; dkim=pass\r\n\
X-Spam-Status: No\r\nList-Id: <lunch.example.com>\r\n\
Message-ID: <lunch@example.com>\r\nIn-Reply-To: <ask@example.com>\r\n\
Date: Thu, 1 Oct 2026 08:00:00 +0000\r\nSubject: Lunch\r\n on Friday\r\n\
To: Dana <DANA@example.com>\r\nFrom: Me <me@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
The caf=C3=A9 at noon? \r\n";

    #[test]
    fn transport_leaves_the_identity_alone() {
        assert_eq!(identity(SENT).unwrap().0, identity(DELIVERED).unwrap().0);
    }

    /// A list adds these to every copy it relays, and changes nothing anyone
    /// reads: the list's copy of your post is still the post in your Sent.
    #[test]
    fn list_headers_leave_the_identity_alone() {
        let relayed = String::from_utf8_lossy(SENT).replace(
            "MIME-Version",
            "List-Id: Lunch <lunch.lists.example>\r\n\
List-Post: <mailto:lunch@lists.example>\r\n\
List-Unsubscribe: <https://lists.example/u>\r\nPrecedence: list\r\nMIME-Version",
        );
        assert_eq!(
            identity(SENT).unwrap().0,
            identity(relayed.as_bytes()).unwrap().0
        );
    }

    #[test]
    fn a_bcc_line_is_reported_and_leaves_the_identity_alone() {
        let with_bcc = String::from_utf8_lossy(SENT).replace(
            "MIME-Version",
            "Bcc: Priya <priya@example.net>\r\nMIME-Version",
        );
        let (a, facts_a) = identity(SENT).unwrap();
        let (b, facts_b) = identity(with_bcc.as_bytes()).unwrap();
        assert_eq!(a, b);
        assert!(!facts_a.has_bcc);
        assert!(facts_b.has_bcc);
    }

    /// Whatever a person reads, changed, is another message.
    #[test]
    fn what_a_person_reads_is_the_identity() {
        let base = identity(SENT).unwrap().0;
        let changed = |from: &str, to: &str| {
            let text = String::from_utf8_lossy(SENT).replace(from, to);
            identity(text.as_bytes()).unwrap().0
        };
        for (from, to) in [
            (
                "MIME-Version",
                "Reply-To: Billing <billing@elsewhere.example>\r\nMIME-Version",
            ),
            (
                "MIME-Version",
                "Sender: Assistant <assistant@example.com>\r\nMIME-Version",
            ),
            ("Lunch on Friday", "[team] Lunch on Friday"),
            ("at noon?", "at one?"),
            ("Me <me@", "Mel <me@"),
            ("dana@example.com", "sam@example.com"),
            ("08:00:00", "09:00:00"),
            ("<ask@example.com>", "<other@example.com>"),
            ("<lunch@example.com>", "<dinner@example.com>"),
        ] {
            assert_ne!(changed(from, to), base, "{from} -> {to}");
        }
    }

    #[test]
    fn an_attachment_is_known_by_its_name_and_its_bytes() {
        let with = |name: &str, data: &str| {
            format!(
                "From: a@example.com\r\nTo: b@example.com\r\nSubject: s\r\n\
Message-ID: <att@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\n\
Content-Type: text/plain\r\n\r\nsee attached\r\n--b\r\n\
Content-Type: application/pdf; name=\"{name}\"\r\n\
Content-Disposition: attachment; filename=\"{name}\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{data}\r\n--b--\r\n"
            )
        };
        let a = identity(with("terms.pdf", "JVBERi0xLjQ=").as_bytes())
            .unwrap()
            .0;
        assert_ne!(
            a,
            identity(with("terms2.pdf", "JVBERi0xLjQ=").as_bytes())
                .unwrap()
                .0
        );
        assert_ne!(
            a,
            identity(with("terms.pdf", "JVBERi0xLjU=").as_bytes())
                .unwrap()
                .0
        );
        // The same bytes, wrapped differently, are the same attachment.
        assert_eq!(
            a,
            identity(with("terms.pdf", "JVBE\r\nRi0xLjQ=").as_bytes())
                .unwrap()
                .0
        );
    }
}
