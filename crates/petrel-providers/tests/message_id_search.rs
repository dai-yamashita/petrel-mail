//! A Message-ID search names exactly that Message-ID.
//!
//! IMAP's `SEARCH HEADER` is a substring match (RFC 3501 §6.4.4), and the id
//! went out without its angle brackets, so `5@host.example` found
//! `<15@host.example>` too. Without MOVE, a move first asks whether its copy
//! already landed: Archive holding `<15@host.example>` answered yes for
//! `5@host.example`, the COPY was skipped, and the source was expunged — the
//! message left the server and was copied nowhere. The drain's UID heal took
//! the last hit the same way, and could aim an action, Delete forever
//! included, at another message.
//!
//! The server below searches as servers do: a case-insensitive substring of
//! the header. It can also answer every search with everything (a server
//! whose search is fuzzy), and refuse a command carrying 8-bit bytes, which
//! a quoted string may not hold.
#![cfg(feature = "insecure-plaintext")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, ImapConfig, Security, Stored, copies_of_message_id, find_message_id, move_uid,
    uids_for_message_id,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone, Copy, PartialEq)]
enum Search {
    /// A case-insensitive substring of the Message-ID header, as RFC 3501
    /// defines it and Dovecot, Cyrus and Exchange implement it.
    Substring,
    /// Every message in the mailbox, whatever was asked.
    Everything,
}

struct Message {
    uid: u32,
    raw: Vec<u8>,
    /// INTERNALDATE: when it arrived. A COPY keeps it.
    date: String,
    deleted: bool,
}

struct World {
    caps: &'static str,
    search: Search,
    /// BAD for a command line with a byte over 0x7f in it.
    strict_ascii: bool,
    boxes: BTreeMap<String, Vec<Message>>,
    next: BTreeMap<String, u32>,
    /// Every command line, as bytes.
    lines: Vec<Vec<u8>>,
    /// Arrivals so far, for each one's own INTERNALDATE.
    arrivals: u32,
}

type Shared = Arc<Mutex<World>>;

fn world(caps: &'static str, search: Search, boxes: &[&str]) -> Shared {
    Arc::new(Mutex::new(World {
        caps,
        search,
        strict_ascii: false,
        boxes: boxes.iter().map(|b| (b.to_string(), Vec::new())).collect(),
        next: boxes.iter().map(|b| (b.to_string(), 1)).collect(),
        lines: Vec::new(),
        arrivals: 0,
    }))
}

/// A message arriving: its own INTERNALDATE, a second after the last.
fn put(w: &Shared, mailbox: &str, raw: Vec<u8>) -> u32 {
    let date = {
        let mut g = w.lock().unwrap();
        g.arrivals += 1;
        format!(
            "01-Oct-2026 09:{:02}:{:02} +0000",
            g.arrivals / 60,
            g.arrivals % 60
        )
    };
    put_dated(w, mailbox, raw, date)
}

fn put_dated(w: &Shared, mailbox: &str, raw: Vec<u8>, date: String) -> u32 {
    let mut w = w.lock().unwrap();
    let uid = w.next[mailbox];
    w.next.insert(mailbox.to_string(), uid + 1);
    w.boxes.get_mut(mailbox).unwrap().push(Message {
        uid,
        raw,
        date,
        deleted: false,
    });
    uid
}

/// What a COPY that landed left: the same bytes, the same arrival.
fn landed(w: &Shared, from: &str, uid: u32, to: &str) -> u32 {
    let (raw, date) = {
        let g = w.lock().unwrap();
        let m = g.boxes[from].iter().find(|m| m.uid == uid).unwrap();
        (m.raw.clone(), m.date.clone())
    };
    put_dated(w, to, raw, date)
}

fn held(w: &Shared, mailbox: &str) -> Vec<Vec<u8>> {
    w.lock().unwrap().boxes[mailbox]
        .iter()
        .map(|m| m.raw.clone())
        .collect()
}

fn said(w: &Shared, what: &str) -> bool {
    w.lock().unwrap().lines.iter().any(|l| {
        String::from_utf8_lossy(l)
            .to_ascii_uppercase()
            .contains(what)
    })
}

fn message(id: &str, body: &str) -> Vec<u8> {
    format!(
        "From: a@example.com\r\nTo: b@example.com\r\nSubject: s\r\nMessage-ID: <{id}>\r\n\
         MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

/// The Message-ID header of a message, folded lines included, as the
/// header block a `HEADER.FIELDS (MESSAGE-ID)` fetch returns.
fn id_header(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let mut out = String::new();
    let mut inside = false;
    for line in head.split("\r\n") {
        if line.starts_with(' ') || line.starts_with('\t') {
            if inside {
                out.push_str(line);
                out.push_str("\r\n");
            }
            continue;
        }
        inside = line.to_ascii_lowercase().starts_with("message-id:");
        if inside {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("\r\n");
    out
}

/// The quoted argument after `HEADER Message-ID`, unescaped.
fn quoted_term(line: &str) -> String {
    let at = line.to_ascii_uppercase().find("MESSAGE-ID").unwrap_or(0);
    let rest = &line[at..];
    let Some(start) = rest.find('"') else {
        return String::new();
    };
    let mut out = String::new();
    let mut chars = rest[start + 1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '"' => break,
            c => out.push(c),
        }
    }
    out
}

/// The quoted strings on a command line, in order.
fn quoted(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

fn uid_set(spec: &str) -> Vec<(u32, u32)> {
    spec.split(',')
        .filter_map(|part| match part.split_once(':') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().unwrap_or(u32::MAX))),
            None => {
                let n = part.parse().ok()?;
                Some((n, n))
            }
        })
        .collect()
}

fn answer(w: &Shared, selected: &mut Option<String>, raw_line: &[u8]) -> (Vec<u8>, bool) {
    let line = String::from_utf8_lossy(raw_line).to_string();
    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
    let up = line.to_ascii_uppercase();
    let mut w = w.lock().unwrap();
    w.lines.push(raw_line.to_vec());
    let ok = |s: &str| format!("{tag} {s}\r\n").into_bytes();
    if w.strict_ascii && raw_line.iter().any(|b| *b > 0x7f) {
        return (ok("BAD 8-bit data in a quoted string"), false);
    }
    if up.contains(" LOGOUT") {
        return (format!("* BYE\r\n{tag} OK bye\r\n").into_bytes(), true);
    }
    if up.contains(" CAPABILITY") {
        return (
            format!("* CAPABILITY IMAP4rev1 {}\r\n{tag} OK done\r\n", w.caps).into_bytes(),
            false,
        );
    }
    if up.contains(" LOGIN ") {
        return (ok("OK signed in"), false);
    }
    if up.contains(" SELECT ") || up.contains(" EXAMINE ") {
        let name = quoted(&line).into_iter().next().unwrap_or_default();
        let Some(b) = w.boxes.get(&name) else {
            return (ok("NO [NONEXISTENT] no such mailbox"), false);
        };
        *selected = Some(name.clone());
        return (
            format!(
                "* {} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {}] ok\r\n{tag} OK done\r\n",
                b.len(),
                w.next[&name]
            )
            .into_bytes(),
            false,
        );
    }
    let Some(sel) = selected.clone() else {
        return (ok("OK nothing selected"), false);
    };
    if up.contains(" SEARCH ") && up.contains("HEADER MESSAGE-ID") {
        let term = quoted_term(&line).to_lowercase();
        let hits: Vec<String> = w.boxes[&sel]
            .iter()
            .filter(|m| match w.search {
                Search::Everything => true,
                Search::Substring => {
                    let header = id_header(&m.raw);
                    let value = header.split_once(':').map(|(_, v)| v).unwrap_or("");
                    let value = value.replace("\r\n", "").to_lowercase();
                    !term.is_empty() && value.contains(&term)
                }
            })
            .map(|m| m.uid.to_string())
            .collect();
        return (
            format!("* SEARCH {}\r\n{tag} OK searched\r\n", hits.join(" ")).into_bytes(),
            false,
        );
    }
    if up.contains(" UID FETCH ") {
        let spec = line.split_whitespace().nth(3).unwrap_or("");
        let ranges = uid_set(spec);
        let mut out = Vec::new();
        for (seq, m) in w.boxes[&sel].iter().enumerate() {
            if !ranges.iter().any(|(a, b)| m.uid >= *a && m.uid <= *b) {
                continue;
            }
            let mut item = format!("* {} FETCH (UID {}", seq + 1, m.uid).into_bytes();
            if up.contains("RFC822.SIZE") {
                item.extend(format!(" RFC822.SIZE {}", m.raw.len()).bytes());
            }
            if up.contains("INTERNALDATE") {
                item.extend(format!(" INTERNALDATE \"{}\"", m.date).bytes());
            }
            if up.contains("HEADER.FIELDS") {
                let header = id_header(&m.raw);
                item.extend(
                    format!(" BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n", header.len()).bytes(),
                );
                item.extend(header.bytes());
            }
            item.extend(b")\r\n");
            out.extend(item);
        }
        out.extend(ok("OK fetched"));
        return (out, false);
    }
    // COPY, MOVE, STORE and EXPUNGE are answered before this is reached.
    (ok("OK done"), false)
}

/// COPY and MOVE need the lock released to call `put`, so they are answered
/// here rather than in `answer`.
fn copy_or_move(w: &Shared, selected: &Option<String>, raw_line: &[u8]) -> Option<Vec<u8>> {
    let line = String::from_utf8_lossy(raw_line).to_string();
    let up = line.to_ascii_uppercase();
    if !(up.contains(" UID COPY ") || up.contains(" UID MOVE ")) {
        return None;
    }
    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
    w.lock().unwrap().lines.push(raw_line.to_vec());
    let sel = selected.clone()?;
    let uid: u32 = line
        .split_whitespace()
        .nth(3)
        .and_then(|u| u.parse().ok())
        .unwrap_or(0);
    let to = quoted(&line).pop().unwrap_or_default();
    let (raw, date) = {
        let mut g = w.lock().unwrap();
        let Some(pos) = g.boxes[&sel].iter().position(|m| m.uid == uid) else {
            return Some(format!("{tag} OK nothing to do\r\n").into_bytes());
        };
        if up.contains(" UID MOVE ") {
            let m = g.boxes.get_mut(&sel).unwrap().remove(pos);
            (m.raw, m.date)
        } else {
            let m = &g.boxes[&sel][pos];
            (m.raw.clone(), m.date.clone())
        }
    };
    put_dated(w, &to, raw, date);
    Some(format!("{tag} OK done\r\n").into_bytes())
}

/// STORE \Deleted and UID EXPUNGE, on the selected mailbox.
fn store_or_expunge(w: &Shared, selected: &Option<String>, raw_line: &[u8]) -> Option<Vec<u8>> {
    let line = String::from_utf8_lossy(raw_line).to_string();
    let up = line.to_ascii_uppercase();
    let storing = up.contains(" UID STORE ") && up.contains("\\DELETED");
    let expunging = up.contains(" UID EXPUNGE ");
    if !(storing || expunging) {
        return None;
    }
    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
    let mut g = w.lock().unwrap();
    g.lines.push(raw_line.to_vec());
    let sel = selected.clone()?;
    let uid: u32 = line
        .split_whitespace()
        .nth(3)
        .and_then(|u| u.parse().ok())
        .unwrap_or(0);
    let mailbox = g.boxes.get_mut(&sel).unwrap();
    if storing {
        for m in mailbox.iter_mut().filter(|m| m.uid == uid) {
            m.deleted = true;
        }
    } else {
        mailbox.retain(|m| !(m.deleted && m.uid == uid));
    }
    Some(format!("{tag} OK done\r\n").into_bytes())
}

async fn serve(w: Shared) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let w = Arc::clone(&w);
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                let mut selected = None;
                let mut line = Vec::new();
                loop {
                    line.clear();
                    if reader.read_until(b'\n', &mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let strict = w.lock().unwrap().strict_ascii && line.iter().any(|b| *b > 0x7f);
                    let (reply, close) = if strict {
                        answer(&w, &mut selected, &line)
                    } else if let Some(r) = copy_or_move(&w, &selected, &line) {
                        (r, false)
                    } else if let Some(r) = store_or_expunge(&w, &selected, &line) {
                        (r, false)
                    } else {
                        answer(&w, &mut selected, &line)
                    };
                    if tx.write_all(&reply).await.is_err() || close {
                        let _ = tx.shutdown().await;
                        return;
                    }
                }
            });
        }
    });
    port
}

/// The message as the store would hold it.
fn stored<'a>(message_id: &'a str, raw: &[u8]) -> Stored<'a> {
    Stored {
        message_id,
        size: Some(raw.len() as u32),
    }
}

fn cfg(port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p"),
        security: Security::InsecurePlaintext,
    }
}

#[tokio::test]
async fn archiving_5_copies_it_although_the_archive_holds_15() {
    // The reviewer's case: no MOVE, UIDPLUS, and an Archive holding an id
    // the one being archived is a substring of.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    let five = message("5@host.example", "the one being archived");
    put(&w, "INBOX", five.clone());
    put(&w, "Archive", message("15@host.example", "another one"));
    let port = serve(Arc::clone(&w)).await;
    let expunged = move_uid(
        &cfg(port),
        "INBOX",
        1,
        "Archive",
        false,
        false,
        Some(stored("5@host.example", &five)),
    )
    .await
    .unwrap();
    assert!(expunged);
    assert!(said(&w, "UID COPY 1"), "the copy was made");
    assert!(held(&w, "INBOX").is_empty());
    let archive = held(&w, "Archive");
    assert_eq!(archive.len(), 2, "both are in the Archive");
    assert!(archive.contains(&five), "the archived message is there");
}

#[tokio::test]
async fn a_move_retried_after_its_copy_landed_does_not_copy_again() {
    // The case the check is for: the COPY landed, the STORE did not.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    let five = message("5@host.example", "body");
    put(&w, "INBOX", five.clone());
    landed(&w, "INBOX", 1, "Archive");
    let port = serve(Arc::clone(&w)).await;
    move_uid(
        &cfg(port),
        "INBOX",
        1,
        "Archive",
        false,
        false,
        Some(stored("<5@host.example>", &five)),
    )
    .await
    .unwrap();
    assert!(!said(&w, "UID COPY"), "no second copy");
    assert!(held(&w, "INBOX").is_empty());
    assert_eq!(held(&w, "Archive"), vec![five]);
}

#[tokio::test]
async fn another_message_carrying_the_same_id_is_not_taken_for_the_copy() {
    // A mailing list's copy of your own post, or a message sent again under
    // its old id: the same Message-ID, other bytes. Already in the Archive,
    // it is not the copy this move would make, and skipping the COPY would
    // expunge the one being archived with nothing to show for it.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    let mine = message("5@host.example", "as I sent it");
    let theirs = message("5@host.example", "as I sent it\r\n-- \r\nlist footer");
    put(&w, "INBOX", mine.clone());
    put(&w, "Archive", theirs.clone());
    let port = serve(Arc::clone(&w)).await;
    move_uid(
        &cfg(port),
        "INBOX",
        1,
        "Archive",
        false,
        false,
        Some(stored("5@host.example", &mine)),
    )
    .await
    .unwrap();
    assert!(said(&w, "UID COPY 1"));
    let archive = held(&w, "Archive");
    assert!(archive.contains(&mine) && archive.contains(&theirs));
}

#[tokio::test]
async fn two_other_messages_under_the_id_are_not_the_copy_either() {
    // Two different messages already filed under this Message-ID, neither
    // of them this one: the copy is made.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    let mine = message("5@host.example", "the real invoice");
    put(&w, "INBOX", mine.clone());
    put(
        &w,
        "Archive",
        message("5@host.example", "a forgery of the invoice"),
    );
    put(
        &w,
        "Archive",
        message("5@host.example", "the list's copy, with a footer"),
    );
    let port = serve(Arc::clone(&w)).await;
    move_uid(
        &cfg(port),
        "INBOX",
        1,
        "Archive",
        false,
        true,
        Some(stored("5@host.example", &mine)),
    )
    .await
    .unwrap();
    assert!(said(&w, "UID COPY 1"));
    assert!(held(&w, "INBOX").is_empty());
    let archive = held(&w, "Archive");
    assert_eq!(archive.len(), 3);
    assert!(archive.contains(&mine), "the real one is filed, not lost");
}

#[tokio::test]
async fn a_forgery_padded_to_the_same_size_is_not_the_copy() {
    // Size is cheap to match on purpose. Arrival is not: the server sets it
    // when the forgery is delivered, and a COPY keeps the original's.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    let mine = message("5@host.example", "pay to 11-1111-1111");
    let forged = message("5@host.example", "pay to 99-9999-9999");
    assert_eq!(mine.len(), forged.len());
    put(&w, "INBOX", mine.clone());
    put(&w, "Archive", forged.clone());
    let port = serve(Arc::clone(&w)).await;
    move_uid(
        &cfg(port),
        "INBOX",
        1,
        "Archive",
        false,
        true,
        Some(stored("5@host.example", &mine)),
    )
    .await
    .unwrap();
    assert!(said(&w, "UID COPY 1"));
    assert!(held(&w, "Archive").contains(&mine));
}

#[tokio::test]
async fn a_landed_copy_of_another_size_than_the_store_holds_is_copied_again() {
    // The store's own size is the last word: a copy the server describes
    // otherwise is not taken on trust.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    let five = message("5@host.example", "body");
    put(&w, "INBOX", five.clone());
    landed(&w, "INBOX", 1, "Archive");
    let port = serve(Arc::clone(&w)).await;
    move_uid(
        &cfg(port),
        "INBOX",
        1,
        "Archive",
        false,
        true,
        Some(Stored {
            message_id: "5@host.example",
            size: Some(five.len() as u32 + 1),
        }),
    )
    .await
    .unwrap();
    assert!(said(&w, "UID COPY 1"));
}

#[tokio::test]
async fn a_key_the_store_made_matches_nothing() {
    // The store keys a second message under a reused id `<id>::b-<hash>`;
    // the server has never seen that, so it is never taken for a landed
    // copy or healed onto the message it shares an id with.
    let w = world("UIDPLUS", Search::Everything, &["INBOX"]);
    put(&w, "INBOX", message("5@host.example", "a"));
    let port = serve(Arc::clone(&w)).await;
    for key in [
        "5@host.example::b-0123456789abcdef",
        "5@host.example::copy-3",
    ] {
        let copies = copies_of_message_id(&cfg(port), "INBOX", key)
            .await
            .unwrap();
        assert!(copies.is_empty(), "{key}: {copies:?}");
    }
}

#[tokio::test]
async fn a_search_that_answers_loosely_is_checked_exactly() {
    let w = world("UIDPLUS", Search::Everything, &["Drafts"]);
    put(&w, "Drafts", message("other@host.example", "a"));
    let wanted = put(&w, "Drafts", message("d1@host.example", "b"));
    put(&w, "Drafts", message("x-d1@host.example", "c"));
    let port = serve(Arc::clone(&w)).await;
    for asked in [
        "d1@host.example",
        "<d1@host.example>",
        " <d1@host.example> ",
    ] {
        let uids = uids_for_message_id(&cfg(port), "Drafts", asked)
            .await
            .unwrap();
        assert_eq!(uids, vec![wanted], "{asked}");
        let found = find_message_id(&cfg(port), "Drafts", asked).await.unwrap();
        assert_eq!(found.len(), 1, "{asked}");
    }
    let none = uids_for_message_id(&cfg(port), "Drafts", "nobody@host.example")
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn the_search_carries_the_angle_brackets() {
    let w = world("UIDPLUS", Search::Substring, &["INBOX"]);
    put(&w, "INBOX", message("15@host.example", "a"));
    let five = put(&w, "INBOX", message("5@host.example", "b"));
    let port = serve(Arc::clone(&w)).await;
    let uids = uids_for_message_id(&cfg(port), "INBOX", "5@host.example")
        .await
        .unwrap();
    assert_eq!(uids, vec![five]);
    assert!(said(&w, "HEADER MESSAGE-ID \"<5@HOST.EXAMPLE>\""));
}

#[tokio::test]
async fn an_id_that_differs_only_in_case_is_another_message() {
    // Searches are case-insensitive; Message-IDs, as stored and threaded,
    // are not.
    let w = world("UIDPLUS", Search::Substring, &["INBOX"]);
    put(&w, "INBOX", message("ABC@host.example", "a"));
    let port = serve(Arc::clone(&w)).await;
    let uids = uids_for_message_id(&cfg(port), "INBOX", "abc@host.example")
        .await
        .unwrap();
    assert!(uids.is_empty(), "{uids:?}");
}

#[tokio::test]
async fn several_copies_of_one_id_are_all_named_in_order() {
    // The draft push appends a new revision beside the old one and takes
    // the newest: every exact copy, sorted.
    let w = world("UIDPLUS", Search::Substring, &["Drafts"]);
    let first = put(&w, "Drafts", message("d2@host.example", "first"));
    put(&w, "Drafts", message("d22@host.example", "another draft"));
    let second = put(&w, "Drafts", message("d2@host.example", "second"));
    let port = serve(Arc::clone(&w)).await;
    let uids = uids_for_message_id(&cfg(port), "Drafts", "d2@host.example")
        .await
        .unwrap();
    assert_eq!(uids, vec![first, second]);
}

#[tokio::test]
async fn an_eight_bit_id_is_searched_in_ascii_and_still_found() {
    // docs/25 #119. A quoted string cannot carry 8-bit bytes; strict servers
    // answer BAD, which a move read as "not copied yet" and the drain as a
    // failed search, every time, forever.
    let w = world("UIDPLUS", Search::Substring, &["INBOX", "Archive"]);
    w.lock().unwrap().strict_ascii = true;
    put(&w, "INBOX", message("x-42@host.example", "a decoy"));
    let wanted = put(&w, "INBOX", message("grüße-42@host.example", "b"));
    let port = serve(Arc::clone(&w)).await;
    let uids = uids_for_message_id(&cfg(port), "INBOX", "grüße-42@host.example")
        .await
        .unwrap();
    assert_eq!(uids, vec![wanted]);
    let lines = w.lock().unwrap().lines.clone();
    assert!(
        lines.iter().all(|l| l.iter().all(|b| *b <= 0x7f)),
        "nothing 8-bit went over the wire"
    );
    // And the move's check: the copy that landed is recognised.
    let raw = held(&w, "INBOX")[1].clone();
    landed(&w, "INBOX", wanted, "Archive");
    move_uid(
        &cfg(port),
        "INBOX",
        wanted,
        "Archive",
        false,
        true,
        Some(stored("grüße-42@host.example", &raw)),
    )
    .await
    .unwrap();
    assert!(!said(&w, "UID COPY"), "the landed copy was recognised");
    assert_eq!(held(&w, "Archive").len(), 1);
}

#[tokio::test]
async fn an_id_with_too_little_ascii_to_search_by_is_an_error_not_absence() {
    // Absent is an answer callers act on: Gmail's Sent check reads it as
    // "did not go" and sends again. An id there is no safe way to ask about
    // is not absent; it is unknown.
    let w = world("UIDPLUS", Search::Substring, &["Sent"]);
    let port = serve(Arc::clone(&w)).await;
    let got = uids_for_message_id(&cfg(port), "Sent", "日本語@例え.jp").await;
    assert!(got.is_err(), "{got:?}");
    let got = find_message_id(&cfg(port), "Sent", "日本語@例え.jp").await;
    assert!(got.is_err(), "{got:?}");
}
