//! Blind copies in the store: kept with a draft for as long as it lives, shown
//! on the sender's own copy, and never offered back as somebody to reply to.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{DraftEnvelope, Store};

const BCC: &str = "Priya Nair <priya@example.net>, board@example.org";

fn draft_with_bcc(store: &Store, account: i64) -> i64 {
    let envelope = DraftEnvelope {
        bcc: BCC.into(),
        ..Default::default()
    };
    store
        .save_draft_full(
            account,
            None,
            "dana@example.com",
            "alex@example.com",
            "Quarterly numbers",
            "The numbers are in.",
            "",
            &envelope,
        )
        .unwrap()
}

#[test]
fn a_drafts_blind_copies_are_kept_with_it() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = draft_with_bcc(&store, account);
    let d = store.load_draft(id).unwrap();
    assert_eq!(d.envelope.bcc, BCC);
    assert_eq!(d.to, "dana@example.com");
    assert_eq!(d.cc, "alex@example.com");

    // Saved again without them, they are gone: the composer is the authority.
    store
        .save_draft_full(
            account,
            Some(id),
            "dana@example.com",
            "",
            "Quarterly numbers",
            "The numbers are in.",
            "",
            &DraftEnvelope::default(),
        )
        .unwrap();
    assert_eq!(store.load_draft(id).unwrap().envelope.bcc, "");
}

/// Drafts saved before there was a Bcc carry an envelope without the field.
#[test]
fn an_envelope_from_before_bcc_reads_as_none() {
    let old: DraftEnvelope =
        serde_json::from_str(r#"{"in_reply_to":null,"references":[],"attachments":[]}"#).unwrap();
    assert_eq!(old.bcc, "");
}

/// Undo or Edit in the Outbox hands the message back to be edited. The blind
/// copies come back with it, including after it was deleted forever while it
/// waited and returns as a new draft.
#[test]
fn a_message_pulled_back_from_the_outbox_keeps_its_blind_copies() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();

    let waiting = draft_with_bcc(&store, account);
    store.schedule_send(waiting, Some(1_000)).unwrap();
    assert_eq!(
        store.due_sends(account, 5_000).unwrap()[0].envelope.bcc,
        BCC
    );
    let back = store.pull_back(waiting).unwrap();
    assert_eq!(store.load_draft(back).unwrap().envelope.bcc, BCC);

    let deleted = draft_with_bcc(&store, account);
    store.schedule_send(deleted, Some(1_000)).unwrap();
    store
        .apply_message_action(
            account,
            deleted,
            ActionKind::DeleteForever,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let reopened = store.pull_back(deleted).unwrap();
    assert_ne!(reopened, deleted, "a new draft, not the deleted row");
    assert_eq!(store.load_draft(reopened).unwrap().envelope.bcc, BCC);
}

const SENT_COPY: &[u8] = b"From: Sam Ortiz <sam@example.com>\r\n\
To: Dana Wu <dana@example.com>\r\nCc: alex@example.com\r\n\
Bcc: Priya Nair <priya@example.net>, board@example.org\r\n\
Subject: Quarterly numbers\r\nDate: Mon, 7 Sep 2026 09:00:00 +0000\r\n\
Message-ID: <q1@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nThe numbers are in.\r\n";

/// The copy in Sent says whom it blind-copied, as Apple Mail and Thunderbird
/// show it. Reply and Reply all never see them: following up on your own
/// message goes to the people you wrote to openly.
#[test]
fn a_sent_copy_shows_its_blind_copies_and_never_offers_them_to_reply_to() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let id = store
        .ingest_raw(&blobs, account, Some(sent), Some(3), SENT_COPY)
        .unwrap()
        .message_id;
    let thread = store.thread_of(id).unwrap().unwrap();
    let detail = store.thread_detail(thread).unwrap();
    let m = &detail[0];
    assert_eq!(
        m.bcc,
        vec!["Priya Nair".to_string(), "board@example.org".to_string()]
    );
    assert_eq!(m.to, vec!["Dana Wu".to_string()]);
    assert_eq!(m.cc, vec!["alex@example.com".to_string()]);
    assert_eq!(
        m.recipients,
        vec!["Dana Wu".to_string(), "alex@example.com".to_string()]
    );
    assert_eq!(
        m.recipient_addrs,
        vec![
            "dana@example.com".to_string(),
            "alex@example.com".to_string()
        ]
    );
}

/// Someone blind-copied is someone written to: they rank as such when an
/// address is typed, and their picture is shown, as for anyone else written to.
#[test]
fn a_blind_copy_counts_as_someone_written_to() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    store
        .ingest_raw(&blobs, account, Some(sent), Some(3), SENT_COPY)
        .unwrap();
    assert!(store.has_written_to(account, "priya@example.net").unwrap());
    let offered = store
        .complete_addresses(account, "pri", 1_790_000_000_000, 10)
        .unwrap();
    let priya = offered
        .iter()
        .find(|c| c.addr == "priya@example.net")
        .expect("offered while typing");
    assert!(priya.written_to, "{offered:?}");
}
