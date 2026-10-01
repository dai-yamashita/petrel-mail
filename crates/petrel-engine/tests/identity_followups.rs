//! The second review's follow-ups to content identity (docs/25 #78): a
//! content row's second delivery into one folder, a Bcc-carrying copy in
//! Sent over a Sent copy already held, and a stored message whose bytes can
//! no longer be read.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Sort, Store};

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

fn invoice() -> Vec<u8> {
    msg(
        "inv-3003@vendor.example",
        "Vendor Billing <billing@vendor.example>",
        "Invoice 3003",
        "Mon, 7 Sep 2026 09:00:00 +0000",
        "",
        "Please remit to Kestrel Bank, account 11-1111.",
    )
}

fn forgery() -> Vec<u8> {
    msg(
        "inv-3003@vendor.example",
        "Vendor Billing <billing@vendor.example>",
        "Invoice 3003",
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

/// Damages the file a blob is stored in, as a disk error or a bad restore does.
fn damage(dir: &tempfile::TempDir, hash: &str) {
    let path = dir
        .path()
        .join("blobs")
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(format!("{hash}.zst"));
    std::fs::write(&path, b"not zstd").expect("damage");
}

// ---- finding 4: a content row's second delivery into one folder -----------

/// Review r2_4. The new scan is a row under its content key, in INBOX at 50.
/// The same scan delivered again (another address, so other trace headers)
/// lands in INBOX at 51: two messages on the server. Folding them moved the
/// row's number to 51, and the reconcile, finding 50 unplaced, moved it back:
/// archiving then moved one copy and the other brought the row back. It is
/// kept apart, as the plain row's second delivery already is.
#[test]
fn a_content_rows_second_delivery_into_one_folder_is_its_own_row() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
    store
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
    let new_scan = scan(
        "Wed, 30 Sep 2026 09:00:00 +0000",
        "Scan of the passport renewal form.",
    );
    let b = store
        .ingest_raw(&blobs, account, Some(inbox), Some(50), &new_scan)
        .unwrap();
    let mut twice = b"Delivered-To: me+alias@example.com\r\n".to_vec();
    twice.extend_from_slice(&new_scan);
    let b2 = store
        .ingest_raw(&blobs, account, Some(inbox), Some(51), &twice)
        .unwrap();
    assert_ne!(
        b2.message_id, b.message_id,
        "two messages on the server, two rows"
    );
    assert_eq!(
        store.placement_uid(b.message_id, inbox).unwrap(),
        Some(Some(50))
    );
    assert_eq!(
        store.placement_uid(b2.message_id, inbox).unwrap(),
        Some(Some(51))
    );

    // Fetched again, as the reconcile would: each stays where it is.
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(50), &new_scan)
        .unwrap();
    assert_eq!(again.message_id, b.message_id);
    let again = store
        .ingest_raw(&blobs, account, Some(inbox), Some(51), &twice)
        .unwrap();
    assert_eq!(again.message_id, b2.message_id);
    assert_eq!(
        store.placement_uid(b.message_id, inbox).unwrap(),
        Some(Some(50))
    );
    assert_eq!(
        store.placement_uid(b2.message_id, inbox).unwrap(),
        Some(Some(51))
    );
    // Replies to either name the one Message-ID they carry.
    assert_eq!(
        store.msgid_header_of(b2.message_id).unwrap().as_deref(),
        Some("scan@printer.local")
    );
    store.fts_integrity_check().expect("index consistent");
}

// ---- finding 5: a Bcc-carrying copy in Sent ---------------------------------

/// Review r2_3b's class. The Sent copy of your own message is the one that
/// keeps its Bcc line, so the first Sent copy to arrive with one is adopted
/// (`your_own_copy_keeps_its_bcc_line_in_either_order`). Not over a Sent copy
/// the row already has: a filter rule or plus-address delivery can put a copy
/// in Sent too, and one that adds only a Bcc line is the same message by
/// every field the identity reads.
#[test]
fn a_bcc_copy_in_sent_is_not_adopted_over_a_sent_copy_already_held() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let delivered = b"Received: from submit.example.com by mx.example.com\r\n\
From: Me <me@example.com>\r\nTo: me@example.com\r\n\
Subject: Wire details\r\nDate: Mon, 7 Sep 2026 09:00:00 +0000\r\n\
Message-ID: <wire@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nAccount 11-1111 for the deposit.\r\n";
    let planted = b"Received: from relay.elsewhere.example by mx.example.com\r\n\
From: Me <me@example.com>\r\nTo: me@example.com\r\n\
Subject: Wire details\r\nDate: Mon, 7 Sep 2026 09:00:00 +0000\r\n\
Message-ID: <wire@example.com>\r\nBcc: someone@elsewhere.example\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nAccount 11-1111 for the deposit.\r\n";
    let row = store
        .ingest_raw(&blobs, account, Some(inbox), Some(30), delivered)
        .unwrap();
    // The row's own Sent copy, its number not known yet (a renumbered Sent,
    // or a move filed before the server answered).
    store.place_message(row.message_id, sent).unwrap();
    let before = store.blob_hash_for(row.message_id).unwrap();

    let landed = store
        .ingest_raw(&blobs, account, Some(sent), Some(12), planted)
        .unwrap();
    assert_eq!(landed.message_id, row.message_id, "the same message");
    assert_eq!(
        store.blob_hash_for(row.message_id).unwrap(),
        before,
        "the row keeps the copy it had"
    );
    let thread = store.thread_of(row.message_id).unwrap().unwrap();
    let detail = store.thread_detail(thread).unwrap();
    assert!(detail[0].bcc.is_empty(), "{:?}", detail[0].bcc);
    assert_eq!(hits(&store, "elsewhere"), 0);
}

// ---- finding 6: a stored message whose bytes cannot be read ---------------

/// Review r2_5. The invoice's stored bytes are damaged, and a forgery arrives
/// under its Message-ID. Nothing can be compared, so nothing is assumed: the
/// forgery is a message of its own, and the invoice keeps its words.
#[test]
fn an_unreadable_stored_message_is_not_replaced_by_other_bytes() {
    let (dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();
    damage(&dir, &real.blob_hash);
    assert!(blobs.read(&real.blob_hash).is_err());

    let f = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    assert_ne!(f.message_id, real.message_id);
    assert!(f.was_new);
    assert_eq!(hits(&store, "kestrel"), 1);
    assert_eq!(hits(&store, "osprey"), 1);
    let rows = store
        .list_threads(&ListView::UserFolder(receipts), 0, 50, Sort::default())
        .unwrap();
    assert!(rows[0].snippet.contains("Kestrel"), "{:?}", rows[0].snippet);
    assert_eq!(
        store.blob_hash_for(real.message_id).unwrap(),
        Some(real.blob_hash.clone())
    );
}

/// The same bytes fetched again are the same message, unreadable or not, and
/// they mend the damaged file.
#[test]
fn the_same_bytes_land_on_an_unreadable_message_and_mend_it() {
    let (dir, mut store, blobs, account) = setup();
    let receipts = store.ensure_folder(account, "", "Receipts").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();
    damage(&dir, &real.blob_hash);
    let again = store
        .ingest_raw(&blobs, account, Some(receipts), Some(5), &invoice())
        .unwrap();
    assert_eq!(again.message_id, real.message_id);
    assert!(!again.was_new);
    assert_eq!(
        blobs.read(&real.blob_hash).expect("readable again"),
        invoice()
    );
}

// ---- finding 3's engine half ------------------------------------------------

/// What the drain asks before trusting a server copy whose size is not the
/// stored one: whether another row carries the Message-ID, and whether the
/// copy's bytes are this message by every field the identity reads.
#[test]
fn a_copy_is_known_as_this_message_by_its_content_not_its_size() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let real = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &invoice())
        .unwrap();
    assert!(store.carries_its_message_id_alone(real.message_id).unwrap());
    let mut delivered = b"Received: from mx.example.com by imap.example.com\r\n".to_vec();
    delivered.extend_from_slice(&invoice());
    assert!(
        store
            .is_same_message(&blobs, real.message_id, &delivered)
            .unwrap()
    );
    assert!(
        !store
            .is_same_message(&blobs, real.message_id, &forgery())
            .unwrap()
    );

    let f = store
        .ingest_raw(&blobs, account, Some(inbox), Some(77), &forgery())
        .unwrap();
    assert!(!store.carries_its_message_id_alone(real.message_id).unwrap());
    assert!(!store.carries_its_message_id_alone(f.message_id).unwrap());
}
