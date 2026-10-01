//! A store a newer Petrel wrote must not be opened by an older one (docs/25
//! #91). The older code would run against tables it does not understand, and
//! the file keeps the newer version number, so nothing would ever migrate
//! what the older code wrote in its own shape.

use petrel_engine::store::{SCHEMA_VERSION, Store, StoreError};

#[test]
fn a_store_from_a_newer_petrel_is_refused_and_left_as_it_was() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("petrel.db");
    {
        let store = Store::open(&db).expect("store");
        store.ensure_test_account().expect("account");
    }
    {
        // What a later release leaves: a higher version, and a column this
        // build has never heard of.
        let conn = rusqlite::Connection::open(&db).expect("conn");
        conn.execute_batch("ALTER TABLE messages ADD COLUMN from_the_future TEXT")
            .expect("alter");
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 5)
            .expect("version");
        conn.pragma_update(None, "wal_checkpoint", "TRUNCATE").ok();
    }
    let before = std::fs::read(&db).expect("read");

    let err = match Store::open(&db) {
        Ok(_) => panic!("a store from a newer Petrel was opened"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("newer version of Petrel"), "{err}");
    assert!(
        matches!(err, StoreError::NewerSchema { found, supported }
            if found == SCHEMA_VERSION + 5 && supported == SCHEMA_VERSION),
        "{err:?}"
    );

    assert_eq!(
        std::fs::read(&db).expect("read"),
        before,
        "the file is untouched"
    );
    let conn = rusqlite::Connection::open(&db).expect("conn");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("version");
    assert_eq!(version, SCHEMA_VERSION + 5);
}

#[test]
fn a_store_at_this_version_still_opens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("petrel.db");
    drop(Store::open(&db).expect("first open"));
    Store::open(&db).expect("second open");
}
