//! "Select all" means the whole view, not the rows a window has loaded.
//!
//! The window pages a list a hundred conversations at a time, so an action on
//! everything in it is asked of the engine, which walks the view itself. The
//! property under test is that the walk *is* the list: the same conversations
//! its pages show, every one of them, so "all" can never quietly mean the
//! first page, or a different set from the one on screen.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{ListView, NewMessage, Sort, SortKey, Store};
use std::collections::HashSet;

/// A mailbox of `n` separate conversations, all in the inbox — more than two
/// of the window's pages.
fn mailbox(n: usize) -> (Store, i64) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = (0..n)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 1_000 + i as i64,
            from_addr: format!("p{i}@example.com"),
            from_display: format!("P{i}"),
            to_addr: "me@example.com".into(),
            subject: format!("conversation {i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    (store, account)
}

/// The window's way through a view: a page, then the page after its last row.
fn paged(store: &Store, view: &ListView) -> Vec<i64> {
    let sort = Sort {
        key: SortKey::Date,
        ascending: false,
    };
    let mut keys = Vec::new();
    let mut page = store.list_threads(view, 0, 100, sort).unwrap();
    while let Some(last) = page.last().cloned() {
        keys.extend(page.iter().map(|r| r.thread_id));
        page = store
            .list_threads_after(view, 100, sort, last.date_ms, last.thread_id)
            .unwrap();
    }
    keys
}

#[test]
fn the_whole_view_is_every_conversation_its_pages_show() {
    let (store, account) = mailbox(250);
    let all = store.conversation_keys(account, &ListView::Inbox).unwrap();
    assert_eq!(all.len(), 250, "every conversation, not the first page");
    let walked: HashSet<i64> = all.iter().copied().collect();
    assert_eq!(walked.len(), 250, "each once");
    let shown: HashSet<i64> = paged(&store, &ListView::Inbox).into_iter().collect();
    assert_eq!(walked, shown, "the same set the list scrolls through");
}

#[test]
fn the_whole_view_leaves_out_what_the_view_does_not_list() {
    let (store, account) = mailbox(250);
    let first = store.conversation_keys(account, &ListView::Inbox).unwrap();
    // Ten filed away first: the inbox no longer lists them, so "all of the
    // inbox" must not reach them either.
    for key in &first[..10] {
        store
            .apply_thread_action(
                account,
                *key,
                ActionKind::Archive,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }
    let left: HashSet<i64> = store
        .conversation_keys(account, &ListView::Inbox)
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(left.len(), 240);
    assert!(first[..10].iter().all(|k| !left.contains(k)));
}

#[test]
fn acting_on_all_of_them_reaches_past_the_first_page() {
    let (store, account) = mailbox(250);
    for key in store.conversation_keys(account, &ListView::Inbox).unwrap() {
        let receipt = store
            .apply_thread_action(
                account,
                key,
                ActionKind::Archive,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        assert_eq!(receipt.message_count, 1);
    }
    assert!(
        store
            .conversation_keys(account, &ListView::Inbox)
            .unwrap()
            .is_empty()
    );
    let sort = Sort {
        key: SortKey::Date,
        ascending: false,
    };
    assert!(
        store
            .list_threads(&ListView::Inbox, 0, 100, sort)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .conversation_keys(account, &ListView::Folder("archive".into()))
            .unwrap()
            .len(),
        250,
        "all of them filed, not a page of them"
    );
}
