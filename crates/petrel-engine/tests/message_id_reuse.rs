//! A Message-ID is not a secret. The sender, every co-recipient, anyone a
//! thread was forwarded to and every list archive knows it, and so does anyone
//! you ever wrote to about the message they received. So a second message
//! carrying a stored message's Message-ID is a stranger's claim, not an edit:
//! IMAP messages never change once delivered.
//!
//! These pin what such a message may and may not do to the one already held
//! (docs/25 #78): if it says something else it is a message of its own, and
//! it changes nothing the stored one says. `message_identity.rs` holds the
//! rest: the same message seen twice, either order, and the reviewer's cases.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Sort, Store};

fn message(from: &str, subject: &str, date: &str, extra: &str, body: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Date: {date}\r\n\
         Message-ID: <inv-1001@vendor.example>\r\n\
         {extra}\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         {body}\r\n"
    )
    .into_bytes()
}

fn invoice() -> Vec<u8> {
    message(
        "Vendor Billing <billing@vendor.example>",
        "Invoice 1001",
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "",
        "Please remit to Kestrel Bank, account 11-1111.",
    )
}

/// The same Message-ID, different words: a later date, another bank.
fn forgery() -> Vec<u8> {
    message(
        "Vendor Billing <billing@vendor.example>",
        "Invoice 1001",
        "Wed, 30 Sep 2026 09:00:00 +0000",
        "References: <other-thread@elsewhere.example>\r\n",
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

#[test]
fn a_message_reusing_a_stored_message_id_is_a_message_of_its_own() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let receipts = store
        .ensure_folder(account, "", "Receipts")
        .expect("receipts");

    // Weeks ago: the real invoice, filed in Receipts.
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .expect("invoice");
    let thread = store.thread_of(real.message_id).expect("thread");
    assert!(thread.is_some());
    // Someone who knows its Message-ID sends a message that reuses it.
    let later = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .expect("forgery");

    assert_ne!(later.message_id, real.message_id, "two messages");
    assert!(later.was_new, "new mail, announced and seen by rules");
    assert_eq!(
        store.blob_hash_for(real.message_id).expect("hash"),
        Some(real.blob_hash.clone()),
        "the stored message's own bytes, not the newcomer's"
    );
    assert_eq!(hits(&store, "kestrel"), 1, "what it said is still found");
    let (sender, date_ms) = store
        .message_header(real.message_id)
        .expect("header")
        .expect("row");
    assert_eq!(sender, "Vendor Billing");
    assert_eq!(
        date_ms, 1_788_771_600_000,
        "its own date, not the newcomer's"
    );

    // Receipts lists the invoice as it was received, and INBOX the newcomer.
    let rows = store
        .list_threads(&ListView::UserFolder(receipts), 0, 50, Sort::default())
        .expect("receipts");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].snippet.contains("Kestrel"), "{}", rows[0].snippet);
    assert_eq!(
        store.placement_uid(real.message_id, receipts).expect("p"),
        Some(Some(5))
    );
    assert_eq!(
        store.placement_uid(real.message_id, inbox).expect("p"),
        None
    );
    assert_eq!(
        store.placement_uid(later.message_id, inbox).expect("p"),
        Some(Some(77))
    );

    // Its References do not move the stored message into another
    // conversation, and fetching it again lands on its own row.
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .expect("again");
    assert_eq!(again.message_id, later.message_id);
    assert!(!again.was_new);
    assert_eq!(store.thread_of(real.message_id).expect("thread"), thread);
    assert_eq!(store.message_count().expect("count"), 2);
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn a_list_copy_of_a_sent_message_leaves_the_sent_copy_as_written() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let sent = store.ensure_folder(account, "sent", "Sent").expect("sent");
    let posted = message(
        "Me <me@example.com>",
        "Trip notes",
        "Thu, 1 Oct 2026 08:00:00 +0000",
        "",
        "See you at the trailhead at nine.",
    );
    // The list sends the post back with its tag and its footer.
    let list_copy = message(
        "Me <me@example.com>",
        "[hikers] Trip notes",
        "Thu, 1 Oct 2026 08:00:00 +0000",
        "List-Id: Hikers <hikers.lists.example>\r\n",
        "See you at the trailhead at nine.\r\n--\r\nhikers mailing list, unsubscribe at lists.example",
    );
    let mine = store
        .ingest_raw(&blobs, account, Some(sent), Some(3), &posted)
        .expect("sent copy");
    let back = store
        .ingest_raw(&blobs, account, Some(inbox), Some(40), &list_copy)
        .expect("list copy");

    // What people read differs, so the list's copy is a message of its own.
    assert_ne!(back.message_id, mine.message_id);
    let rows = store
        .list_threads(&ListView::Folder("sent".into()), 0, 50, Sort::default())
        .expect("sent");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject, "Trip notes");
    assert!(
        !rows[0].snippet.contains("unsubscribe"),
        "{}",
        rows[0].snippet
    );
    assert_eq!(
        store.placement_uid(back.message_id, inbox).expect("p"),
        Some(Some(40))
    );
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn a_message_the_server_hands_back_after_a_tombstone_is_live_again() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let archive = store
        .ensure_folder(account, "archive", "Archive")
        .expect("archive");
    let first = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .expect("ingest");
    // A folder pruned from the survey, or a deletion seen elsewhere.
    store
        .tombstone_message(first.message_id)
        .expect("tombstone");
    assert_eq!(hits(&store, "kestrel"), 0);

    // The server hands the same message back from another folder.
    let back = store
        .ingest_raw(&blobs, account, Some(archive), Some(9), &invoice())
        .expect("back");
    assert_eq!(back.message_id, first.message_id);
    assert_eq!(hits(&store, "kestrel"), 1, "live and searchable again");
    let rows = store
        .list_threads(&ListView::Folder("archive".into()), 0, 50, Sort::default())
        .expect("archive");
    assert_eq!(rows.len(), 1);
    store.fts_integrity_check().expect("index consistent");
}

/// A row Petrel itself had thrown away protects nothing: what the server holds
/// now under that Message-ID is the message, so the row takes its words. This
/// is no more than a stranger could do with a fresh Message-ID.
#[test]
fn a_tombstoned_message_takes_what_the_server_now_holds() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let first = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .expect("ingest");
    store
        .tombstone_message(first.message_id)
        .expect("tombstone");

    let later = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .expect("later");
    assert_eq!(later.message_id, first.message_id);
    assert_eq!(hits(&store, "osprey"), 1);
    assert_eq!(hits(&store, "kestrel"), 0);
    store.fts_integrity_check().expect("index consistent");
}

/// Two live copies under one Message-ID in the same folder are two messages
/// on the server, as the reconcile's second-copy path already holds. The
/// ordinary fetch keeps them two as well, rather than moving the stored row's
/// placement onto the newcomer's UID and leaving the stored UID to be fetched
/// back later as a duplicate.
#[test]
fn a_second_live_copy_in_the_same_folder_is_kept_as_its_own_row() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let real = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .expect("invoice");
    let other = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .expect("forgery");

    assert_ne!(other.message_id, real.message_id, "two messages");
    assert!(other.was_new, "a message of its own arrived");
    assert_eq!(
        store.placement_uid(real.message_id, inbox).expect("p"),
        Some(Some(5))
    );
    assert_eq!(
        store.placement_uid(other.message_id, inbox).expect("p"),
        Some(Some(77))
    );
    assert_eq!(hits(&store, "kestrel"), 1);
    assert_eq!(hits(&store, "osprey"), 1);

    // Fetched again, each copy lands on its own row.
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .expect("refetch");
    assert_eq!(again.message_id, other.message_id);
    assert!(!again.was_new);
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .expect("refetch");
    assert_eq!(again.message_id, real.message_id);
    assert_eq!(store.message_count().expect("count"), 2);
    store.fts_integrity_check().expect("index consistent");
}

/// The common case stays as it was: the same bytes again, from a resync or a
/// move on another device, are one message with one more placement.
#[test]
fn the_same_bytes_in_another_folder_are_one_message() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let archive = store
        .ensure_folder(account, "archive", "Archive")
        .expect("archive");
    let a = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .expect("inbox copy");
    let b = store
        .ingest_raw(&blobs, account, Some(archive), Some(12), &invoice())
        .expect("archive copy");
    assert_eq!(a.message_id, b.message_id);
    assert_eq!(store.message_count().expect("count"), 1);
    assert_eq!(
        store.placement_uid(a.message_id, archive).expect("p"),
        Some(Some(12))
    );
    assert_eq!(hits(&store, "kestrel"), 1);
}
