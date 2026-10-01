//! Matching a server copy to a row by Message-ID, now that one Message-ID can
//! belong to more than one row (docs/25 #78, review round two).
//!
//! Several paths know a server message only by its UID and its Message-ID: a
//! folder renumbered by a UIDVALIDITY reset, the Gmail label sweep, the
//! numbering of the inbox placements that sweep makes, and the All Mail walk.
//! Where one live row carries the id, they match as they always have. Where
//! more than one does, or the server lists the id more than once, the listing
//! cannot say which row a copy is: the copy is left unmatched, fetched again,
//! and placed by its content, never handed to the wrong row.
//!
//! And a copy whose bytes a row already holds is that row, whatever its key
//! says, so a parser change that moves a content key does not turn the next
//! fetch of the same message into new mail.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{DraftEnvelope, ListView, Sort, Store};

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

fn scan(date: &str, body: &str) -> Vec<u8> {
    msg(
        "scan@printer.local",
        "Office Scanner <scanner@example.com>",
        "Scanned document",
        date,
        "",
        body,
    )
}

fn old_scan() -> Vec<u8> {
    scan(
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "Scan of the lease agreement.",
    )
}

fn new_scan() -> Vec<u8> {
    scan(
        "Wed, 30 Sep 2026 09:00:00 +0000",
        "Scan of the passport renewal form.",
    )
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

fn side(dir: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("petrel.db")).expect("side connection")
}

fn live(c: &rusqlite::Connection, id: i64) -> bool {
    c.query_row(
        "SELECT deleted_at_ms IS NULL FROM messages WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .expect("row")
}

fn key_of(c: &rusqlite::Connection, id: i64) -> String {
    c.query_row(
        "SELECT message_id_hdr FROM messages WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .expect("row")
}

// ---- a UIDVALIDITY reset ----------------------------------------------------

/// Review r2_1a. Two scans under one Message-ID, both in INBOX, and then the
/// server renumbers INBOX. The listing cannot say which number is which scan,
/// so neither is handed to the wrong row: both are fetched again and each
/// lands on its own row by its bytes. Nothing is lost, and a Delete forever
/// of the old scan reaches the old scan's server copy.
#[test]
fn a_reset_does_not_give_one_message_anothers_number() {
    let (dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let old = store
        .ingest_raw(&blobs, account, Some(inbox), Some(3), &old_scan())
        .unwrap();
    let new = store
        .ingest_raw(&blobs, account, Some(inbox), Some(50), &new_scan())
        .unwrap();
    assert_ne!(old.message_id, new.message_id);

    let mid = Some("scan@printer.local".to_string());
    let out = store
        .remap_folder_after_reset(inbox, &[(103, mid.clone()), (150, mid)], true)
        .unwrap();
    let mut to_fetch = out.to_fetch.clone();
    to_fetch.sort_unstable();
    assert_eq!(to_fetch, vec![103, 150], "neither number is guessed");
    assert_ne!(
        store.placement_uid(old.message_id, inbox).unwrap(),
        Some(Some(150)),
        "the old scan never holds the new scan's number"
    );

    // What `recover_folder` does with `to_fetch`.
    let a = store
        .ingest_raw(&blobs, account, Some(inbox), Some(103), &old_scan())
        .unwrap();
    let b = store
        .ingest_raw(&blobs, account, Some(inbox), Some(150), &new_scan())
        .unwrap();
    assert_eq!(a.message_id, old.message_id, "the same row, by its bytes");
    assert_eq!(b.message_id, new.message_id, "the same row, by its bytes");
    assert!(!a.was_new && !b.was_new, "nothing announced as new");
    let c = side(&dir);
    assert!(live(&c, old.message_id) && live(&c, new.message_id));
    assert_eq!(
        store.placement_uid(old.message_id, inbox).unwrap(),
        Some(Some(103))
    );
    assert_eq!(
        store.placement_uid(new.message_id, inbox).unwrap(),
        Some(Some(150))
    );
    assert_eq!(store.message_count().unwrap(), 2);
    assert_eq!(hits(&store, "passport"), 1);
    assert_eq!(hits(&store, "lease"), 1);

    store
        .apply_message_action(
            account,
            old.message_id,
            ActionKind::DeleteForever,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let pending = store.pending_actions(account).unwrap();
    let queued = pending
        .iter()
        .find(|p| p.message_id == old.message_id)
        .expect("queued");
    assert_eq!(queued.uid, Some(103), "the old scan's own copy");
}

/// Review r2_1b. The forgery holds the plain key (it was fetched first) and
/// the real invoice its content key, both in INBOX. A reset keeps both.
#[test]
fn a_reset_keeps_the_real_invoice_when_the_forgery_holds_the_plain_key() {
    let (dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let forged = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    let mid = Some("inv-1001@vendor.example".to_string());
    let out = store
        .remap_folder_after_reset(inbox, &[(5, mid.clone()), (77, mid)], true)
        .unwrap();
    let mut to_fetch = out.to_fetch.clone();
    to_fetch.sort_unstable();
    assert_eq!(to_fetch, vec![5, 77]);
    let r = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    let f = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    assert_eq!(r.message_id, real.message_id);
    assert_eq!(f.message_id, forged.message_id);
    let c = side(&dir);
    assert!(live(&c, real.message_id), "the real invoice is not lost");
    assert_eq!(hits(&store, "kestrel"), 1);
    assert_eq!(hits(&store, "osprey"), 1);
}

/// The ordinary case is as it always was: one row carries the id, the
/// listing names it once, and the row learns its new number without a fetch.
#[test]
fn a_reset_still_matches_a_message_one_row_carries() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let only = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    let out = store
        .remap_folder_after_reset(
            inbox,
            &[(500, Some("inv-1001@vendor.example".to_string()))],
            true,
        )
        .unwrap();
    assert_eq!(out.rematched, 1);
    assert!(out.to_fetch.is_empty());
    assert_eq!(
        store.placement_uid(only.message_id, inbox).unwrap(),
        Some(Some(500))
    );
}

/// One row carries the id, but the server lists it twice: a second message
/// the store has not seen yet. Neither number is guessed; both are fetched,
/// and the unseen one becomes a message of its own.
#[test]
fn a_reset_fetches_a_second_server_copy_the_store_has_not_seen() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let old = store
        .ingest_raw(&blobs, account, Some(inbox), Some(3), &old_scan())
        .unwrap();
    let mid = Some("scan@printer.local".to_string());
    let out = store
        .remap_folder_after_reset(inbox, &[(103, mid.clone()), (150, mid)], true)
        .unwrap();
    let mut to_fetch = out.to_fetch.clone();
    to_fetch.sort_unstable();
    assert_eq!(to_fetch, vec![103, 150]);
    let a = store
        .ingest_raw(&blobs, account, Some(inbox), Some(103), &old_scan())
        .unwrap();
    let b = store
        .ingest_raw(&blobs, account, Some(inbox), Some(150), &new_scan())
        .unwrap();
    assert_eq!(a.message_id, old.message_id);
    assert_ne!(b.message_id, old.message_id);
    assert!(b.was_new);
    assert_eq!(
        store.placement_uid(old.message_id, inbox).unwrap(),
        Some(Some(103))
    );
}

// ---- numbering the inbox placements the label sweep makes -----------------

/// The label sweep files by Message-ID and learns no INBOX number; the
/// numbering pass then matches the inbox listing to those placements. Two
/// rows carry this id, so the one copy listed cannot be given to either: the
/// placement stays as it is, unnumbered and not dropped, until a fetch of
/// the copy places it by content.
#[test]
fn an_inbox_placement_two_rows_could_be_is_left_unnumbered() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    let old = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &old_scan())
        .unwrap();
    let new = store
        .ingest_raw(&blobs, account, Some(archive), Some(4), &new_scan())
        .unwrap();
    assert_ne!(old.message_id, new.message_id);
    // The sweep filed the bare row into the inbox by Message-ID.
    store.place_message(old.message_id, inbox).unwrap();
    let out = store
        .reconcile_unaddressed_placements(inbox, &[(9, Some("scan@printer.local".into()))])
        .unwrap();
    assert_eq!(out.rematched, 0);
    assert_eq!(out.dropped, 0);
    assert_eq!(
        store.placement_uid(old.message_id, inbox).unwrap(),
        Some(None),
        "not numbered with a copy that may be the other scan, and not dropped"
    );
}

/// Every copy the listing holds under the id is already numbered on another
/// row, so this placement has no copy in the folder: it goes.
#[test]
fn an_inbox_placement_whose_copies_are_all_another_rows_is_dropped() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    let old = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &old_scan())
        .unwrap();
    let new = store
        .ingest_raw(&blobs, account, Some(inbox), Some(9), &new_scan())
        .unwrap();
    store.place_message(old.message_id, inbox).unwrap();
    let out = store
        .reconcile_unaddressed_placements(inbox, &[(9, Some("scan@printer.local".into()))])
        .unwrap();
    assert_eq!(out.dropped, 1);
    assert_eq!(store.placement_uid(old.message_id, inbox).unwrap(), None);
    assert_eq!(
        store.placement_uid(new.message_id, inbox).unwrap(),
        Some(Some(9))
    );
}

/// A row under a content key was dropped by every numbering pass, because
/// its key never equals a wire id. It may well be the copy listed: left
/// unnumbered, not dropped.
#[test]
fn an_inbox_placement_under_a_content_key_is_not_dropped_for_its_key() {
    let (dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    let old = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &old_scan())
        .unwrap();
    let new = store
        .ingest_raw(&blobs, account, Some(archive), Some(4), &new_scan())
        .unwrap();
    // The old scan is deleted and purged; the new scan is the id's one row.
    store.tombstone_message(old.message_id).unwrap();
    let c = side(&dir);
    c.execute("DELETE FROM messages WHERE id = ?1", [old.message_id])
        .unwrap();
    store.place_message(new.message_id, inbox).unwrap();
    let out = store
        .reconcile_unaddressed_placements(inbox, &[(9, Some("scan@printer.local".into()))])
        .unwrap();
    assert_eq!(out.dropped, 0, "{}", key_of(&c, new.message_id));
    assert!(live(&c, new.message_id));
    assert_eq!(
        store.placement_uid(new.message_id, inbox).unwrap(),
        Some(None)
    );
}

/// The ordinary case: one row, one listed copy, numbered as before.
#[test]
fn an_inbox_placement_one_row_carries_is_numbered_as_before() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    let only = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &invoice())
        .unwrap();
    store.place_message(only.message_id, inbox).unwrap();
    let out = store
        .reconcile_unaddressed_placements(inbox, &[(9, Some("inv-1001@vendor.example".into()))])
        .unwrap();
    assert_eq!(out.rematched, 1);
    assert_eq!(
        store.placement_uid(only.message_id, inbox).unwrap(),
        Some(Some(9))
    );
}

// ---- the Gmail label sweep, and the All Mail walk --------------------------

/// The label sweep knows a message by its Message-ID alone. Filing the plain
/// row by labels that may be the other message's moves the wrong one, so an
/// id two rows carry is left for the fetch to settle.
#[test]
fn the_label_sweep_leaves_an_id_two_rows_carry_where_they_are() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "archive").unwrap();
    let old = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &old_scan())
        .unwrap();
    let new = store
        .ingest_raw(&blobs, account, Some(archive), Some(4), &new_scan())
        .unwrap();
    let filed = store
        .apply_gmail_labels(
            account,
            &[(
                "scan@printer.local".to_string(),
                vec!["\\Inbox".to_string()],
            )],
        )
        .unwrap();
    assert_eq!(filed, 0);
    assert_eq!(store.placement_uid(old.message_id, inbox).unwrap(), None);
    assert_eq!(store.placement_uid(new.message_id, inbox).unwrap(), None);
}

/// One row carries the id: filed by its labels, as before. An id the same
/// sweep reports twice is two messages on the server, and is left alone.
#[test]
fn the_label_sweep_files_an_id_one_row_carries_and_skips_one_reported_twice() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "archive").unwrap();
    let only = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &invoice())
        .unwrap();
    let inbox_label = vec!["\\Inbox".to_string()];
    let filed = store
        .apply_gmail_labels(
            account,
            &[("inv-1001@vendor.example".to_string(), inbox_label.clone())],
        )
        .unwrap();
    assert_eq!(filed, 1);
    assert!(
        store
            .placement_uid(only.message_id, inbox)
            .unwrap()
            .is_some()
    );

    let other = store
        .ingest_raw(&blobs, account, Some(archive), Some(5), &old_scan())
        .unwrap();
    let filed = store
        .apply_gmail_labels(
            account,
            &[
                ("scan@printer.local".to_string(), inbox_label.clone()),
                ("scan@printer.local".to_string(), vec![]),
            ],
        )
        .unwrap();
    assert_eq!(filed, 0);
    assert_eq!(store.placement_uid(other.message_id, inbox).unwrap(), None);
}

/// The All Mail walk claims a listed UID for the row that carries its
/// Message-ID, without fetching it. It may claim only when that cannot be
/// wrong; anything else is fetched and placed by its content.
#[test]
fn the_all_mail_walk_claims_only_what_one_row_carries() {
    let (_dir, mut store, blobs, account) = setup();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    let invoice_row = store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &invoice())
        .unwrap();
    store
        .ingest_raw(&blobs, account, Some(archive), Some(4), &old_scan())
        .unwrap();
    store
        .ingest_raw(&blobs, account, Some(archive), Some(5), &new_scan())
        .unwrap();
    let listed = vec![
        (40, Some("inv-1001@vendor.example".to_string())),
        (41, Some("scan@printer.local".to_string())),
        (42, Some("scan@printer.local".to_string())),
        (43, Some("nobody-has-this@example.com".to_string())),
        (44, None),
    ];
    let claims = store.claims_by_message_id(account, &listed).unwrap();
    assert_eq!(
        claims,
        vec![
            (40, Some(invoice_row.message_id)),
            (41, None),
            (42, None),
            (43, None),
            (44, None),
        ]
    );
}

// ---- the same bytes are the same row --------------------------------------

/// Review r2_7. A content key is a digest of what the parser read, so a
/// parser change moves it. The bytes do not move: a fetch of bytes a row
/// already holds lands on that row, whatever its key, and is not new mail.
#[test]
fn a_content_row_is_found_by_its_bytes_after_its_key_moved() {
    let (dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    store
        .ingest_raw(&blobs, account, Some(archive), Some(3), &old_scan())
        .unwrap();
    let b = store
        .ingest_raw(&blobs, account, Some(inbox), Some(50), &new_scan())
        .unwrap();
    let c = side(&dir);
    let key = key_of(&c, b.message_id);
    let (bare, _) = key.split_once("::b-").unwrap();
    // What a parser change does to the key the next fetch would look for.
    c.execute(
        "UPDATE messages SET message_id_hdr = ?2 WHERE id = ?1",
        rusqlite::params![b.message_id, format!("{bare}::b-0000000000000000")],
    )
    .unwrap();
    let again = store
        .ingest_raw(&blobs, account, Some(archive), Some(9), &new_scan())
        .unwrap();
    assert_eq!(again.message_id, b.message_id, "the same row");
    assert!(!again.was_new);
    assert_eq!(hits(&store, "passport"), 1);
    assert_eq!(
        store.placement_uid(b.message_id, archive).unwrap(),
        Some(Some(9))
    );
}

// ---- Reply-To and Sender are part of what a message is ---------------------

/// Review r2_2. A copy that differs only by a Reply-To decides where a reply
/// goes, so it is another message. Stored first, it stays apart from the
/// real one, which keeps its own reply target.
#[test]
fn a_copy_with_a_strangers_reply_to_is_its_own_message() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();
    let real = msg(
        "inv-2002@vendor.example",
        "Vendor Billing <billing@vendor.example>",
        "Invoice 2002",
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "Received: from mail.vendor.example by mx.example.com\r\n",
        "Please confirm the remittance by replying to this message.",
    );
    let resent = msg(
        "inv-2002@vendor.example",
        "Vendor Billing <billing@vendor.example>",
        "Invoice 2002",
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "Received: from relay.elsewhere.example by mx.example.com\r\n\
         Reply-To: Vendor Billing <billing@vendor-payments.example>\r\n",
        "Please confirm the remittance by replying to this message.",
    );
    let first = store
        .ingest_raw(&blobs, account, Some(inbox), Some(90), &resent)
        .unwrap();
    let second = store
        .ingest_raw(&blobs, account, Some(receipts), Some(4), &real)
        .unwrap();
    assert_ne!(first.message_id, second.message_id);
    assert!(second.was_new);
    let hash = store.blob_hash_for(second.message_id).unwrap().unwrap();
    let raw = blobs.read(&hash).unwrap();
    let reply_to = petrel_mime::parse_message(&raw).unwrap().reply_to;
    assert!(
        reply_to.is_empty(),
        "replies go to the real sender: {reply_to:?}"
    );
}

// ---- taking a server revision of a draft that has gone ---------------------

/// Review r2_6. The draft was sent or discarded between the command's check
/// and its write. Nothing is written for a row that is not there: no index
/// entry for search to find.
#[test]
fn taking_the_servers_revision_of_a_draft_that_has_gone_writes_nothing() {
    let (dir, store, _blobs, account) = setup();
    let id = store
        .save_draft_full(
            account,
            None,
            "dana@example.com",
            "",
            "Plan v1",
            "first words",
            "",
            &DraftEnvelope::default(),
        )
        .unwrap();
    store.delete_draft(id).unwrap();
    let done = store.adopt_server_revision(id, "Plan v2", "orphaned osprey words", "", Some(9));
    assert!(done.is_err(), "refused: {done:?}");
    let c = side(&dir);
    let entries: i64 = c
        .query_row(
            "SELECT count(*) FROM fts_content WHERE message_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(entries, 0);
    assert!(store.search("osprey", 10).unwrap().is_empty());
    store.fts_integrity_check().expect("index consistent");
}

/// The sanity check that the listing helpers did not change what the views
/// show for an ordinary mailbox.
#[test]
fn an_ordinary_mailbox_reads_as_before() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    let rows = store
        .list_threads(&ListView::Inbox, 0, 50, Sort::default())
        .unwrap();
    assert_eq!(rows.len(), 1);
}
