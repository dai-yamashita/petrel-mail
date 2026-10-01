//! Writes that touch a message and its index entry are one unit: all of them
//! land, or none do (docs/25 #92). Each test stands a trigger in for a
//! failure partway through — a full disk, an I/O error — and checks that the
//! store reads exactly as it did before the call.
//!
//! AGENTS.md calls index drift the one unforgivable bug class, and asks for
//! messages and `fts_content` to be written in one transaction.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn setup() -> (
    tempfile::TempDir,
    Store,
    BlobStore,
    i64,
    rusqlite::Connection,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("petrel.db");
    let store = Store::open(&db).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    store.set_active_account(account).expect("active");
    let side = rusqlite::Connection::open(&db).expect("side connection");
    (dir, store, blobs, account, side)
}

/// Everything a reader of the row sees: the row, its index entry, its
/// recipients, and where it is filed.
fn snapshot(side: &rusqlite::Connection, id: i64) -> (String, Option<String>, i64, i64, bool) {
    let subject: String = side
        .query_row(
            "SELECT coalesce(subject, '') FROM messages WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("row");
    let indexed: Option<String> = side
        .query_row(
            "SELECT subject FROM fts_content WHERE message_id = ?1",
            [id],
            |r| r.get(0),
        )
        .ok();
    let recipients: i64 = side
        .query_row(
            "SELECT count(*) FROM message_addresses WHERE message_id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("recipients");
    let placements: i64 = side
        .query_row(
            "SELECT count(*) FROM placements WHERE message_id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("placements");
    let live: bool = side
        .query_row(
            "SELECT deleted_at_ms IS NULL FROM messages WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("live");
    (subject, indexed, recipients, placements, live)
}

fn fail_on(side: &rusqlite::Connection, when: &str) {
    side.execute_batch(&format!(
        "CREATE TRIGGER zz_fail {when} BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
    ))
    .expect("trigger");
}

fn stop_failing(side: &rusqlite::Connection) {
    side.execute_batch("DROP TRIGGER zz_fail")
        .expect("drop trigger");
}

#[test]
fn a_draft_save_that_fails_partway_leaves_the_draft_as_it_was() {
    let (_dir, store, _blobs, account, side) = setup();
    let id = store
        .save_draft(
            account,
            None,
            "sam@example.com, dana@example.com",
            "Before",
            "old words",
            "",
        )
        .expect("first save");
    let before = snapshot(&side, id);

    // The third write — the first recipient — refuses.
    fail_on(&side, "BEFORE INSERT ON message_addresses");
    let saved = store.save_draft(
        account,
        Some(id),
        "alex@example.com",
        "After",
        "new words",
        "",
    );
    assert!(saved.is_err(), "the failure is reported");
    assert_eq!(
        snapshot(&side, id),
        before,
        "nothing of the failed save landed"
    );
    assert_eq!(store.search("old", 10).expect("search").len(), 1);
    assert!(store.search("new", 10).expect("search").is_empty());

    // And the next save, once the disk has room again, is whole.
    stop_failing(&side);
    store
        .save_draft(
            account,
            Some(id),
            "alex@example.com",
            "After",
            "new words",
            "",
        )
        .expect("save");
    assert_eq!(snapshot(&side, id).0, "After");
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn a_tombstone_that_fails_partway_leaves_the_message_as_it_was() {
    let (_dir, mut store, blobs, account, side) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let raw = b"From: Dana <dana@example.com>\r\nTo: me@example.com\r\nSubject: Quarterly\r\n\
Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <q1@example.com>\r\n\r\nnumbers\r\n";
    let id = store
        .ingest_raw(&blobs, account, Some(inbox), Some(4), raw)
        .expect("ingest")
        .message_id;
    let before = snapshot(&side, id);

    // The index entry goes, then the placements refuse to.
    fail_on(&side, "BEFORE DELETE ON placements");
    assert!(store.tombstone_message(id).is_err());
    assert_eq!(snapshot(&side, id), before);
    assert_eq!(store.search("numbers", 10).expect("search").len(), 1);
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn a_delete_forever_that_fails_partway_changes_nothing() {
    let (_dir, mut store, blobs, account, side) = setup();
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let raw = b"From: Dana <dana@example.com>\r\nTo: me@example.com\r\nSubject: Quarterly\r\n\
Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <q2@example.com>\r\n\r\nnumbers\r\n";
    let id = store
        .ingest_raw(&blobs, account, Some(inbox), Some(4), raw)
        .expect("ingest")
        .message_id;
    let before = snapshot(&side, id);
    let queued = |side: &rusqlite::Connection| -> i64 {
        side.query_row("SELECT count(*) FROM actions", [], |r| r.get(0))
            .expect("actions")
    };
    let queued_before = queued(&side);

    // The row is tombstoned, then its index entry refuses to go.
    fail_on(&side, "BEFORE DELETE ON fts_content");
    let done = store.apply_message_action(
        account,
        id,
        ActionKind::DeleteForever,
        None,
        PlacementPolicy::Exclusive,
    );
    assert!(done.is_err());
    assert_eq!(snapshot(&side, id), before, "still live, filed and found");
    assert_eq!(
        queued(&side),
        queued_before,
        "and nothing queued for the server"
    );
    stop_failing(&side);
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn a_draft_delete_that_fails_partway_leaves_the_draft() {
    let (_dir, store, _blobs, account, side) = setup();
    let id = store
        .save_draft(
            account,
            None,
            "sam@example.com",
            "Keep me",
            "draft words",
            "",
        )
        .expect("save");
    let before = snapshot(&side, id);

    // The index entry goes first, then the row refuses to.
    fail_on(&side, "BEFORE DELETE ON messages");
    assert!(store.delete_draft(id).is_err());
    assert_eq!(snapshot(&side, id), before);
    assert_eq!(store.search("draft", 10).expect("search").len(), 1);
}

#[test]
fn emptying_a_demo_mailbox_that_fails_partway_empties_nothing() {
    let (_dir, store, _blobs, account, side) = setup();
    let id = store
        .save_draft(account, None, "sam@example.com", "Demo", "demo words", "")
        .expect("save");
    let before = snapshot(&side, id);

    // The messages go first, and `messages_ad` takes their index entries with
    // them; the sweep of what is left in the index then refuses. An entry no
    // message owns is what that sweep is for, so it is the one that fails.
    // The index's triggers call the store's own functions; stand-ins are
    // enough to write one entry from this side.
    use rusqlite::functions::FunctionFlags;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    side.create_scalar_function("petrel_cjk", 1, flags, |ctx| ctx.get::<String>(0))
        .expect("petrel_cjk");
    side.create_scalar_function("petrel_has_cjk", 1, flags, |_| Ok(0))
        .expect("petrel_has_cjk");
    side.execute_batch(
        "INSERT INTO fts_content(message_id, subject, body_text) VALUES (999999, 'stray', '');",
    )
    .expect("stray entry");
    fail_on(
        &side,
        "BEFORE DELETE ON fts_content WHEN old.message_id = 999999",
    );
    assert!(store.delete_all_messages().is_err());
    assert_eq!(snapshot(&side, id), before);
}
