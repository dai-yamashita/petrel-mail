//! One folder per role.
//!
//! A server may flag several mailboxes with the same special use: cPanel's
//! Dovecot flags `Trash` and Apple's `Deleted Messages` both `\Trash`, `Spam`
//! and `Junk` both `\Junk`, and `Sent`, `Sent Items` and `Sent Messages` all
//! `\Sent`. When every one of them wore the role, the sync took the first in
//! rail order and triage took the lowest id, so the two disagreed — and on a
//! real account the folder synced as the Trash was one the server had
//! deleted weeks before. These pin the rule that settles it: one holder per
//! role, the same for every reader, and nobody's mail lost on the way.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn rows(list: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
    list.iter()
        .map(|(p, r)| (p.to_string(), r.map(|r| r.to_string())))
        .collect()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    (dir, store, blobs, account)
}

fn raw(mid: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\n\
         Subject: {mid}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{mid}@x.example>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nbody {mid}\r\n"
    )
    .into_bytes()
}

/// The paths holding `role`, in id order.
fn holders(store: &Store, account: i64, role: &str) -> Vec<String> {
    let mut v: Vec<(i64, String)> = store
        .folders(account)
        .unwrap()
        .into_iter()
        .filter(|f| f.role == role)
        .map(|f| (f.id, f.path))
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

fn path_of(store: &Store, id: i64) -> String {
    store.folder_path(id).unwrap().unwrap()
}

fn id_of(store: &Store, account: i64, path: &str) -> Option<i64> {
    store
        .folders(account)
        .unwrap()
        .into_iter()
        .find(|f| f.path == path)
        .map(|f| f.id)
}

fn tombstoned(dir: &tempfile::TempDir, message_id: i64) -> bool {
    let c = rusqlite::Connection::open(dir.path().join("petrel.db")).unwrap();
    c.query_row(
        "SELECT deleted_at_ms IS NOT NULL FROM messages WHERE id = ?1",
        [message_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// What cPanel's Dovecot lists once Apple Mail and Outlook have both been
/// pointed at the account.
const CPANEL: &[(&str, Option<&str>)] = &[
    ("INBOX", Some("inbox")),
    ("Drafts", Some("drafts")),
    ("Sent Messages", Some("sent")),
    ("Sent", Some("sent")),
    ("Sent Items", Some("sent")),
    ("Deleted Messages", Some("trash")),
    ("Trash", Some("trash")),
    ("Junk", Some("spam")),
    ("Spam", Some("spam")),
    ("Archive", None),
];

#[test]
fn a_survey_flagging_several_folders_with_one_role_leaves_one_holder() {
    let (_dir, mut store, _blobs, a) = setup();
    store.sync_folders(a, &rows(CPANEL)).unwrap();
    // Nothing holds mail yet, so the conventional name decides — not the
    // order the server listed them in, which put the Apple ones first.
    assert_eq!(holders(&store, a, "sent"), vec!["Sent"]);
    assert_eq!(holders(&store, a, "trash"), vec!["Trash"]);
    assert_eq!(holders(&store, a, "spam"), vec!["Spam"]);
    assert_eq!(holders(&store, a, "drafts"), vec!["Drafts"]);
    assert_eq!(holders(&store, a, "archive"), vec!["Archive"]);
    // The others are still there, as ordinary folders: their mail is the
    // person's, and Thunderbird lists every one of them too.
    for path in ["Sent Messages", "Sent Items", "Deleted Messages", "Junk"] {
        let id = id_of(&store, a, path).unwrap_or_else(|| panic!("{path} kept"));
        assert!(
            store
                .folders(a)
                .unwrap()
                .iter()
                .any(|f| f.id == id && f.role.is_empty()),
            "{path} is an ordinary folder now"
        );
    }
    // Every reader agrees: what triage files into is what the rail lists.
    for role in ["sent", "trash", "spam", "drafts", "archive"] {
        let filed = store.folder_for_role(a, role).unwrap().unwrap();
        assert_eq!(vec![path_of(&store, filed)], holders(&store, a, role));
    }
    // And it holds: the next survey, listing the same, changes nothing.
    store.sync_folders(a, &rows(CPANEL)).unwrap();
    assert_eq!(holders(&store, a, "trash"), vec!["Trash"]);
    assert_eq!(holders(&store, a, "spam"), vec!["Spam"]);
    assert_eq!(holders(&store, a, "sent"), vec!["Sent"]);
}

#[test]
fn where_several_already_hold_a_role_the_one_with_mail_keeps_it() {
    // The shape a real store was left in before this rule existed: every
    // flagged folder wore the role, and the empty Apple-convention ones sat
    // first in the rail, so they were what the sync asked for.
    let (dir, mut store, blobs, a) = setup();
    store
        .sync_folders(
            a,
            &rows(&[
                ("INBOX", Some("inbox")),
                ("Sent", Some("sent")),
                ("Spam", Some("spam")),
                ("Trash", Some("trash")),
            ]),
        )
        .unwrap();
    let spam = store.folder_for_role(a, "spam").unwrap().unwrap();
    let trash = store.folder_for_role(a, "trash").unwrap().unwrap();
    let sent = store.folder_for_role(a, "sent").unwrap().unwrap();
    let junk_mail = store
        .ingest_raw(&blobs, a, Some(spam), Some(7), &raw("s1"))
        .unwrap()
        .message_id;
    store
        .ingest_raw(&blobs, a, Some(trash), Some(3), &raw("t1"))
        .unwrap();
    store
        .ingest_raw(&blobs, a, Some(sent), Some(9), &raw("o1"))
        .unwrap();
    {
        // Lower ids than nothing, but first in the rail by position: the
        // ordering that made the sync pick them.
        let c = rusqlite::Connection::open(dir.path().join("petrel.db")).unwrap();
        for (path, role, order) in [
            ("Junk", "spam", 1),
            ("Deleted Messages", "trash", 2),
            ("Sent Messages", "sent", 3),
        ] {
            c.execute(
                "INSERT INTO folders(account_id, role, name, path, sort_order)
                 VALUES (?1, ?2, ?3, ?3, ?4)",
                rusqlite::params![a, role, path, order],
            )
            .unwrap();
        }
    }
    assert_eq!(holders(&store, a, "spam").len(), 2, "the old shape");

    // The server still lists Junk and Sent Messages; Deleted Messages it
    // dropped.
    store
        .sync_folders(
            a,
            &rows(&[
                ("INBOX", Some("inbox")),
                ("Sent", Some("sent")),
                ("Sent Messages", Some("sent")),
                ("Spam", Some("spam")),
                ("Junk", Some("spam")),
                ("Trash", Some("trash")),
            ]),
        )
        .unwrap();
    assert_eq!(holders(&store, a, "spam"), vec!["Spam"]);
    assert_eq!(holders(&store, a, "trash"), vec!["Trash"]);
    assert_eq!(holders(&store, a, "sent"), vec!["Sent"]);
    assert!(
        id_of(&store, a, "Deleted Messages").is_none(),
        "a role folder the server no longer has is pruned"
    );
    assert!(id_of(&store, a, "Junk").is_some(), "Junk is still listed");
    assert!(!tombstoned(&dir, junk_mail), "nobody's mail went anywhere");
    assert_eq!(store.folders_of(junk_mail).unwrap(), vec![spam]);
}

#[test]
fn the_holder_keeps_the_role_after_it_empties() {
    // Empty Trash leaves the Trash with nothing in it, and the Apple folder
    // beside it may still hold mail another client binned. The role must
    // not jump to that folder: the bin just emptied would fill with mail the
    // person never threw away here.
    let (_dir, mut store, blobs, a) = setup();
    store.sync_folders(a, &rows(CPANEL)).unwrap();
    let apple = id_of(&store, a, "Deleted Messages").unwrap();
    store
        .ingest_raw(&blobs, a, Some(apple), Some(4), &raw("elsewhere"))
        .unwrap();
    store.sync_folders(a, &rows(CPANEL)).unwrap();
    assert_eq!(holders(&store, a, "trash"), vec!["Trash"]);
}

#[test]
fn a_local_role_folder_is_never_pruned() {
    let (_dir, mut store, _blobs, a) = setup();
    store
        .sync_folders(
            a,
            &rows(&[("INBOX", Some("inbox")), ("Archive", Some("archive"))]),
        )
        .unwrap();
    let archive = store.folder_for_role(a, "archive").unwrap().unwrap();
    store.mark_folder_local(archive).unwrap();
    store
        .sync_folders(a, &rows(&[("INBOX", Some("inbox"))]))
        .unwrap();
    assert_eq!(store.folder_for_role(a, "archive").unwrap(), Some(archive));
}

#[test]
fn a_role_folder_made_here_waits_for_the_server() {
    // Nothing on the server is an archive, so archiving invents one here and
    // the drain creates it there. A survey landing in between must not prune
    // it: its message is placed nowhere else.
    let (dir, mut store, blobs, a) = setup();
    store
        .sync_folders(a, &rows(&[("INBOX", Some("inbox"))]))
        .unwrap();
    let inbox = store.folder_for_role(a, "inbox").unwrap().unwrap();
    let m = store
        .ingest_raw(&blobs, a, Some(inbox), Some(1), &raw("a1"))
        .unwrap()
        .message_id;
    let made = store.ensure_folder(a, "archive", "archive").unwrap();
    store.place_message(m, made).unwrap();
    store
        .sync_folders(a, &rows(&[("INBOX", Some("inbox"))]))
        .unwrap();
    assert_eq!(store.folder_for_role(a, "archive").unwrap(), Some(made));
    assert!(!tombstoned(&dir, m));
}

#[test]
fn a_listed_archive_takes_over_from_one_made_here_and_keeps_its_mail() {
    // A draft or an archive made before the account's first survey invents
    // the folder locally. When the survey then lists the server's own, that
    // is the one every reader must use — and what was filed into the
    // invented one goes with the role, not into the void.
    let (dir, mut store, blobs, a) = setup();
    store
        .sync_folders(a, &rows(&[("INBOX", Some("inbox"))]))
        .unwrap();
    let inbox = store.folder_for_role(a, "inbox").unwrap().unwrap();
    let m = store
        .ingest_raw(&blobs, a, Some(inbox), Some(1), &raw("a2"))
        .unwrap()
        .message_id;
    let made = store.ensure_folder(a, "archive", "archive").unwrap();
    store.place_message(m, made).unwrap();
    store
        .sync_folders(
            a,
            &rows(&[("INBOX", Some("inbox")), ("Archive", Some("archive"))]),
        )
        .unwrap();
    let archive = store.folder_for_role(a, "archive").unwrap().unwrap();
    assert_eq!(path_of(&store, archive), "Archive");
    assert_eq!(holders(&store, a, "archive"), vec!["Archive"]);
    assert!(
        id_of(&store, a, "archive").is_none(),
        "the invented one goes"
    );
    assert!(!tombstoned(&dir, m));
    assert!(store.folders_of(m).unwrap().contains(&archive));
}

#[test]
fn a_mailbox_flagged_all_never_holds_the_archive_away_from_one_that_is() {
    // Dovecot's virtual/All took the role from \All, and a plain Archive sat
    // beside it role-less. Archiving filed into the view of everything, which
    // the server cannot move mail into.
    let (_dir, mut store, _blobs, a) = setup();
    let survey = rows(&[
        ("INBOX", Some("inbox")),
        ("virtual/All", Some("archive")),
        ("Archive", None),
    ]);
    store.sync_folders(a, &survey).unwrap();
    store
        .set_all_mail_folders(a, &["virtual/All".to_string()])
        .unwrap();
    store.sync_folders(a, &survey).unwrap();
    assert_eq!(holders(&store, a, "archive"), vec!["Archive"]);
    let all = id_of(&store, a, "virtual/All").unwrap();
    assert!(store.folder_is_all_mail(all).unwrap());
}

#[test]
fn gmails_all_mail_stays_the_archive() {
    let (_dir, mut store, _blobs, a) = setup();
    store.set_account_kind(a, "gmail").unwrap();
    let survey = rows(&[
        ("INBOX", Some("inbox")),
        ("[Gmail]/All Mail", Some("archive")),
        ("[Gmail]/Sent Mail", Some("sent")),
        ("[Gmail]/Spam", Some("spam")),
        ("[Gmail]/Trash", Some("trash")),
        ("[Gmail]/Starred", Some("starred")),
        ("[Gmail]/Drafts", Some("drafts")),
    ]);
    store.sync_folders(a, &survey).unwrap();
    // Even marked the way a survey that took Gmail for something else would
    // mark it: Gmail's All Mail is where archived mail lives.
    store
        .set_all_mail_folders(a, &["[Gmail]/All Mail".to_string()])
        .unwrap();
    store.sync_folders(a, &survey).unwrap();
    assert_eq!(holders(&store, a, "archive"), vec!["[Gmail]/All Mail"]);
    assert_eq!(holders(&store, a, "sent"), vec!["[Gmail]/Sent Mail"]);
    assert_eq!(holders(&store, a, "starred"), vec!["[Gmail]/Starred"]);
}
