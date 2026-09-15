//! Sparse views must page from their membership, not the mailbox.
//!
//! Walking every message looking for a sent or starred row is the plan
//! the counts used to take — 300ms to find thirteen drafts. These tests
//! keep a large inbox next to a handful of matching rows so a walk of
//! the mailbox would still *work*, and only the membership path stays
//! cheap. Correctness is what we can assert here: the list is those
//! rows, and only those rows.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{ListView, NewMessage, Store, flags};

fn store() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let account = s.ensure_test_account().unwrap();
    (s, account)
}

fn fill_inbox(store: &mut Store, account: i64, n: i64) -> Vec<i64> {
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let msgs: Vec<NewMessage> = (0..n)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 10_000 + i,
            from_addr: "a@example.com".into(),
            from_display: "A".into(),
            to_addr: "me@example.com".into(),
            subject: format!("inbox-{i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    ids
}

fn subjects(store: &Store, view: &ListView) -> Vec<String> {
    store
        .list_threads(view, 0, 50, petrel_engine::store::Sort::default())
        .unwrap()
        .into_iter()
        .map(|r| r.subject)
        .collect()
}

fn place_named(store: &mut Store, account: i64, folder: i64, subject: &str, date_ms: i64) -> i64 {
    let ids = store
        .insert_messages(&[NewMessage {
            account_id: account,
            date_ms,
            from_addr: "b@example.com".into(),
            from_display: "B".into(),
            to_addr: "me@example.com".into(),
            subject: subject.into(),
            body_text: "body".into(),
        }])
        .unwrap();
    store.place_message(ids[0], folder).unwrap();
    ids[0]
}

#[test]
fn sent_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 200);
    let sent = s.ensure_folder(account, "sent", "Sent").unwrap();
    place_named(&mut s, account, sent, "old-sent", 1_000);
    place_named(&mut s, account, sent, "new-sent", 2_000);

    let found = subjects(&s, &ListView::Folder("sent".into()));
    assert_eq!(found.len(), 2);
    assert!(found.contains(&"old-sent".into()));
    assert!(found.contains(&"new-sent".into()));
    assert!(
        !found.iter().any(|s| s.starts_with("inbox-")),
        "inbox mail must not leak into sent"
    );
}

#[test]
fn spam_and_trash_lists_find_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 200);
    let spam = s.ensure_folder(account, "spam", "Spam").unwrap();
    let trash = s.ensure_folder(account, "trash", "Trash").unwrap();
    place_named(&mut s, account, spam, "junk", 1_000);
    place_named(&mut s, account, trash, "gone", 2_000);

    assert_eq!(subjects(&s, &ListView::Folder("spam".into())), ["junk"]);
    assert_eq!(subjects(&s, &ListView::Folder("trash".into())), ["gone"]);
}

#[test]
fn a_user_folder_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 200);
    let filed = s.ensure_folder(account, "", "Contracts").unwrap();
    place_named(&mut s, account, filed, "contract", 1_000);

    let found = subjects(&s, &ListView::UserFolder(filed));
    assert_eq!(found, ["contract"]);
}

#[test]
fn a_tag_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    let inbox_ids = fill_inbox(&mut s, account, 200);
    let tag = s.ensure_tag(account, "Urgent", None).unwrap();
    s.tag_message(inbox_ids[3], tag).unwrap();
    s.tag_message(inbox_ids[9], tag).unwrap();

    let mut found = subjects(&s, &ListView::Tag("Urgent".into()));
    found.sort();
    assert_eq!(found, ["inbox-3", "inbox-9"]);
}

#[test]
fn starred_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    let inbox_ids = fill_inbox(&mut s, account, 200);
    s.set_flags(inbox_ids[5], flags::FLAGGED, 0).unwrap();
    s.set_flags(inbox_ids[17], flags::FLAGGED, 0).unwrap();

    let mut found = subjects(&s, &ListView::Starred);
    found.sort();
    assert_eq!(found, ["inbox-17", "inbox-5"]);
}

#[test]
fn snoozed_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    let inbox_ids = fill_inbox(&mut s, account, 200);
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        + 600_000;
    let tid = s.thread_of(inbox_ids[8]).unwrap().unwrap_or(-inbox_ids[8]);
    s.apply_thread_action(
        account,
        tid,
        ActionKind::Snooze,
        Some(until),
        PlacementPolicy::Exclusive,
    )
    .unwrap();

    assert_eq!(subjects(&s, &ListView::Snoozed), ["inbox-8"]);
}
