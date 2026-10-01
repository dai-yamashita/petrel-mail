//! Mail on its way out: what the outbox holds and what can be done about it.

use crate::config::imap_config;
use crate::send::sent_folder_evidence;
use crate::state::{AppState, now_ms};
use crate::sync::drafts::drop_server_draft_using;
use std::sync::Arc;
use tauri::State;

/// The outbox, row by row, with each message's state.
#[tauri::command(async)]
pub fn list_outbox(
    state: State<Arc<AppState>>,
) -> Result<Vec<petrel_engine::store::OutboxRow>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.outbox(account).map_err(|e| e.to_string())
}

/// "Send now", "Try now", "Send anyway". The person has looked and decided,
/// which is the only thing that may move a message out of `NeedsAttention` —
/// so this is also the one place that does.
///
/// A message already on the wire is already going now, and is left alone:
/// put back in the queue under the worker, it went a second time. One guard
/// for the look and the change, so the worker cannot claim it in between.
///
/// Says whether the message is going. One pulled back a moment before is
/// not, and the Sending bar's "Sent" waits for this answer.
#[tauri::command(async)]
pub fn outbox_send_now(id: i64, state: State<Arc<AppState>>) -> Result<bool, String> {
    let outcome = {
        let store = state.store()?;
        send_now(&store, id)?
    };
    if outcome == SendNow::Queued {
        // Wake the send worker so "now" means now, not the next drain.
        state.wake_send();
    }
    Ok(outcome != SendNow::PulledBack)
}

#[derive(Debug, PartialEq)]
pub(crate) enum SendNow {
    /// Put back on the queue to go at once.
    Queued,
    /// On the wire already, or sent and gone. A bar behind the worker (a
    /// hidden window's timers are slowed) can be pressed after the message
    /// went.
    Going,
    /// Back in Drafts: pulled back a moment before, by Z or the Outbox.
    PulledBack,
}

/// What `outbox_send_now` does, under the caller's guard.
pub(crate) fn send_now(store: &petrel_engine::store::Store, id: i64) -> Result<SendNow, String> {
    if store
        .account_of_message(id)
        .map_err(|e| e.to_string())?
        .is_none()
        || outbox_state(store, id)?.as_deref() == Some("Transmitting")
    {
        return Ok(SendNow::Going);
    }
    Ok(
        if store.resend_now(id, now_ms()).map_err(|e| e.to_string())? {
            SendNow::Queued
        } else {
            SendNow::PulledBack
        },
    )
}

/// "Edit": back to Drafts with the text intact, out of the queue.
///
/// Refused while the message is actually on the wire. `unschedule_send`
/// clears the schedule whatever state the row is in, so pressing Edit
/// during the second the SMTP conversation takes cleared the row from under
/// the send worker — which then finished, wrote its outcome to a row that no
/// longer had a schedule, and left a message that had been sent sitting in
/// Drafts as if it had not.
///
/// Returns the draft to open: the message itself, or a new draft with its
/// words when it was deleted forever while it waited (`Store::pull_back`).
#[tauri::command(async)]
pub fn outbox_edit(id: i64, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    pull_back(&store, id)
}

/// What `outbox_edit` does, under the caller's guard.
pub(crate) fn pull_back(store: &petrel_engine::store::Store, id: i64) -> Result<i64, String> {
    refuse_while_transmitting(store, id)?;
    refuse_when_gone(store, id)?;
    // The interface matches the store's words for a second pull-back of a
    // deleted message ("already back in Drafts") as it does the ones below.
    store.pull_back(id).map_err(|e| e.to_string())
}

/// An error for a message that is gone: sent while the person reached for
/// Undo or Discard, or discarded already. Pulling it back reported success
/// over a row that was not there, and Undo went on to open a draft that did
/// not exist; with its id taken by newer mail, it opened that instead (see
/// `Store::is_own_outgoing`). One already back in Drafts is still here, and
/// pulling it back again is the same success it always was.
pub(crate) fn refuse_when_gone(store: &petrel_engine::store::Store, id: i64) -> Result<(), String> {
    if !store.is_own_outgoing(id).map_err(|e| e.to_string())? {
        return Err("that message is no longer in the outbox".into());
    }
    Ok(())
}

/// An error for a message on the wire, which nothing can pull back: Edit and
/// Discard say so rather than act on it.
pub(crate) fn refuse_while_transmitting(
    store: &petrel_engine::store::Store,
    id: i64,
) -> Result<(), String> {
    if outbox_state(store, id)?.as_deref() == Some("Transmitting") {
        // The interface matches these words to say them in the person's
        // language (lib/outbox-refusal.ts), as it does the ones below.
        return Err("that message is being sent right now".into());
    }
    Ok(())
}

/// The state of one queued message, read from the row itself — never through
/// the active account, which may be a different mailbox by the time a queued
/// message is touched.
fn outbox_state(store: &petrel_engine::store::Store, id: i64) -> Result<Option<String>, String> {
    store.queued_state(id).map_err(|e| e.to_string())
}

/// "Check again" for a message whose outcome is unknown: look in Sent once
/// more and resolve it if the evidence is now there. Never sends.
#[tauri::command]
pub async fn outbox_check(id: i64, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    check_in_sent(state.inner(), id).await
}

/// `outbox_check`, for any state.
async fn check_in_sent(state: &Arc<AppState>, id: i64) -> Result<String, String> {
    use petrel_engine::outbox::{AttemptOutcome, SendState, reconcile};
    let (account, message_id) = {
        let store = state.store()?;
        // The row's own account. Asked of the active one, a message queued
        // in the other mailbox reported that it was no longer in the outbox.
        let account = store
            .account_of_message(id)
            .map_err(|e| e.to_string())?
            .ok_or("that message is no longer here")?;
        let row = store
            .outbox(account)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("that message is no longer in the outbox")?;
        let mid: Option<String> = store
            .conn_query_send_message_id(id)
            .map_err(|e| e.to_string())?;
        (account, mid.filter(|_| row.state == "NeedsAttention"))
    };
    let Some(mid) = message_id else {
        return Ok("Indeterminate".into());
    };
    // Signed out, Sent cannot be looked in: asking was one more sign-in with
    // the refused password, and the answer "cannot reach the server" besides.
    if state.signin(account).is_some() {
        return Err(crate::signin::SIGN_IN_FIRST.into());
    }
    let cfg = imap_config(state, account).ok_or("no account is configured")?;
    let evidence = sent_folder_evidence(state, &cfg, account, &mid).await;
    let next = reconcile(AttemptOutcome::UnknownAfterTransmit, evidence);
    let store = state.store()?;
    match next {
        SendState::Sent => {
            drop_server_draft_using(state, &store, id);
            let _ = store.delete_draft(id);
        }
        SendState::RetryQueued => {
            let _ = store.resend_now(id, now_ms());
            state.wake_send();
        }
        _ => {}
    }
    Ok(format!("{next:?}"))
}

#[cfg(test)]
mod signed_out_tests {
    use super::check_in_sent;
    use crate::scripted_imap::{Srv, serve};
    use crate::signin::test_support::{servers_at, unkeyed_account};
    use crate::signin::{SIGN_IN_FIRST, SignIn};
    use crate::state::test_state;
    use petrel_engine::outbox::SendState;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    /// The second UI review's finding 8. Check again, on a message whose
    /// outcome is unknown, signed in to look in Sent whatever the account's
    /// state: with the password refused, one more refused sign-in, and "still
    /// cannot reach the server" for an answer.
    #[test]
    fn check_again_asks_nothing_of_a_signed_out_server() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let srv = Srv::new("UIDPLUS", &[("INBOX", ""), ("Sent", "\\Sent")], &[]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            servers_at(&state, account, port);
            crate::config::remember_password(account, "revoked");
            let id = {
                let store = state.store.lock().unwrap();
                let id = store
                    .save_draft(account, None, "dana@example.com", "Board pack", "words", "")
                    .unwrap();
                store.schedule_send(id, Some(1_000)).unwrap();
                store
                    .set_send_state(
                        id,
                        SendState::NeedsAttention,
                        Some("connection closed after DATA"),
                        None,
                        Some("<board@example.com>"),
                    )
                    .unwrap();
                id
            };
            state.set_signin(account, SignIn::Refused);
            let result = check_in_sent(&state, id).await;
            crate::config::forget_password(account);
            assert_eq!(result, Err(SIGN_IN_FIRST.to_string()));
            assert_eq!(srv.conns.load(Ordering::SeqCst), 0, "asked the server");
        });
    }
}

#[cfg(test)]
mod outbox_tests {
    use super::{
        SendNow, outbox_state, pull_back, refuse_when_gone, refuse_while_transmitting, send_now,
    };
    use petrel_engine::outbox::SendState;
    use petrel_engine::store::{AccountServers, Store};

    /// Edit and Send-now read the row's state, and the row belongs to the
    /// account that wrote it rather than to whichever one is on screen.
    #[test]
    fn a_transmitting_row_is_recognised_and_found_by_its_own_account() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .add_account("imap", "a@example.com", "A", &AccountServers::default())
            .unwrap();
        let second = store
            .add_account("imap", "b@example.com", "B", &AccountServers::default())
            .unwrap();
        // The rail shows the first account; the draft is the second's.
        store.set_active_account(first).unwrap();
        let draft = store
            .save_draft(second, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(draft, Some(1_771_803_000_000)).unwrap();

        assert_eq!(
            outbox_state(&store, draft).unwrap().as_deref(),
            Some("RetryQueued"),
            "a queued row is found through the account that owns it"
        );

        store
            .set_send_state(draft, SendState::Transmitting, None, None, None)
            .unwrap();
        assert_eq!(
            outbox_state(&store, draft).unwrap().as_deref(),
            Some("Transmitting"),
            "and Edit has something to refuse on"
        );

        assert_eq!(outbox_state(&store, 9_999).unwrap(), None);
    }

    /// Edit and Discard refuse a message on the wire and nothing else: a
    /// queued one, a plain draft and one already gone are theirs to act on
    /// (or to report missing) as before.
    #[test]
    fn only_a_message_on_the_wire_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let queued = store
            .save_draft(account, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(queued, Some(1_000)).unwrap();
        let draft = store
            .save_draft(account, None, "someone@example.com", "Draft", "body", "")
            .unwrap();

        assert!(refuse_while_transmitting(&store, queued).is_ok());
        assert!(refuse_while_transmitting(&store, draft).is_ok());
        assert!(refuse_while_transmitting(&store, 9_999).is_ok());

        assert!(store.claim_send(queued, 5_000).unwrap());
        assert_eq!(
            refuse_while_transmitting(&store, queued).unwrap_err(),
            "that message is being sent right now"
        );
    }

    /// Put in the Trash while it was on the wire: the Outbox still lists it
    /// as going, and Edit and Discard still refuse.
    #[test]
    fn a_message_binned_on_the_wire_is_still_refused() {
        use petrel_engine::actions::{ActionKind, PlacementPolicy};
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let queued = store
            .save_draft(account, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(queued, Some(1_000)).unwrap();
        assert!(store.claim_send(queued, 5_000).unwrap());
        store
            .apply_message_action(
                account,
                queued,
                ActionKind::Trash,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        let rows = store.outbox(account).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, "Transmitting");
        assert_eq!(
            refuse_while_transmitting(&store, queued).unwrap_err(),
            "that message is being sent right now"
        );
    }

    /// Undo that loses the race with the send: the message went, its row with
    /// it, and Edit says so instead of reporting a pull-back.
    #[test]
    fn a_message_already_gone_cannot_be_pulled_back() {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let queued = store
            .save_draft(account, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(queued, Some(1_000)).unwrap();
        assert!(refuse_when_gone(&store, queued).is_ok());

        // Already pulled back once, from the outbox or the bar: still here.
        store.unschedule_send(queued).unwrap();
        assert!(refuse_when_gone(&store, queued).is_ok());

        store.delete_draft(queued).unwrap();
        assert_eq!(
            refuse_when_gone(&store, queued).unwrap_err(),
            "that message is no longer in the outbox"
        );
    }

    /// Received mail is never taken for an outgoing message: Edit and
    /// Discard act only on a message still queued or a draft saved here. A
    /// freed id is not given out again since step 28; this guard held before
    /// it, and still does for an id freed before the upgrade.
    #[test]
    fn received_mail_is_not_taken_for_an_outgoing_message() {
        use petrel_engine::store::NewMessage;
        let mut store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let queued = store
            .save_draft(account, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(queued, Some(1_000)).unwrap();
        // The worker, once the message went.
        store.delete_draft(queued).unwrap();
        let arrived = store
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: 2,
                from_addr: "stranger@example.com".into(),
                from_display: "Stranger".into(),
                to_addr: "me@example.com".into(),
                subject: "Your invoice".into(),
                body_text: "pay".into(),
            }])
            .unwrap()[0];
        assert_ne!(arrived, queued, "a freed id is not given out again");
        for id in [queued, arrived] {
            assert_eq!(
                refuse_when_gone(&store, id).unwrap_err(),
                "that message is no longer in the outbox"
            );
        }
    }

    /// Pulled back after it was deleted forever while it waited: one new
    /// draft with its words, however often the pull-back is asked for. The
    /// Outbox row stays up until it redraws, and Z can race it. The second
    /// ask is refused rather than handed the deleted row, which no list
    /// shows and the sweep reaps.
    #[test]
    fn pulling_back_a_deleted_message_makes_one_draft() {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let queued = store
            .save_draft(account, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(queued, Some(1_000)).unwrap();
        store.tombstone_message(queued).unwrap();

        let draft = pull_back(&store, queued).unwrap();
        assert_ne!(draft, queued);
        assert_eq!(store.load_draft(draft).unwrap().body, "body");
        assert_eq!(
            pull_back(&store, queued).unwrap_err(),
            "that message is already back in Drafts"
        );
        let drafts = store
            .list_threads(
                &petrel_engine::store::ListView::Folder("drafts".into()),
                0,
                50,
                petrel_engine::store::Sort::default(),
            )
            .unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].id, draft);

        // One that was not deleted opens as itself, as often as asked.
        let live = store
            .save_draft(account, None, "someone@example.com", "Hey", "more", "")
            .unwrap();
        store.schedule_send(live, Some(1_000)).unwrap();
        assert_eq!(pull_back(&store, live).unwrap(), live);
        assert_eq!(pull_back(&store, live).unwrap(), live);
    }

    /// Send now says whether the message is going, and "Sent" waits for it:
    /// not for one pulled back a moment before, which stays a draft.
    #[test]
    fn send_now_says_whether_the_message_is_going() {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let draft = |subject: &str| {
            let id = store
                .save_draft(account, None, "someone@example.com", subject, "body", "")
                .unwrap();
            store.schedule_send(id, Some(i64::MAX / 2)).unwrap();
            id
        };

        let waiting = draft("waiting");
        assert_eq!(send_now(&store, waiting).unwrap(), SendNow::Queued);
        assert_eq!(
            outbox_state(&store, waiting).unwrap().as_deref(),
            Some("RetryQueued")
        );

        let on_the_wire = draft("on the wire");
        assert!(store.claim_send(on_the_wire, i64::MAX).unwrap());
        assert_eq!(send_now(&store, on_the_wire).unwrap(), SendNow::Going);

        let pulled_back = draft("pulled back");
        store.unschedule_send(pulled_back).unwrap();
        assert_eq!(send_now(&store, pulled_back).unwrap(), SendNow::PulledBack);
        assert_eq!(
            outbox_state(&store, pulled_back).unwrap(),
            None,
            "still a draft"
        );

        let sent = draft("sent");
        store.delete_draft(sent).unwrap();
        assert_eq!(send_now(&store, sent).unwrap(), SendNow::Going);
    }
}
