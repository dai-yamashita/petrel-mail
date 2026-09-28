//! The outbox as rows in the store: what is due, what is held, what goes back.
//!
//! The reconciliation *rule* is proved in `outbox.rs` and against a real
//! fault-injected server in `ambiguous_send`. This is the other half — that the
//! store honours it. A message held for a person must never come back from
//! `due_sends` on its own, because the worker sends whatever that returns.

use petrel_engine::outbox::SendState;
use petrel_engine::store::Store;

fn queued(store: &Store, account: i64, at_ms: i64) -> i64 {
    let id = store
        .save_draft(
            account,
            None,
            "sam@example.com",
            "Board pack v4",
            "body",
            "",
        )
        .unwrap();
    store.schedule_send(id, Some(at_ms)).unwrap();
    id
}

#[test]
fn a_message_held_for_a_person_is_never_due() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);

    // Before anything happened it was due...
    assert_eq!(store.due_sends(account, 5_000).unwrap().len(), 1);

    // ...and once its outcome is unknown and unprovable, it is not — however
    // long it waits. Sending it could send the board pack twice.
    store
        .set_send_state(
            id,
            SendState::NeedsAttention,
            Some("socket closed"),
            None,
            Some("<m@x>"),
        )
        .unwrap();
    assert!(store.due_sends(account, 5_000).unwrap().is_empty());
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());

    // It is still in the outbox, saying so.
    let rows = store.outbox(account).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "NeedsAttention");
    assert_eq!(rows[0].error.as_deref(), Some("socket closed"));
}

#[test]
fn a_permanent_rejection_waits_for_the_person_too() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(
            id,
            SendState::FailedPermanent,
            Some("550 no such user"),
            None,
            None,
        )
        .unwrap();
    // Retrying a 550 gets another 550. It is edit or discard, not wait.
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());
}

#[test]
fn a_retry_waits_for_its_turn_and_counts_its_attempts() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);

    store
        .set_send_state(
            id,
            SendState::RetryQueued,
            Some("connect refused"),
            Some(60_000),
            None,
        )
        .unwrap();
    assert!(
        store.due_sends(account, 30_000).unwrap().is_empty(),
        "not yet"
    );
    assert_eq!(store.due_sends(account, 60_000).unwrap().len(), 1, "now");

    let row = &store.outbox(account).unwrap()[0];
    assert_eq!(row.attempts, 1);
    assert_eq!(row.next_ms, Some(60_000));
}

#[test]
fn a_person_deciding_is_what_moves_a_held_message() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(
            id,
            SendState::NeedsAttention,
            Some("socket closed"),
            None,
            Some("<m@x>"),
        )
        .unwrap();

    // "Send anyway": they looked, and decided.
    store.resend_now(id, 9_000).unwrap();
    let due = store.due_sends(account, 9_000).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(store.outbox(account).unwrap()[0].state, "RetryQueued");
    assert!(
        store.outbox(account).unwrap()[0].error.is_none(),
        "the old error is cleared"
    );
}

/// The worker reads the due list once per pass and sends one message at a
/// time. A message undone, discarded or re-timed while an earlier one was on
/// the wire was still on that list, and a claim by id alone sent it anyway.
#[test]
fn a_message_is_claimed_only_while_it_is_still_due() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let state = |id: i64| {
        store
            .outbox(account)
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| (r.state, r.error))
    };

    // Due, and claimed once: a second claim finds it on the wire.
    let a = queued(&store, account, 1_000);
    assert!(store.claim_send(a, 5_000).unwrap());
    assert!(!store.claim_send(a, 5_000).unwrap());
    assert_eq!(state(a), Some(("Transmitting".into(), None)));

    // Undone after the pass read its list: back in Drafts, and not claimed.
    let b = queued(&store, account, 1_000);
    assert!(
        store
            .due_sends(account, 5_000)
            .unwrap()
            .iter()
            .any(|d| d.id == b)
    );
    store.unschedule_send(b).unwrap();
    assert!(!store.claim_send(b, 5_000).unwrap());
    assert_eq!(state(b), None, "not in the outbox");
    assert_eq!(store.load_draft(b).unwrap().subject, "Board pack v4");

    // Discarded: nothing left to claim.
    let c = queued(&store, account, 1_000);
    store.delete_draft(c).unwrap();
    assert!(!store.claim_send(c, 5_000).unwrap());

    // Given a later time: not before it.
    let d = queued(&store, account, 1_000);
    store.schedule_send(d, Some(9_000)).unwrap();
    assert!(!store.claim_send(d, 5_000).unwrap());
    assert!(store.claim_send(d, 9_000).unwrap());

    // Waiting out a retry: not before its turn, and the old error goes when
    // it is taken.
    let e = queued(&store, account, 1_000);
    store
        .set_send_state(e, SendState::RetryQueued, Some("421"), Some(8_000), None)
        .unwrap();
    assert!(!store.claim_send(e, 5_000).unwrap());
    assert!(store.claim_send(e, 8_000).unwrap());
    assert_eq!(state(e), Some(("Transmitting".into(), None)));

    // Held for a person: never, however long it waits.
    let f = queued(&store, account, 1_000);
    store
        .set_send_state(f, SendState::NeedsAttention, Some("dropped"), None, None)
        .unwrap();
    assert!(!store.claim_send(f, i64::MAX).unwrap());
}

fn is_due(store: &Store, account: i64, id: i64) -> bool {
    store
        .due_sends(account, 5_000)
        .unwrap()
        .iter()
        .any(|d| d.id == id)
}

fn is_listed(store: &Store, account: i64, id: i64) -> bool {
    store.outbox(account).unwrap().iter().any(|r| r.id == id)
}

/// Whether the folder with `role` lists `id`, or the conversation holding it:
/// where the person would see the message, if anywhere. Drafts lists
/// messages, the rest conversations.
fn shown_in(store: &Store, role: &str, id: i64) -> bool {
    use petrel_engine::store::{ListView, Sort};
    let key = store.thread_of(id).unwrap().unwrap_or(-id);
    store
        .list_threads(&ListView::Folder(role.into()), 0, 50, Sort::default())
        .unwrap()
        .iter()
        .any(|r| r.id == id || r.thread_id == key)
}

/// Where a queued message sits is not what stops it. Put in the Trash or
/// Spam, or deleted forever, it still goes, and the Outbox still lists it
/// beside the Undo, Edit and Discard that do stop it. A rule that held
/// binned mail back also held back a reply binned with its conversation.
#[test]
fn filing_or_deleting_a_queued_message_does_not_stop_it() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    for kind in [
        ActionKind::Trash,
        ActionKind::Spam,
        ActionKind::DeleteForever,
    ] {
        let id = queued(&store, account, 1_000);
        store
            .apply_message_action(account, id, kind, None, PlacementPolicy::Exclusive)
            .unwrap();
        // Where the action put it: in the bin, or out of every list.
        match kind {
            ActionKind::Trash => assert!(shown_in(&store, "trash", id)),
            ActionKind::Spam => assert!(shown_in(&store, "spam", id)),
            _ => assert!(!shown_in(&store, "trash", id) && !shown_in(&store, "drafts", id)),
        }
        assert!(is_due(&store, account, id), "{kind:?}: still due");
        assert!(is_listed(&store, account, id), "{kind:?}: still listed");
        assert_eq!(
            store.next_due_ms(account).unwrap(),
            Some(1_000),
            "{kind:?}: still woken for"
        );
        assert!(store.claim_send(id, 5_000).unwrap(), "{kind:?}: and sent");
        store.delete_draft(id).unwrap();
    }
}

/// One waiting on a decision still waits on it wherever it is filed: the
/// Outbox lists it with its state, and the rail's "need you" count asks.
#[test]
fn a_held_message_filed_away_still_needs_a_decision() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let attention = || {
        store
            .view_counts(&Default::default())
            .unwrap()
            .into_iter()
            .find(|(k, _)| k == "outbox:attention")
            .map(|(_, n)| n)
            .unwrap_or(0)
    };
    for kind in [
        ActionKind::Trash,
        ActionKind::Spam,
        ActionKind::DeleteForever,
    ] {
        let id = queued(&store, account, 1_000);
        store
            .set_send_state(id, SendState::NeedsAttention, Some("dropped"), None, None)
            .unwrap();
        store
            .apply_message_action(account, id, kind, None, PlacementPolicy::Exclusive)
            .unwrap();
        assert_eq!(attention(), 1, "{kind:?}: still asks");
        let row = store.outbox(account).unwrap();
        assert_eq!(row.len(), 1, "{kind:?}: still listed");
        assert_eq!(row[0].state, "NeedsAttention");
        assert!(!is_due(&store, account, id), "{kind:?}: and still held");
        store.delete_draft(id).unwrap();
    }
}

/// A reply in a conversation, as one written for longer than the push's
/// pause arrives: its draft reached the server, and the copy coming back
/// through Drafts threaded the row into the conversation.
fn reply_in_a_conversation(
    store: &mut Store,
    blobs: &petrel_engine::blob::BlobStore,
    account: i64,
) -> (i64, i64) {
    use petrel_engine::store::DraftEnvelope;
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
    let received = b"From: Sam <sam@example.com>\r\nTo: me@example.com\r\n\
Subject: Plans\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
Message-ID: <m1@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nshall we?\r\n";
    let first = store
        .ingest_raw(blobs, account, Some(inbox), Some(1), received)
        .unwrap()
        .message_id;
    let envelope = DraftEnvelope {
        in_reply_to: Some("<m1@example.com>".into()),
        references: vec!["<m1@example.com>".into()],
        ..Default::default()
    };
    let reply = store
        .save_draft_full(
            account,
            None,
            "sam@example.com",
            "",
            "Re: Plans",
            "yes",
            "",
            &envelope,
        )
        .unwrap();
    store.set_draft_msgid(reply, "draft-r@example.com").unwrap();
    let copy = b"From: Me <me@example.com>\r\nTo: sam@example.com\r\n\
Subject: Re: Plans\r\nDate: Tue, 18 Aug 2026 14:05:00 +0000\r\n\
Message-ID: <draft-r@example.com>\r\nIn-Reply-To: <m1@example.com>\r\n\
References: <m1@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nyes\r\n";
    let landed = store
        .ingest_raw(blobs, account, Some(drafts), Some(7), copy)
        .unwrap()
        .message_id;
    assert_eq!(landed, reply, "the copy lands on the reply's own row");
    let conversation = store.thread_of(first).unwrap().unwrap();
    assert_eq!(store.thread_of(reply).unwrap(), Some(conversation));
    (reply, conversation)
}

/// Reply, send, and bin the conversation before the reply has gone, or
/// delete it forever from the Trash: the reply still goes. A conversation
/// action takes every member, the queued reply with them.
#[test]
fn a_reply_goes_whatever_happens_to_its_conversation() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    for kinds in [
        &[ActionKind::Trash][..],
        &[ActionKind::Spam],
        &[ActionKind::Trash, ActionKind::DeleteForever],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("p.db")).unwrap();
        let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let (reply, conversation) = reply_in_a_conversation(&mut store, &blobs, account);
        store.schedule_send(reply, Some(1_000)).unwrap();
        for kind in kinds {
            store
                .apply_thread_action(
                    account,
                    conversation,
                    *kind,
                    None,
                    PlacementPolicy::Exclusive,
                )
                .unwrap();
        }
        // The conversation action took the reply with it.
        match kinds.last() {
            Some(ActionKind::Trash) => assert!(store.message_in_role(reply, "trash").unwrap()),
            Some(ActionKind::Spam) => assert!(store.message_in_role(reply, "spam").unwrap()),
            _ => assert!(!shown_in(&store, "trash", reply) && !shown_in(&store, "drafts", reply)),
        }
        assert!(is_due(&store, account, reply), "{kinds:?}: still due");
        assert!(is_listed(&store, account, reply), "{kinds:?}: still listed");
    }
}

/// Send is the last word. A draft put in the Trash, or then deleted forever,
/// from the list while its composer stayed open (a pop-out, say) and then
/// sent from that composer goes. The composer saves it where it is and gives
/// it a time, and where it is holds nothing back.
#[test]
fn a_draft_thrown_away_and_then_sent_goes() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    for kinds in [
        &[ActionKind::Trash][..],
        &[ActionKind::Trash, ActionKind::DeleteForever],
    ] {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let id = store
            .save_draft(account, None, "sam@example.com", "Notes", "first", "")
            .unwrap();
        for kind in kinds {
            store
                .apply_message_action(account, id, *kind, None, PlacementPolicy::Exclusive)
                .unwrap();
        }
        // Send, from the composer that stayed open.
        store
            .save_draft(account, Some(id), "sam@example.com", "Notes", "final", "")
            .unwrap();
        store.schedule_send(id, Some(1_000)).unwrap();
        assert!(is_due(&store, account, id), "{kinds:?}: due");
        assert!(is_listed(&store, account, id), "{kinds:?}: listed");
        assert_eq!(store.load_draft(id).unwrap().body, "final");
    }
}

/// Deleted forever while it waits, a message still goes, so the sweep that
/// reaps deleted mail after its grace period leaves it until it has. A Send
/// Later further off than the grace period was reaped first.
#[test]
fn a_message_deleted_while_queued_outlives_the_grace_period() {
    use petrel_engine::retention::{DEFAULT_GRACE_DAYS, MS_PER_DAY};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let id = queued(&store, account, now + 60 * MS_PER_DAY);
    // How emptying the Trash deletes it, with no action left to wait for.
    store.tombstone_message(id).unwrap();

    let past_grace = now + (DEFAULT_GRACE_DAYS + 1) * MS_PER_DAY;
    let report = store.gc(&blobs, past_grace, DEFAULT_GRACE_DAYS).unwrap();
    assert_eq!(report.messages_purged, 0, "kept while it has a time");
    assert!(is_listed(&store, account, id));

    // Pulled back, it is deleted mail like any other.
    store.unschedule_send(id).unwrap();
    let report = store.gc(&blobs, past_grace, DEFAULT_GRACE_DAYS).unwrap();
    assert_eq!(report.messages_purged, 1, "reaped once it has none");
}

/// Pulled back after it was deleted forever with its conversation, a reply
/// comes back as a new draft in Drafts, with every word and the thread it
/// answers. The deleted row stays with what is queued for its server copy,
/// and the sweep reaps it; the new draft outlives it.
#[test]
fn a_message_deleted_while_queued_comes_back_as_a_new_draft() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_engine::retention::{DEFAULT_GRACE_DAYS, MS_PER_DAY};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let (reply, conversation) = reply_in_a_conversation(&mut store, &blobs, account);
    store.schedule_send(reply, Some(1_000)).unwrap();
    for kind in [ActionKind::Trash, ActionKind::DeleteForever] {
        store
            .apply_thread_action(
                account,
                conversation,
                kind,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }
    assert!(!shown_in(&store, "trash", reply) && !shown_in(&store, "drafts", reply));

    // Z, or Edit in the Outbox.
    let draft = store.pull_back(reply).unwrap();
    assert_ne!(draft, reply, "a new draft, not the deleted row");
    // Asked again, from the other place it can be asked: refused, not the
    // deleted row handed over.
    assert!(store.pull_back(reply).is_err());
    assert!(shown_in(&store, "drafts", draft), "listed in Drafts");
    assert!(!is_listed(&store, account, draft) && !is_due(&store, account, draft));
    let (old, new) = (
        store.load_draft(reply).unwrap(),
        store.load_draft(draft).unwrap(),
    );
    assert_eq!(
        (&new.to, &new.subject, &new.body, &new.envelope),
        (&old.to, &old.subject, &old.body, &old.envelope)
    );
    assert_eq!(
        new.envelope.in_reply_to.as_deref(),
        Some("<m1@example.com>")
    );
    // Nothing queued for the old server copy names the new draft.
    assert!(
        store
            .pending_actions(account)
            .unwrap()
            .iter()
            .all(|a| a.message_id != draft)
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let past_grace = now + (DEFAULT_GRACE_DAYS + 1) * MS_PER_DAY;
    // The deleted row waits for the delete still queued for its copy. With
    // that delivered, the sweep reaps it, and the draft is still there.
    for a in store.pending_actions(account).unwrap() {
        store
            .mark_message_outcome(a.action_id, a.message_id, true)
            .unwrap();
    }
    let report = store.gc(&blobs, past_grace, DEFAULT_GRACE_DAYS).unwrap();
    assert!(report.messages_purged >= 1, "the deleted rows are reaped");
    assert!(shown_in(&store, "drafts", draft), "and the draft is kept");

    // A message that was not deleted is its own draft.
    let live = queued(&store, account, 1_000);
    assert_eq!(store.pull_back(live).unwrap(), live);
    assert!(!is_listed(&store, account, live));
    assert_eq!(store.pull_back(live).unwrap(), live, "and again");
}

/// Send now on a row the Outbox has not redrawn yet, after the message was
/// pulled back: the draft open in the composer is not queued again.
#[test]
fn a_late_send_now_does_not_send_a_message_pulled_back() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store.unschedule_send(id).unwrap();
    assert!(!store.resend_now(id, 5_000).unwrap(), "nothing to send");
    assert!(!is_due(&store, account, id), "not due");
    assert!(!is_listed(&store, account, id), "not back in the Outbox");

    // One still waiting goes at once, as before.
    let waiting = queued(&store, account, 9_000);
    assert!(store.resend_now(waiting, 5_000).unwrap());
    assert!(is_due(&store, account, waiting));
}

#[test]
fn editing_takes_it_out_of_the_outbox_and_keeps_the_text() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(id, SendState::FailedPermanent, Some("550"), None, None)
        .unwrap();

    store.unschedule_send(id).unwrap();

    assert!(store.outbox(account).unwrap().is_empty());
    let draft = store.load_draft(id).unwrap();
    assert_eq!(draft.subject, "Board pack v4");
    assert_eq!(draft.body, "body");
}

#[test]
fn the_message_id_is_kept_across_states() {
    // It is what a later "check again" searches Sent for, so a state change
    // that dropped it would make the message un-checkable.
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(id, SendState::Transmitting, None, None, Some("<m@x>"))
        .unwrap();
    store
        .set_send_state(id, SendState::NeedsAttention, Some("dropped"), None, None)
        .unwrap();
    assert_eq!(
        store.conn_query_send_message_id(id).unwrap().as_deref(),
        Some("<m@x>")
    );
}

#[test]
fn an_interrupted_transmit_is_held_for_a_person() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(id, SendState::Transmitting, None, None, Some("<m@x>"))
        .unwrap();
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());

    assert_eq!(store.recover_interrupted_sends().unwrap(), 1);
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());
    let rows = store.outbox(account).unwrap();
    assert_eq!(rows[0].state, "NeedsAttention");
    assert_eq!(
        store.conn_query_send_message_id(id).unwrap().as_deref(),
        Some("<m@x>")
    );
}

#[test]
fn the_clock_knows_when_to_wake() {
    // The drain is not clock-driven on its own: it runs when a triage action
    // asks or when the sync comes round, and with IDLE the sync sleeps until
    // the server pushes. A scheduled message therefore needs its own alarm,
    // and this is what the alarm reads.
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    assert_eq!(
        store.next_due_ms(account).unwrap(),
        None,
        "empty outbox: nothing to wake for"
    );

    let a = queued(&store, account, 5_000);
    let b = queued(&store, account, 2_000);
    assert_eq!(
        store.next_due_ms(account).unwrap(),
        Some(2_000),
        "the earliest"
    );

    // A retry pushes its own message later; the other is still first.
    store
        .set_send_state(
            b,
            SendState::RetryQueued,
            Some("refused"),
            Some(9_000),
            None,
        )
        .unwrap();
    assert_eq!(store.next_due_ms(account).unwrap(), Some(5_000));

    // A message held for a person has no time, only a person.
    store
        .set_send_state(
            a,
            SendState::NeedsAttention,
            Some("unknown"),
            None,
            Some("<m@x>"),
        )
        .unwrap();
    assert_eq!(
        store.next_due_ms(account).unwrap(),
        Some(9_000),
        "only the retry remains"
    );
}

/// The Outbox's number is what the Outbox lists: messages, not conversations,
/// and one deleted forever while it waits, which still goes. Counted by
/// conversation, two replies waiting in one read as one, and a message
/// deleted while it waited on a decision read "0 waiting · 1 need you" above
/// its own row.
#[test]
fn the_outbox_counts_what_it_lists() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_engine::store::{DraftEnvelope, ListView};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let (first, conversation) = reply_in_a_conversation(&mut store, &blobs, account);

    // A second reply to the same message, threaded the same way: its pushed
    // copy comes back through Drafts.
    let envelope = DraftEnvelope {
        in_reply_to: Some("<m1@example.com>".into()),
        references: vec!["<m1@example.com>".into()],
        ..Default::default()
    };
    let second = store
        .save_draft_full(
            account,
            None,
            "sam@example.com",
            "",
            "Re: Plans",
            "also",
            "",
            &envelope,
        )
        .unwrap();
    store
        .set_draft_msgid(second, "draft-r2@example.com")
        .unwrap();
    let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
    let copy = b"From: Me <me@example.com>\r\nTo: sam@example.com\r\n\
Subject: Re: Plans\r\nDate: Tue, 18 Aug 2026 14:06:00 +0000\r\n\
Message-ID: <draft-r2@example.com>\r\nIn-Reply-To: <m1@example.com>\r\n\
References: <m1@example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nalso\r\n";
    store
        .ingest_raw(&blobs, account, Some(drafts), Some(8), copy)
        .unwrap();
    assert_eq!(store.thread_of(second).unwrap(), Some(conversation));

    store.schedule_send(first, Some(1_000)).unwrap();
    store.schedule_send(second, Some(2_000)).unwrap();
    let count = |store: &Store| store.count_view(&ListView::Outbox, true).unwrap();
    let rail = |store: &Store, key: &str| {
        store
            .view_counts(&Default::default())
            .unwrap()
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, n)| n)
            .unwrap_or(0)
    };
    assert_eq!(count(&store), 2, "two replies in one conversation are two");
    assert_eq!(count(&store), store.outbox(account).unwrap().len() as i64);

    // One waits on a decision, and is deleted forever with its conversation.
    store
        .set_send_state(
            second,
            SendState::NeedsAttention,
            Some("dropped"),
            None,
            None,
        )
        .unwrap();
    for kind in [ActionKind::Trash, ActionKind::DeleteForever] {
        store
            .apply_thread_action(
                account,
                conversation,
                kind,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }
    assert_eq!(store.outbox(account).unwrap().len(), 2, "both still listed");
    assert_eq!(count(&store), 2, "and both still counted");
    assert_eq!(rail(&store, "outbox"), 2);
    assert_eq!(rail(&store, "outbox:attention"), 1);
}
