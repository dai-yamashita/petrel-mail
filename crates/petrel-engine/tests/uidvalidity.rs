//! UIDVALIDITY reset: the server renumbered a folder, every stored UID is a
//! lie, and the mend must cost at worst a re-download — never data.
//!
//! The properties pinned here: quarantine holds queued actions off the wire
//! (a NULL UID is already "not addressable"), the re-map learns new UIDs by
//! the same key ingest dedupes on, an incomplete listing never evicts
//! history, and no message row or blob is ever deleted.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn fixture(message_id: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{message_id}>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         body of {subject}\r\n"
    )
    .into_bytes()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let folder = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("folder");
    (dir, store, blobs, account, folder)
}

#[test]
fn validity_is_recorded_and_read_back() {
    let (_dir, mut store, _blobs, _account, folder) = setup();
    assert_eq!(store.folder_validity(folder).unwrap(), None);
    store.set_folder_validity(folder, Some(1111)).unwrap();
    assert_eq!(store.folder_validity(folder).unwrap(), Some(1111));
}

#[test]
fn a_reset_remaps_by_message_id_and_refetches_only_strangers() {
    let (_dir, mut store, blobs, account, folder) = setup();
    // Three messages synced under the old numbering: UIDs 101, 102, 103.
    for (uid, mid, subj) in [
        (101, "alpha@x", "alpha"),
        (102, "beta@x", "beta"),
        (103, "gamma@x", "gamma"),
    ] {
        store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(uid),
                &fixture(mid, subj),
            )
            .expect("ingest");
    }
    store.set_folder_validity(folder, Some(1)).unwrap();

    // The server renumbered: alpha is now 1, gamma is 3, beta is gone from
    // the folder, and a message we never saw (delta) is 4.
    let server = [
        (1u32, Some("alpha@x".to_string())),
        (3, Some("gamma@x".to_string())),
        (4, Some("delta@x".to_string())),
    ];
    let out = store
        .remap_folder_after_reset(folder, &server, true)
        .expect("remap");

    assert_eq!(out.rematched, 2);
    assert_eq!(out.to_fetch, vec![4], "only the stranger is re-downloaded");
    assert_eq!(out.dropped, 1, "beta left the folder");

    // The watermark now speaks the new numbering.
    assert_eq!(store.max_uid(folder).unwrap(), Some(3));
}

#[test]
fn an_incomplete_listing_never_evicts_history() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let mut ids = Vec::new();
    for (uid, mid, subj) in [(500, "old@x", "old"), (900, "new@x", "new")] {
        let out = store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(uid),
                &fixture(mid, subj),
            )
            .expect("ingest");
        ids.push(out.message_id);
    }
    // A depth-limited listing that only reached the newest message.
    let server = [(2u32, Some("new@x".to_string()))];
    let out = store
        .remap_folder_after_reset(folder, &server, false)
        .expect("remap");
    assert_eq!(out.rematched, 1);
    assert_eq!(out.dropped, 0, "the window ending is not the mail ending");

    // The old message is still placed in the folder — just not addressable
    // until a deeper pass learns its new number.
    // Both messages still live in the folder: max_uid sees the remapped one,
    // and the held one keeps its placement (folders_of still names the folder).
    assert_eq!(store.max_uid(folder).unwrap(), Some(2));
    for id in ids {
        assert_eq!(store.folders_of(id).unwrap(), vec![folder]);
    }
}

#[test]
fn quarantine_holds_queued_actions_rather_than_misfiring_them() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(77),
            &fixture("held@x", "held"),
        )
        .expect("ingest");
    let thread = store
        .thread_of(ingested.message_id)
        .expect("thread lookup")
        .expect("threaded");
    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::MarkRead,
            None,
            store.placement_policy(account).expect("policy"),
        )
        .expect("queue");

    // Before the reset the action is deliverable — it knows its UID.
    let before = store.pending_actions(account).expect("pending");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].uid, Some(77));

    // Renumbered, and the listing did not include this message.
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    // The action still exists but no longer names a UID: the drain loop's
    // existing rule for a NULL UID is to hold, so nothing fires at whatever
    // message inherited number 77 on the server.
    let after = store.pending_actions(account).expect("pending");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].uid, None);
}

#[test]
fn recovery_never_deletes_a_blob_or_a_message() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(11),
            &fixture("kept@x", "kept"),
        )
        .expect("ingest");
    // Complete listing, and the server no longer has the message at all.
    let out = store
        .remap_folder_after_reset(folder, &[], true)
        .expect("remap");
    assert_eq!(out.dropped, 1);

    // The placement is gone; the message and its bytes are not. Worst case
    // costs a re-download, never data.
    let hash = store
        .blob_hash_for(a.message_id)
        .expect("query")
        .expect("message row survives");
    assert!(
        blobs.read(&hash).is_ok(),
        "the raw bytes must still be readable"
    );
}

#[test]
fn a_quarantined_action_still_names_its_message_and_where_to_ask() {
    // Quarantine nulls the UID, and that must hold — but the action is not
    // thereby lost. The pending row carries the Message-ID and the folder a
    // drain can search, so delivery can ask the server for the new number
    // instead of holding the change forever.
    let (_dir, mut store, blobs, account, folder) = setup();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(77),
            &fixture("held@x", "held"),
        )
        .expect("ingest");
    let thread = store
        .thread_of(ingested.message_id)
        .expect("thread lookup")
        .expect("threaded");
    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::MarkRead,
            None,
            store.placement_policy(account).expect("policy"),
        )
        .expect("queue");
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    let after = store.pending_actions(account).expect("pending");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].uid, None, "quarantine still holds the number");
    assert_eq!(after[0].msgid.as_deref(), Some("held@x"));
    assert_eq!(after[0].candidate_paths, vec!["INBOX".to_string()]);

    // A deliverable action asks nothing: known UID, no candidates.
    let fresh = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(90),
            &fixture("fresh@x", "fresh"),
        )
        .expect("ingest");
    let t2 = store.thread_of(fresh.message_id).unwrap().unwrap();
    store
        .apply_thread_action(
            account,
            t2,
            petrel_engine::actions::ActionKind::Star,
            None,
            store.placement_policy(account).expect("policy"),
        )
        .expect("queue");
    let again = store.pending_actions(account).expect("pending");
    let starred = again.iter().find(|a| a.uid == Some(90)).expect("fresh row");
    assert!(starred.candidate_paths.is_empty());
}

#[test]
fn healing_fills_only_the_number_that_was_lost() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(41),
            &fixture("a@x", "a"),
        )
        .expect("ingest");
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    // The server, asked by Message-ID, says the message is now UID 900.
    assert!(
        store
            .heal_placement_uid(a.message_id, account, "INBOX", 900, None)
            .expect("heal")
    );
    assert_eq!(store.max_uid(folder).unwrap(), Some(900));

    // A placement already holding a UID is the sync's business: healing it
    // is refused rather than applied.
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(50),
            &fixture("b@x", "b"),
        )
        .expect("ingest");
    assert!(
        !store
            .heal_placement_uid(b.message_id, account, "INBOX", 999, None)
            .expect("no-op heal")
    );
}

/// The drain's heal writes what a search by Message-ID found, and the
/// number belongs to the folder's UIDVALIDITY as it stood before the search
/// asked (review r1, R1-8). Recovery can record a new one meanwhile, and
/// written after it the old number would pass the server's check against
/// some other message. It is written only while the validity is the one it
/// was asked under.
#[test]
fn a_heal_from_before_a_renumbering_is_not_written_after_it() {
    let (_dir, mut store, blobs, account, folder) = setup();
    store.set_folder_validity(folder, Some(1)).unwrap();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(41),
            &fixture("a@x", "a"),
        )
        .expect("ingest");
    // A renumbering, and a listing that did not reach the message.
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    // The drain read validity 1 and asked; the server said UID 900. Before
    // the heal, recovery finished and recorded validity 2.
    store.set_folder_validity(folder, Some(2)).unwrap();
    assert!(
        !store
            .heal_placement_uid(a.message_id, account, "INBOX", 900, Some(1))
            .expect("heal")
    );
    assert_eq!(
        store.max_uid(folder).unwrap(),
        None,
        "a number from before the renumbering was written after it"
    );

    // Asked under the validity the folder has, the heal goes as before.
    assert!(
        store
            .heal_placement_uid(a.message_id, account, "INBOX", 7, Some(2))
            .expect("heal")
    );
    assert_eq!(store.max_uid(folder).unwrap(), Some(7));
}

/// Review r1, R1-1. A move queued by a build from before validities were
/// kept names its source by a number with no UIDVALIDITY. Paired with the
/// folder's validity as it stands, the number went out under the new one
/// once the sync had mapped the folder again, passed the server's check, and
/// moved whichever message held it then. It is asked for by Message-ID
/// instead, in the folder the move took the message out of.
#[test]
fn a_move_queued_before_validities_were_kept_is_asked_for_by_message_id() {
    let (_dir, mut store, blobs, account, inbox) = setup();
    store
        .ensure_folder(account, "archive", "Archive")
        .expect("archive");
    let mut ids = Vec::new();
    for (uid, mid, subj) in [(1, "z@x", "Z"), (2, "a@x", "A"), (3, "b@x", "B")] {
        ids.push(
            store
                .ingest_raw(&blobs, account, Some(inbox), Some(uid), &fixture(mid, subj))
                .expect("ingest")
                .message_id,
        );
    }
    // Queued by the old build: INBOX UID 2, and no validity. Queued here
    // while the folder's reads NULL, which writes `null` into the payload,
    // read as the old build's missing key is.
    let a = ids[1];
    let thread = store.thread_of(a).unwrap().unwrap_or(-a);
    store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .expect("queue");
    store.set_folder_validity(inbox, Some(1)).unwrap();

    // The server deleted Z and renumbered: A is 1 now and B 2. Recovery's
    // listing did not reach A, older than its window, and it recorded
    // validity 2.
    store
        .remap_folder_after_reset(inbox, &[(2, Some("b@x".to_string()))], false)
        .expect("remap");
    store.set_folder_validity(inbox, Some(2)).unwrap();

    let rows = store.pending_actions(account).expect("pending");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].uid, None,
        "UID 2 under validity 2 is msg B: {rows:?}"
    );
    assert_eq!(rows[0].folder_path, "INBOX");
    assert_eq!(rows[0].candidate_paths, vec!["INBOX".to_string()]);
}

/// A folder never numbered has nothing to check a number against: Gmail's
/// All Mail is never asked its UIDVALIDITY. Its numbers go as they are.
#[test]
fn a_move_from_a_folder_never_numbered_keeps_its_number() {
    let (_dir, mut store, blobs, account, inbox) = setup();
    store
        .ensure_folder(account, "archive", "Archive")
        .expect("archive");
    let a = store
        .ingest_raw(&blobs, account, Some(inbox), Some(5), &fixture("a@x", "A"))
        .expect("ingest")
        .message_id;
    let thread = store.thread_of(a).unwrap().unwrap_or(-a);
    store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .expect("queue");
    let rows = store.pending_actions(account).expect("pending");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].uid, rows[0].uid_validity), (Some(5), None));
}

/// Review r1, R1-4. Undo puts a move's source placement back with the UID
/// captured when the move was queued. After a renumbering the re-map has
/// mended, that number is another message's under the folder's new
/// UIDVALIDITY, and every later change to this one (a star, a read, Delete
/// forever) was addressed at it and passed the server's check. The number
/// goes back only under the validity it was captured with; otherwise the
/// placement has none, and the drain asks the folder when it needs one.
#[test]
fn undo_after_a_remap_puts_back_no_number_from_before_it() {
    let (_dir, mut store, blobs, account, inbox) = setup();
    store
        .ensure_folder(account, "archive", "Archive")
        .expect("archive");
    let mut ids = Vec::new();
    for (uid, mid, subj) in [(1, "z@x", "Z"), (2, "a@x", "A"), (3, "b@x", "B")] {
        ids.push(
            store
                .ingest_raw(&blobs, account, Some(inbox), Some(uid), &fixture(mid, subj))
                .expect("ingest")
                .message_id,
        );
    }
    store.set_folder_validity(inbox, Some(1)).unwrap();
    let a = ids[1];

    // Archived while offline: INBOX UID 2 under validity 1.
    let thread = store.thread_of(a).unwrap().unwrap_or(-a);
    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .expect("queue");

    // The server deleted Z and renumbered: A is 1, B 2. Recovery, in
    // `recover_folder`'s order: the re-map, the re-fetch of what it could not
    // match (A, whose inbox placement the archive took), then the validity.
    let server = [
        (1u32, Some("a@x".to_string())),
        (2, Some("b@x".to_string())),
    ];
    let out = store
        .remap_folder_after_reset(inbox, &server, true)
        .expect("remap");
    assert_eq!(out.to_fetch, vec![1], "A is fetched again");
    store
        .ingest_raw(&blobs, account, Some(inbox), Some(1), &fixture("a@x", "A"))
        .expect("refetch");
    store.set_folder_validity(inbox, Some(2)).unwrap();

    // The drain has not delivered the archive yet, and the person presses Z.
    assert!(store.undo_action(receipt.action_id).unwrap());

    // Any later change to A is addressed by its inbox placement.
    let star = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Star,
            None,
            PlacementPolicy::Exclusive,
        )
        .expect("star");
    let rows: Vec<_> = store
        .pending_actions(account)
        .expect("pending")
        .into_iter()
        .filter(|r| r.action_id == star.action_id && r.message_id == a)
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_ne!(
        rows[0].uid,
        Some(2),
        "A's star is addressed at UID 2 under validity 2, which is msg B: {rows:?}"
    );
}

/// And while the folder keeps its numbering, undo puts the number back as
/// it always did.
#[test]
fn undo_puts_the_number_back_while_the_numbering_stands() {
    let (_dir, mut store, blobs, account, inbox) = setup();
    store
        .ensure_folder(account, "archive", "Archive")
        .expect("archive");
    store.set_folder_validity(inbox, Some(1)).unwrap();
    let a = store
        .ingest_raw(&blobs, account, Some(inbox), Some(2), &fixture("a@x", "A"))
        .expect("ingest")
        .message_id;
    let thread = store.thread_of(a).unwrap().unwrap_or(-a);
    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .expect("queue");
    assert!(store.undo_action(receipt.action_id).unwrap());
    assert_eq!(store.placement_uid(a, inbox).unwrap(), Some(Some(2)));
}

#[test]
fn the_sweep_removes_only_what_the_server_no_longer_holds() {
    let (_dir, mut store, blobs, account, folder) = setup();
    for (uid, mid) in [(11, "a@x"), (12, "b@x"), (13, "c@x")] {
        store
            .ingest_raw(&blobs, account, Some(folder), Some(uid), &fixture(mid, mid))
            .expect("ingest");
    }
    // One placement quarantined: its number is NULL, and no server answer
    // about live UIDs says anything about it.
    let held = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(14),
            &fixture("d@x", "d"),
        )
        .expect("ingest");
    store
        .remap_folder_after_reset(
            folder,
            &[
                (11, Some("a@x".into())),
                (12, Some("b@x".into())),
                (13, Some("c@x".into())),
            ],
            // Incomplete, so the unlisted fourth message is held, not evicted.
            false,
        )
        .expect("remap");
    assert_eq!(store.uid_placement_count(folder).unwrap(), 3);

    // The server now holds only 11 and 13: 12 was moved elsewhere.
    let present: std::collections::HashSet<u32> = [11, 13].into_iter().collect();
    let removed = store
        .remove_placements_absent(folder, &present)
        .expect("sweep");
    assert_eq!(removed, 1);
    assert_eq!(store.uid_placement_count(folder).unwrap(), 2);
    // The quarantined message keeps its placement.
    assert_eq!(store.folders_of(held.message_id).unwrap(), vec![folder]);
}

#[test]
fn a_delivered_move_takes_the_source_placement_with_it() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(21),
            &fixture("m@x", "m"),
        )
        .expect("ingest");
    assert!(
        store
            .remove_placement(a.message_id, account, "INBOX")
            .expect("remove")
    );
    assert!(store.folders_of(a.message_id).unwrap().is_empty());
    // Absent placement: nothing to remove, and saying so is not an error.
    assert!(
        !store
            .remove_placement(a.message_id, account, "INBOX")
            .expect("no-op")
    );
}

#[test]
fn a_second_live_copy_is_its_own_message() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let first = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(70),
            &fixture("twice@x", "draft one"),
        )
        .expect("ingest");
    // Same Message-ID, different content, both live on the server: the
    // second copy keeps its own row and placement rather than overwriting
    // the first.
    let raw2 = fixture("twice@x", "draft two");
    let second = store
        .ingest_raw_second_copy(&blobs, account, Some(folder), 71, &raw2)
        .expect("second copy");
    assert_ne!(first.message_id, second.message_id);
    assert_eq!(store.uid_placement_count(folder).unwrap(), 2);
    // A refetch of the same copy lands on the same row, not a third.
    let again = store
        .ingest_raw_second_copy(&blobs, account, Some(folder), 71, &raw2)
        .expect("refetch");
    assert_eq!(again.message_id, second.message_id);
    assert_eq!(store.uid_placement_count(folder).unwrap(), 2);
}
