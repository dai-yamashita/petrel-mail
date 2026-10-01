//! Two messages that share a Message-ID are told apart by what they say, not
//! by which arrived first (docs/25 #78, round two).
//!
//! A Message-ID is no secret, so a stranger can send one that a stored
//! message already carries, and the order a sync meets them in is no guide to
//! which is real: a fresh sync fetches the newest mail first and backfills the
//! rest, and INBOX is watched while Sent is only swept. So the rule is content
//! identity. The same message seen twice — sent to yourself, delivered under
//! an alias, fetched again with other trace headers — is one row. A different
//! message under the same Message-ID is a message of its own: new mail, its
//! own words, its own flags.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{DraftEnvelope, ListView, Sort, Store, flags};

fn msg(mid: &str, from: &str, subject: &str, date: &str, extra: &str, body: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Date: {date}\r\n\
         Message-ID: <{mid}>\r\n\
         {extra}\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         {body}\r\n"
    )
    .into_bytes()
}

fn invoice() -> Vec<u8> {
    msg(
        "inv-1001@vendor.example",
        "Vendor Billing <billing@vendor.example>",
        "Invoice 1001",
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "",
        "Please remit to Kestrel Bank, account 11-1111.",
    )
}

fn forgery() -> Vec<u8> {
    msg(
        "inv-1001@vendor.example",
        "Vendor Billing <billing@vendor.example>",
        "Invoice 1001",
        "Wed, 30 Sep 2026 09:00:00 +0000",
        "",
        "Please remit to Osprey Bank, account 99-9999.",
    )
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    store.set_active_account(account).expect("active");
    (dir, store, blobs, account)
}

fn hits(store: &Store, word: &str) -> usize {
    store.search(word, 10).expect("search").len()
}

fn listed(store: &Store, view: &ListView) -> Vec<(String, String)> {
    store
        .list_threads(view, 0, 50, Sort::default())
        .expect("list")
        .into_iter()
        .map(|r| (r.subject, r.snippet))
        .collect()
}

fn folder_view(role: &str) -> ListView {
    ListView::Folder(role.into())
}

// ---- the reviewer's probes, asserting what should happen ------------------

/// p78a. On a fresh sync the forgery in INBOX is fetched before the real
/// invoice is backfilled into Receipts. Order must not decide: each is its
/// own message, and Receipts shows the invoice's own words.
#[test]
fn a_forgery_stored_first_does_not_become_the_real_invoice() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();

    let forged = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();

    assert_ne!(real.message_id, forged.message_id, "two messages");
    assert!(
        real.was_new,
        "the backfilled invoice is a message of its own"
    );
    let rows = listed(&store, &ListView::UserFolder(receipts));
    assert_eq!(rows.len(), 1);
    assert!(rows[0].1.contains("Kestrel"), "{:?}", rows[0]);
    assert_eq!(hits(&store, "kestrel"), 1);
    assert_eq!(
        hits(&store, "osprey"),
        1,
        "the forgery is still there to be seen"
    );
    assert_eq!(store.placement_uid(real.message_id, inbox).unwrap(), None);
    store.fts_integrity_check().expect("index consistent");
}

/// p78b. The list's copy of your post reaches INBOX before your Sent copy is
/// swept. Sent shows what you sent.
#[test]
fn sent_shows_what_was_sent_when_the_list_copy_arrives_first() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let (posted, tagged) = trip_notes();
    store
        .ingest_raw(&blobs, account, Some(inbox), Some(40), &tagged)
        .unwrap();
    store
        .ingest_raw(&blobs, account, Some(sent), Some(3), &posted)
        .unwrap();
    let rows = listed(&store, &folder_view("sent"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "Trip notes", "Sent shows the post as written");
    assert!(!rows[0].1.contains("unsubscribe"), "{:?}", rows[0]);
    store.fts_integrity_check().expect("index consistent");
}

/// p78c. A sender that reuses one Message-ID (a scanner, a script, a ticket
/// system) sends a new message after the first was archived. It is new mail:
/// listed in INBOX, searchable, and announced.
#[test]
fn a_new_message_under_a_reused_id_is_new_mail() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    let scan = |date: &str, body: &str| {
        msg(
            "scan@printer.local",
            "Office Scanner <scanner@example.com>",
            "Scanned document",
            date,
            "",
            body,
        )
    };
    let first = store
        .ingest_raw(
            &blobs,
            account,
            Some(archive),
            Some(3),
            &scan(
                "Mon, 7 Sep 2026 09:00:00 +0000",
                "Scan of the lease agreement.",
            ),
        )
        .unwrap();
    let second = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(50),
            &scan(
                "Wed, 30 Sep 2026 09:00:00 +0000",
                "Scan of the passport renewal form.",
            ),
        )
        .unwrap();

    assert_ne!(first.message_id, second.message_id);
    assert!(second.was_new, "announced, and rules run on it");
    let inbox_rows = listed(&store, &ListView::Inbox);
    assert!(
        inbox_rows.iter().any(|(_, s)| s.contains("passport")),
        "{inbox_rows:?}"
    );
    assert!(
        !inbox_rows.iter().any(|(_, s)| s.contains("lease")),
        "the old scan stays archived: {inbox_rows:?}"
    );
    assert_eq!(hits(&store, "passport"), 1);
    assert_eq!(hits(&store, "lease"), 1);
}

/// p78d. The shell writes the server's flags onto whatever row an ingest
/// returns. A stranger's copy is its own row, so its flags are its own.
#[test]
fn a_strangers_copy_has_its_own_read_and_starred_state() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();
    store
        .set_message_flags(real.message_id, flags::SEEN | flags::FLAGGED)
        .unwrap();
    // As `ingest_fenced` does with the forgery's flags from the server.
    let later = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    store.set_message_flags(later.message_id, 0).unwrap();

    assert_ne!(later.message_id, real.message_id);
    assert_eq!(
        store.flags_of(real.message_id).unwrap(),
        flags::SEEN | flags::FLAGGED,
        "the invoice is still read and starred"
    );
}

/// p78e. Landing in Drafts is no licence to rewrite a message that is not a
/// draft: a filter rule or a plus-address delivery can file mail there.
#[test]
fn a_copy_landing_in_drafts_does_not_rewrite_a_message_that_is_not_a_draft() {
    let (_dir, mut store, blobs, account) = setup();
    let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();
    let later = store
        .ingest_raw(&blobs, account, Some(drafts), Some(9), &forgery())
        .unwrap();
    assert_ne!(later.message_id, real.message_id);
    let rows = listed(&store, &ListView::UserFolder(receipts));
    assert!(rows[0].1.contains("Kestrel"), "{rows:?}");
    assert_eq!(hits(&store, "kestrel"), 1);
}

/// p78f. A draft written in another client is stored, then the message is
/// sent under the same Message-ID and its Sent copy says something else.
/// Sent shows what went out, and finding it by its words works.
#[test]
fn a_message_sent_from_another_client_shows_what_was_sent() {
    let (_dir, mut store, blobs, account) = setup();
    let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let draft = msg(
        "d7@phone.example",
        "Me <me@example.com>",
        "Offer",
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "X-Uniform-Type-Identifier: com.apple.mail-draft\r\n",
        "We can offer 40,000.",
    );
    let sent_copy = msg(
        "d7@phone.example",
        "Me <me@example.com>",
        "Offer",
        "Mon, 7 Sep 2026 09:05:00 +0000",
        "",
        "We can offer 45,000, final.",
    );
    store
        .ingest_raw(&blobs, account, Some(drafts), Some(4), &draft)
        .unwrap();
    let s = store
        .ingest_raw(&blobs, account, Some(sent), Some(30), &sent_copy)
        .unwrap();
    // The other client removes its draft, and the Drafts pass sees it gone.
    store
        .remove_placements_absent(drafts, &std::collections::HashSet::new())
        .unwrap();

    let rows = listed(&store, &folder_view("sent"));
    assert_eq!(rows.len(), 1);
    assert!(rows[0].1.contains("45,000"), "{rows:?}");
    assert_eq!(hits(&store, "final"), 1);
    assert!(s.was_new);
    assert!(listed(&store, &folder_view("drafts")).is_empty());
}

/// p108a. "To: me, Bcc: the parents": the copy delivered to INBOX has no Bcc
/// line, and the sender's copy in Sent has one. They are the same message,
/// so one row, and in either order it shows its blind copies.
#[test]
fn your_own_copy_keeps_its_bcc_line_in_either_order() {
    for wire_first in [true, false] {
        let (_dir, mut store, blobs, account) = setup();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
        let wire = b"Received: from submit.example.com by mx.example.com\r\n\
From: Me <me@example.com>\r\nTo: me@example.com\r\n\
Subject: Field trip\r\nDate: Mon, 7 Sep 2026 09:00:00 +0000\r\n\
Message-ID: <trip@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nBuses leave at eight.\r\n";
        let sender_copy = b"From: Me <me@example.com>\r\nTo: me@example.com\r\n\
Subject: Field trip\r\nDate: Mon, 7 Sep 2026 09:00:00 +0000\r\n\
Message-ID: <trip@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Bcc: Priya Nair <priya@example.net>, parents@example.org\r\n\r\nBuses leave at eight.\r\n";
        let (a, b) = if wire_first {
            let a = store
                .ingest_raw(&blobs, account, Some(inbox), Some(12), wire)
                .unwrap();
            let b = store
                .ingest_raw(&blobs, account, Some(sent), Some(4), sender_copy)
                .unwrap();
            (a, b)
        } else {
            let b = store
                .ingest_raw(&blobs, account, Some(sent), Some(4), sender_copy)
                .unwrap();
            let a = store
                .ingest_raw(&blobs, account, Some(inbox), Some(12), wire)
                .unwrap();
            (a, b)
        };
        assert_eq!(
            a.message_id, b.message_id,
            "one message (wire first: {wire_first})"
        );
        let thread = store.thread_of(b.message_id).unwrap().unwrap();
        let detail = store.thread_detail(thread).unwrap();
        assert_eq!(detail.len(), 1);
        assert!(
            detail[0].bcc.iter().any(|n| n.contains("Priya")),
            "wire first: {wire_first}: {:?}",
            detail[0].bcc
        );
        assert_eq!(hits(&store, "priya"), 1, "wire first: {wire_first}");
        assert_eq!(
            store.placement_uid(a.message_id, inbox).unwrap(),
            Some(Some(12))
        );
        assert_eq!(
            store.placement_uid(a.message_id, sent).unwrap(),
            Some(Some(4))
        );
        store.fts_integrity_check().expect("index consistent");
    }
}

/// p92a, docs/24 #33. Taking the server's revision of a draft re-indexes it
/// in the same step: found by its new words, not by its old ones.
#[test]
fn taking_the_servers_revision_reindexes_the_draft() {
    let (dir, mut store, blobs, account) = setup();
    let id = store
        .save_draft_full(
            account,
            None,
            "dana@example.com",
            "",
            "Plan v1",
            "first words kestrel",
            "",
            &DraftEnvelope::default(),
        )
        .unwrap();
    store.set_draft_msgid(id, "d1@example.com").unwrap();
    let drafts = store.folder_for_role(account, "drafts").unwrap().unwrap();
    let revision = msg(
        "d1@example.com",
        "Me <me@example.com>",
        "Plan v2",
        "Wed, 30 Sep 2026 09:00:00 +0000",
        "",
        "second words osprey",
    );
    let other = store
        .ingest_raw_second_copy(&blobs, account, Some(drafts), 9, &revision)
        .unwrap()
        .message_id;
    assert_eq!(store.draft_conflict(id).unwrap(), Some((other, Some(9))));

    store
        .adopt_server_revision(id, "Plan v2", "second words osprey", "", Some(9))
        .unwrap();
    store.retire_second_copy(other).unwrap();

    let c = rusqlite::Connection::open(dir.path().join("petrel.db")).unwrap();
    let indexed: String = c
        .query_row(
            "SELECT subject || ' / ' || body_text FROM fts_content WHERE message_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, "Plan v2 / second words osprey");
    let found = store.search("osprey", 10).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].message_id, id);
    assert!(store.search("kestrel", 10).unwrap().is_empty());
    assert_eq!(hits(&store, "dana"), 1, "still found by its recipient");
    let snippet: String = c
        .query_row("SELECT snippet FROM messages WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(snippet.contains("second words"), "{snippet}");
    store.fts_integrity_check().expect("index consistent");
}

// ---- the same message, seen twice -----------------------------------------

/// Sent to yourself: the delivered copy carries trace headers, its own header
/// order and another transfer encoding; the sender's copy in Sent carries
/// none of that. The same message, so one row, whichever arrives first.
#[test]
fn a_message_sent_to_yourself_is_one_row_in_either_order() {
    let sender_copy = b"From: Me <me@example.com>\r\nTo: me@example.com\r\n\
Subject: Notes for Friday and the plan for the long weekend away\r\n\
Date: Thu, 1 Oct 2026 08:00:00 +0000\r\nMessage-ID: <self-1@example.com>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\nMeet at the caf\xc3\xa9 at nine.\r\n"
        .to_vec();
    let delivered = b"Return-Path: <me@example.com>\r\nDelivered-To: me@example.com\r\n\
Received: from mx.example.com by imap.example.com; Thu, 1 Oct 2026 08:00:02 +0000\r\n\
Received: from submit.example.com by mx.example.com; Thu, 1 Oct 2026 08:00:01 +0000\r\n\
DKIM-Signature: v=1; a=rsa-sha256; d=example.com; s=s1; b=AAAA\r\n\
Authentication-Results: mx.example.com; dkim=pass header.d=example.com\r\n\
X-Spam-Status: No, score=-1.0\r\n\
Message-ID: <self-1@example.com>\r\nDate: Thu, 1 Oct 2026 08:00:00 +0000\r\n\
Subject: Notes for Friday and the plan for the long weekend\r\n away\r\n\
To: me@example.com\r\nFrom: Me <me@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
Meet at the caf=C3=A9 at nine.\r\n"
        .to_vec();
    for delivered_first in [true, false] {
        let (_dir, mut store, blobs, account) = setup();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
        let order: [(i64, u32, &[u8]); 2] = if delivered_first {
            [(inbox, 21, &delivered), (sent, 8, &sender_copy)]
        } else {
            [(sent, 8, &sender_copy), (inbox, 21, &delivered)]
        };
        let first = store
            .ingest_raw(
                &blobs,
                account,
                Some(order[0].0),
                Some(order[0].1),
                order[0].2,
            )
            .unwrap();
        let second = store
            .ingest_raw(
                &blobs,
                account,
                Some(order[1].0),
                Some(order[1].1),
                order[1].2,
            )
            .unwrap();
        assert_eq!(
            first.message_id, second.message_id,
            "delivered first: {delivered_first}"
        );
        assert!(!second.was_new, "seen before, not announced twice");
        assert_eq!(store.message_count().unwrap(), 1);
        assert_eq!(
            store.placement_uid(first.message_id, inbox).unwrap(),
            Some(Some(21))
        );
        assert_eq!(
            store.placement_uid(first.message_id, sent).unwrap(),
            Some(Some(8))
        );
        assert_eq!(hits(&store, "café"), 1);
    }
}

/// A list that only adds its List-* headers has changed nothing anyone reads.
#[test]
fn a_list_copy_that_only_adds_list_headers_is_the_same_message() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let (posted, _) = trip_notes();
    let relayed = msg(
        "post-1@example.com",
        "Me <me@example.com>",
        "Trip notes",
        "Thu, 1 Oct 2026 08:00:00 +0000",
        "List-Id: Hikers <hikers.lists.example>\r\n\
         List-Unsubscribe: <https://lists.example/u>\r\n\
         List-Post: <mailto:hikers@lists.example>\r\n\
         Precedence: list\r\n",
        "See you at the trailhead at nine.",
    );
    let a = store
        .ingest_raw(&blobs, account, Some(sent), Some(3), &posted)
        .unwrap();
    let b = store
        .ingest_raw(&blobs, account, Some(inbox), Some(40), &relayed)
        .unwrap();
    assert_eq!(a.message_id, b.message_id);
    assert!(!b.was_new);
    assert_eq!(store.message_count().unwrap(), 1);
}

/// A list that names itself the Sender, or points Reply-To at itself, has
/// changed what answering the message does: its copy is a message of its
/// own, as Thunderbird shows it, and Sent keeps the post as written.
#[test]
fn a_list_copy_that_changes_sender_or_reply_to_is_its_own_message() {
    for added in [
        "Sender: hikers-bounces@lists.example\r\n",
        "Reply-To: Hikers <hikers@lists.example>\r\n",
    ] {
        let (_dir, mut store, blobs, account) = setup();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
        let (posted, _) = trip_notes();
        let relayed = msg(
            "post-1@example.com",
            "Me <me@example.com>",
            "Trip notes",
            "Thu, 1 Oct 2026 08:00:00 +0000",
            &format!("List-Id: Hikers <hikers.lists.example>\r\n{added}"),
            "See you at the trailhead at nine.",
        );
        let a = store
            .ingest_raw(&blobs, account, Some(sent), Some(3), &posted)
            .unwrap();
        let b = store
            .ingest_raw(&blobs, account, Some(inbox), Some(40), &relayed)
            .unwrap();
        assert_ne!(a.message_id, b.message_id, "{added}");
        assert_eq!(store.message_count().unwrap(), 2);
    }
}

/// A list that tags the subject and adds a footer has changed what people
/// read: its copy is a message of its own, and Sent shows the post as it was
/// written, whichever arrives first.
#[test]
fn a_tagged_list_copy_is_its_own_message_and_sent_shows_the_post_in_either_order() {
    for list_first in [true, false] {
        let (_dir, mut store, blobs, account) = setup();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
        let (posted, tagged) = trip_notes();
        let (mine, theirs) = if list_first {
            let theirs = store
                .ingest_raw(&blobs, account, Some(inbox), Some(40), &tagged)
                .unwrap();
            let mine = store
                .ingest_raw(&blobs, account, Some(sent), Some(3), &posted)
                .unwrap();
            (mine, theirs)
        } else {
            let mine = store
                .ingest_raw(&blobs, account, Some(sent), Some(3), &posted)
                .unwrap();
            let theirs = store
                .ingest_raw(&blobs, account, Some(inbox), Some(40), &tagged)
                .unwrap();
            (mine, theirs)
        };
        assert_ne!(
            mine.message_id, theirs.message_id,
            "list first: {list_first}"
        );
        let rows = listed(&store, &folder_view("sent"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Trip notes", "list first: {list_first}");
        let inbox_rows = listed(&store, &ListView::Inbox);
        assert!(
            inbox_rows.iter().any(|(s, _)| s == "[hikers] Trip notes"),
            "{inbox_rows:?}"
        );
        // A reply to either names the one Message-ID they both carry.
        for id in [mine.message_id, theirs.message_id] {
            let thread = store.thread_of(id).unwrap().unwrap();
            let detail = store.thread_detail(thread).unwrap();
            let me = detail.iter().find(|m| m.id == id).unwrap();
            assert_eq!(me.msgid.as_deref(), Some("post-1@example.com"));
            assert_eq!(
                store.msgid_header_of(id).unwrap().as_deref(),
                Some("post-1@example.com")
            );
        }
        store.fts_integrity_check().expect("index consistent");
    }
}

// ---- the reconcile, and the same folder ------------------------------------

/// The reconcile's second-copy path and the ordinary fetch key a different
/// message the same way, so whichever meets it first, the other lands on the
/// same row.
#[test]
fn a_different_message_found_by_the_reconcile_lands_where_a_fetch_would() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    let found = store
        .ingest_raw_second_copy(&blobs, account, Some(inbox), 77, &forgery())
        .unwrap();
    assert_ne!(found.message_id, real.message_id);
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    assert_eq!(again.message_id, found.message_id);
    assert!(!again.was_new);
    assert_eq!(store.message_count().unwrap(), 2);
}

/// The same message delivered twice into one folder is two messages on the
/// server, at two UIDs. One row cannot hold both numbers in one folder, so it
/// stays two rows, as the reconcile's second copy keeps it; folding them
/// moved the stored placement back and forth between the two UIDs.
#[test]
fn the_same_message_twice_in_one_folder_is_two_rows() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let once = invoice();
    let mut twice = b"Received: from relay.example.com by mx.example.com\r\n".to_vec();
    twice.extend_from_slice(&once);
    let a = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &once)
        .unwrap();
    let b = store
        .ingest_raw(&blobs, account, Some(inbox), Some(6), &twice)
        .unwrap();
    assert_ne!(a.message_id, b.message_id);
    assert_eq!(
        store.placement_uid(a.message_id, inbox).unwrap(),
        Some(Some(5))
    );
    assert_eq!(
        store.placement_uid(b.message_id, inbox).unwrap(),
        Some(Some(6))
    );
    // And the reconcile, meeting UID 6 again, finds the same row.
    let c = store
        .ingest_raw_second_copy(&blobs, account, Some(inbox), 6, &twice)
        .unwrap();
    assert_eq!(c.message_id, b.message_id);
}

/// A row Petrel itself had thrown away keeps nothing it must protect, but if
/// the arriving message is one that already has a row of its own, it lands
/// there rather than reviving the thrown-away row as a second copy of it.
#[test]
fn a_message_with_a_row_of_its_own_lands_there_when_the_first_row_is_gone() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();
    let forged = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    store.tombstone_message(real.message_id).unwrap();
    // The forgery's copy is fetched again (a UIDVALIDITY reset, say).
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    assert_eq!(again.message_id, forged.message_id);
    assert_eq!(
        hits(&store, "kestrel"),
        0,
        "the deleted invoice stays deleted"
    );
    assert_eq!(hits(&store, "osprey"), 1);
}

/// What makes a stored key a wire Message-ID again: no suffix of Petrel's own,
/// and no stand-in for a message that had none.
#[test]
fn a_reply_names_only_a_real_message_id() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let no_id = b"From: a@example.com\r\nTo: me@example.com\r\nSubject: no id\r\n\r\nbody\r\n";
    let row = store
        .ingest_raw(&blobs, account, Some(inbox), Some(1), no_id)
        .unwrap();
    assert_eq!(store.msgid_header_of(row.message_id).unwrap(), None);
    let real = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    let copy = store
        .ingest_raw_second_copy(&blobs, account, Some(inbox), 6, &{
            let mut b = b"Received: from relay.example.com\r\n".to_vec();
            b.extend_from_slice(&invoice());
            b
        })
        .unwrap();
    assert_ne!(copy.message_id, real.message_id);
    assert_eq!(
        store.msgid_header_of(copy.message_id).unwrap().as_deref(),
        Some("inv-1001@vendor.example")
    );
}

fn trip_notes() -> (Vec<u8>, Vec<u8>) {
    let posted = msg(
        "post-1@example.com",
        "Me <me@example.com>",
        "Trip notes",
        "Thu, 1 Oct 2026 08:00:00 +0000",
        "",
        "See you at the trailhead at nine.",
    );
    let tagged = msg(
        "post-1@example.com",
        "Me <me@example.com>",
        "[hikers] Trip notes",
        "Thu, 1 Oct 2026 08:00:00 +0000",
        "List-Id: Hikers <hikers.lists.example>\r\n",
        "See you at the trailhead at nine.\r\n--\r\nhikers mailing list, unsubscribe at lists.example",
    );
    (posted, tagged)
}
