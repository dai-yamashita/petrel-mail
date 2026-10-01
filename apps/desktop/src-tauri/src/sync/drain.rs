//! Local changes reaching the server: the drain worker and the pass it runs.

use crate::diag::log_sync;
use crate::state::{AppState, stopped, unless_stopped};
use petrel_engine::store::Store;
use petrel_providers::imap::ImapConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Whether an action is still waiting to go, asked immediately before it
/// does.
///
/// The drain reads the queue once and then works through it, and a pass over
/// a real backlog takes seconds. An undo that lands meanwhile pulls the row
/// out of the queue and puts the message back — and the drain, holding its
/// stale list, delivered the cancelled change anyway about a second later.
/// So every item is asked again at the moment of delivery, and anything that
/// has left the queue is left alone.
fn still_queued(store: &Store, action_id: i64) -> bool {
    matches!(store.action_state(action_id), Ok(Some(ref s)) if s == "queued")
}

/// How many times a permanently-refused action is asked again before it is
/// put out of the queue.
///
/// Not once: a single odd answer should not discard work somebody did, and a
/// folder deleted by accident can come back. Not indefinitely: that is the
/// behaviour this replaces, where one action failed on every cycle for days
/// and the change it carried was never made and never abandoned either.
const GIVE_UP_AFTER: i64 = 5;

/// Same kind, same folder, already-resolved UIDs: one STORE instead of one
/// connection per message.
fn flag_op(kind: &petrel_engine::actions::ActionKind) -> Option<(&'static str, bool)> {
    use petrel_engine::actions::ActionKind;
    match kind {
        ActionKind::MarkRead => Some(("\\Seen", true)),
        ActionKind::MarkUnread => Some(("\\Seen", false)),
        ActionKind::Star => Some(("\\Flagged", true)),
        ActionKind::Unstar => Some(("\\Flagged", false)),
        _ => None,
    }
}

/// How many following rows share this flag STORE.
///
/// Stops at the first row that would need a search or a different folder —
/// those must stay in order relative to this action.
fn consecutive_flag_uids(
    pending: &[petrel_engine::store::PendingAction],
    start: usize,
    kind_json: &str,
    folder: &str,
    first_uid: u32,
    max: usize,
) -> Vec<(usize, i64, u32)> {
    let mut out = vec![(start, pending[start].action_id, first_uid)];
    let mut i = start + 1;
    while out.len() < max && i < pending.len() {
        let row = &pending[i];
        if row.kind_json != kind_json {
            break;
        }
        let later = if row.folder_path.is_empty() {
            "INBOX"
        } else {
            row.folder_path.as_str()
        };
        if later != folder {
            break;
        }
        let Some(uid) = row.uid else { break };
        out.push((i, row.action_id, uid));
        i += 1;
    }
    out
}

/// The size of a message as the store holds it: the bytes the server sent,
/// read from its blob. `None` when there is no blob to read.
fn stored_size(state: &AppState, message_id: i64) -> Option<u32> {
    let hash = state.store.lock().ok()?.blob_hash_for(message_id).ok()??;
    let raw = state.blobs.read(&hash).ok()?;
    u32::try_from(raw.len()).ok()
}

/// Whether the one server copy under a row's Message-ID is the row's own
/// message: read in full and compared by content identity, so a copy that
/// differs only by what transport added counts, and a different message
/// under the same id does not.
async fn copy_is_this_message(
    state: &AppState,
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    message_id: i64,
) -> std::result::Result<bool, petrel_providers::imap::ImapError> {
    let mut fetched: Option<Vec<u8>> = None;
    petrel_providers::imap::fetch_uids_each(cfg, folder, &[uid], |_, _, raw| {
        fetched = Some(raw.to_vec());
    })
    .await?;
    let Some(raw) = fetched else {
        return Ok(false);
    };
    Ok(state
        .store
        .lock()
        .ok()
        .and_then(|s| s.is_same_message(&state.blobs, message_id, &raw).ok())
        .unwrap_or(false))
}

/// Whether the server's refusal is one that asking again cannot fix.
///
/// A mailbox that does not exist will not start existing on the two hundredth
/// try — that is a folder renamed or deleted out from under a queued change.
/// A broken pipe is the opposite kind of failure and must not count.
///
/// Anything unrecognised is treated as temporary, so a wrong guess costs a
/// retry rather than somebody's change.
fn permanent_refusal(error: &str) -> bool {
    let low = error.to_lowercase();
    low.contains("nonexistent")
        || low.contains("trycreate")
        || low.contains("doesn't exist")
        || low.contains("does not exist")
        || low.contains("no such mailbox")
        || low.contains("unknown mailbox")
}

/// Delivers queued triage as soon as there is any, rather than when the next
/// sync happens to run.
///
/// Debounced, because triage comes in bursts: working down an inbox is a run of
/// archives a few hundred milliseconds apart, and one connection carrying all
/// of them beats one connection each. A second of latency is invisible to the
/// person doing it and saves a login per keystroke.
pub(crate) fn spawn_drain_worker(
    state: Arc<AppState>,
    account: i64,
    cfg: ImapConfig,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let signals = state.outbox_signals(account);
    tauri::async_runtime::spawn(async move {
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                _ = signals.drain.notified() => {}
                _ = stopped(&mut stop) => break,
            }
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            // While the server refuses the password, the queue keeps what was
            // asked for and goes once signing in works again. Asking anyway
            // spent one refused sign-in per archive, which is how fail2ban
            // counts its way to a ban.
            if !crate::signin::wait_for_signin(&state, account, &mut stop).await {
                break;
            }
            // Unknown capabilities — a launch with no network never probed —
            // read as false here, and the provider asks the server on the
            // connection that does the work (`confirm_move_caps`). Gmail is
            // known by its host as well, so a tag made before the first
            // probe answers still goes as a label rather than a keyword.
            let caps = state.caps(account);
            let has_move = caps.has_move;
            let has_uidplus = caps.has_uidplus;
            let is_gmail = caps.is_gmail || crate::sync::account_is_gmail(&cfg);
            // The overlap guard is one flag across every account, and losing
            // to it must not lose the wake-up: a notification arriving while
            // another account drains used to be consumed and dropped, leaving
            // this account's queue waiting for the next unrelated signal.
            // The loser retries until the guard is free.
            //
            // Under the worker's own switch, each try and each wait. Sign in
            // again stops this worker and starts another with the new
            // password; a pass that carried on with the old one — waiting
            // out another account's drain, or mid-way through its own —
            // delivered the queue with a password the server had stopped
            // taking, and its refusal stood the new run down for an hour.
            let mine = stop.clone();
            loop {
                let Some(held_floor) = unless_stopped(
                    &mut stop,
                    drain_actions(
                        Arc::clone(&state),
                        account,
                        cfg.clone(),
                        has_move,
                        has_uidplus,
                        is_gmail,
                        &mine,
                    ),
                )
                .await
                else {
                    break;
                };
                if held_floor {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                    _ = stopped(&mut stop) => break,
                }
            }
            if *stop.borrow() {
                break;
            }
            // Triage is done. If anything became due while we were in IMAP,
            // the send worker takes it — we must not await send_due here or
            // the next Send now waits on the next backlog the same way.
            state.nudge_send(account);
        }
        log_sync(&format!("account {account}: drain worker stopped"));
    });
}

/// Clears the draining flag however the drain ends, including on an early
/// return or a panic — a flag left set would silently stop every later drain.
struct DrainGuard(Arc<AppState>);

impl Drop for DrainGuard {
    fn drop(&mut self) {
        self.0.draining.store(false, Ordering::SeqCst);
    }
}

/// Delivers queued triage to the server.
///
/// This is the second half of the optimistic model. Everything the user does is
/// applied locally at once and written to a queue; until something drains that
/// queue, archiving a conversation in Petrel means nothing to anyone else, and
/// the next resync quietly puts it back.
///
/// Order matters and is preserved: two actions on the same message have to
/// arrive the way the user performed them, or the later one loses. A failure
/// stops that action rather than the drain — one unreachable message should not
/// strand every other change behind it — and leaves it queued to retry, because
/// a change that never reached the server is not one to discard.
pub(crate) async fn drain_actions(
    state: Arc<AppState>,
    account: i64,
    cfg: ImapConfig,
    has_move: bool,
    has_uidplus: bool,
    // Whether this account's tags are Gmail labels. Passed in with the other
    // capabilities rather than sniffed here: the probe already worked it out,
    // and two places deciding what a server is would eventually disagree.
    looks_like_gmail: bool,
    // The switch of the worker this pass belongs to. Stopped — Sign in again
    // started the account over, or it was removed — the pass goes no
    // further between actions, and a refusal it meets is not written over
    // the run that replaced it (`AppState::refused_by`).
    stop: &tokio::sync::watch::Receiver<bool>,
    // Whether this call held the floor: false only when another drain was
    // already running, so the caller knows to come back rather than treat
    // the queue as attended to.
) -> bool {
    use petrel_engine::actions::ActionKind;

    // Refuse to overlap. compare_exchange rather than a load-then-store: two
    // tasks arriving together would both see `false` and both proceed.
    if state
        .draining
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    let _guard = DrainGuard(Arc::clone(&state));

    let pending = match state.store.lock().map(|s| s.pending_actions(account)) {
        Ok(Ok(p)) => p,
        _ => return true,
    };
    if pending.is_empty() {
        return true;
    }
    log_sync(&format!("draining {} queued change(s)", pending.len()));

    let mut delivered = 0usize;
    let mut stuck = 0usize;
    let mut undeliverable = 0usize;
    // An action can carry several messages and arrives here once per message,
    // so a failing thread of ten would otherwise spend ten tries in a single
    // cycle. One count per action per pass.
    let mut counted: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut batched: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // Where the pass stopped because the account cannot sign in: the server
    // refused the password here, or another worker met the refusal first.
    let mut refused_at: Option<usize> = None;
    // Where the pass stopped because its worker was told to stop.
    let mut stopped_at: Option<usize> = None;
    'items: for (idx, item) in pending.iter().enumerate() {
        if batched.contains(&idx) {
            continue;
        }
        // Told to stop: what is left waits for the run that replaced this
        // one, with its attempts untouched.
        if *stop.borrow() {
            stopped_at = Some(idx);
            break 'items;
        }
        // Stood down meanwhile, by a refusal the IDLE watcher or backfill
        // met: the next action would only be refused again.
        if state.signin(account).is_some() {
            refused_at = Some(idx);
            break 'items;
        }
        let Ok(kind) = serde_json::from_str::<ActionKind>(&item.kind_json) else {
            continue;
        };
        // Undone, or settled by another path, since the list was read: not
        // ours to deliver any more.
        let queued = state
            .store
            .lock()
            .map(|s| still_queued(&s, item.action_id))
            .unwrap_or(false);
        if !queued {
            continue;
        }
        // No UID survived locally — a move destroyed the placement that held
        // it, or a UIDVALIDITY reset declared the number a lie. Before giving
        // up, ask the server the question recovery asks, scoped to this one
        // message: which of your numbers carries this Message-ID? The
        // candidates are the folders the store last saw the message in, and
        // a hit heals the placement that lost its number.
        //
        // One copy that is this message, or none. The last of several hits
        // used to win, and a search for `5@host` names `<15@host>` too: an
        // archive, or a Delete forever, went to another message. A copy
        // carrying the exact id is this message only at the size the store
        // holds — another message can carry the same id, a forgery of an
        // invoice among them — and two such copies are not told apart by
        // guessing. The key is the store's own: one with a suffix the store
        // added matches nothing on the server, so a forgery's row is never
        // healed onto the message it imitates.
        let key = item.msgid.as_deref().filter(|m| !m.is_empty());
        let mut resolved = item.uid.map(|u| (u, item.folder_path.clone()));
        let mut search_failed = false;
        if resolved.is_none()
            && let Some(key) = key
        {
            let size = stored_size(&state, item.message_id);
            // One row stands for every copy of the same message (sent to
            // yourself, delivered under an alias) and holds one copy's bytes,
            // so another copy's size is not a fair test of it. Where no other
            // row carries the Message-ID, a copy of another size is read and
            // known by its content instead: refused for its size, the copy in
            // INBOX of a message sent to yourself was passed over for the one
            // in Sent, and a Delete forever left it there to come back
            // (docs/25 review). A different message under the id still has
            // another identity, and is still refused.
            let alone = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.carries_its_message_id_alone(item.message_id).ok())
                .unwrap_or(false);
            for path in &item.candidate_paths {
                match petrel_providers::imap::copies_of_message_id(&cfg, path, key).await {
                    Ok(copies) => match copies.as_slice() {
                        [] => {}
                        [copy] => {
                            let fits = copy.size.is_some() && copy.size == size;
                            let ours = if fits || !alone {
                                Ok(fits)
                            } else {
                                copy_is_this_message(&state, &cfg, path, copy.uid, item.message_id)
                                    .await
                            };
                            match ours {
                                Ok(true) => {
                                    let u = copy.uid;
                                    log_sync(&format!(
                                        "action {}: {path} answers to the Message-ID with UID {u}",
                                        item.action_id
                                    ));
                                    if let Ok(store) = state.store.lock() {
                                        let _ = store.heal_placement_uid(
                                            item.message_id,
                                            account,
                                            path,
                                            u,
                                        );
                                    }
                                    resolved = Some((u, path.clone()));
                                    break;
                                }
                                Ok(false) => log_sync(&format!(
                                    "action {}: {path} holds a different message under the Message-ID; not healing",
                                    item.action_id
                                )),
                                Err(e) if signed_out(&state, account, stop, &e) => {
                                    refused_at = Some(idx);
                                    break 'items;
                                }
                                // Not read, so not known: the copy here may well
                                // be this message's, so no other folder's copy
                                // is taken in its place. The next drain asks
                                // again.
                                Err(e) => {
                                    search_failed = true;
                                    log_sync(&format!(
                                        "action {}: reading the copy in {path} failed: {e}",
                                        item.action_id
                                    ));
                                    break;
                                }
                            }
                        }
                        several => log_sync(&format!(
                            "action {}: {path} holds {} messages under the Message-ID; not guessing",
                            item.action_id,
                            several.len()
                        )),
                    },
                    Err(e) if signed_out(&state, account, stop, &e) => {
                        refused_at = Some(idx);
                        break 'items;
                    }
                    // A failed search says nothing about the message — the
                    // network did not answer, so the action stays queued and
                    // the next drain asks again.
                    Err(e) => {
                        search_failed = true;
                        log_sync(&format!(
                            "action {}: search of {path} failed: {e}",
                            item.action_id
                        ));
                    }
                }
            }
        }
        let Some((uid, folder_path)) = resolved else {
            if search_failed {
                stuck += 1;
                continue;
            }
            // Every folder we know of answered, and none holds it — or it has
            // no Message-ID to ask about. There is no server copy for this
            // message to change, and retrying cannot learn more. The message
            // leaves the queue; the action settles once every message it
            // carries has an outcome, and reads as undeliverable only if none
            // of them reached the server.
            undeliverable += 1;
            if let Ok(store) = state.store.lock() {
                let _ = store.mark_message_outcome(item.action_id, item.message_id, false);
            }
            log_sync(&format!(
                "action {}: no server copy answers to one of its messages; dropped",
                item.action_id
            ));
            continue;
        };
        let folder = if folder_path.is_empty() {
            "INBOX".to_string()
        } else {
            folder_path
        };

        let result = match kind {
            ActionKind::MarkRead
            | ActionKind::MarkUnread
            | ActionKind::Star
            | ActionKind::Unstar => {
                let (flag, add) = flag_op(&kind).expect("flag kind");
                let mut group = consecutive_flag_uids(
                    &pending,
                    idx,
                    &item.kind_json,
                    &folder,
                    uid,
                    petrel_providers::imap::STORE_FLAG_BATCH,
                );
                // The same question for every row riding along in the batch:
                // one STORE must not carry a change somebody has undone.
                if let Ok(store) = state.store.lock() {
                    group.retain(|(i, action_id, _)| *i == idx || still_queued(&store, *action_id));
                }
                let uids: Vec<u32> = group.iter().map(|(_, _, u)| *u).collect();
                let result =
                    petrel_providers::imap::store_flags(&cfg, &folder, &uids, flag, add).await;
                if result.is_ok() {
                    for (i, action_id, _) in &group {
                        if *i != idx {
                            batched.insert(*i);
                            delivered += 1;
                            if let Ok(store) = state.store.lock() {
                                let _ = store.mark_message_outcome(
                                    *action_id,
                                    pending[*i].message_id,
                                    true,
                                );
                            }
                        }
                    }
                }
                result
            }
            ActionKind::Archive | ActionKind::Trash | ActionKind::Spam | ActionKind::Move => {
                // The local move has already happened, so the destination is
                // wherever the message now sits, other than where this row
                // says the server still holds it. A move to a named folder
                // lands in the folder the action names: on a labels provider
                // the message also sits in All Mail, and "the first folder"
                // could pick that and archive it instead.
                let target = matches!(kind, ActionKind::Move)
                    .then(|| serde_json::from_str::<serde_json::Value>(&item.payload_json).ok())
                    .flatten()
                    .and_then(|p| p.get("target").and_then(|t| t.as_i64()));
                let dest = state.store.lock().ok().and_then(|s| {
                    target
                        .and_then(|fid| s.folder_path(fid).ok().flatten())
                        .or_else(|| {
                            s.folders_of(item.message_id).ok().and_then(|fids| {
                                fids.into_iter()
                                    .filter_map(|fid| s.folder_path(fid).ok().flatten())
                                    .find(|p| *p != folder)
                            })
                        })
                });
                match dest {
                    Some(to) if to != folder => {
                        // Read only where a move can take the careful path:
                        // the server's MOVE needs no check.
                        // The stored size is a test only where another row
                        // carries the Message-ID. Alone, the row may hold
                        // another copy's bytes than the one being moved (a
                        // message sent to yourself), and the source's own
                        // size and arrival time already say which landed copy
                        // is its: refused for the stored size, a COPY that
                        // landed was made again (docs/25 review).
                        let stored = key.map(|message_id| petrel_providers::imap::Stored {
                            message_id,
                            size: if has_move
                                || state
                                    .store
                                    .lock()
                                    .ok()
                                    .and_then(|s| {
                                        s.carries_its_message_id_alone(item.message_id).ok()
                                    })
                                    .unwrap_or(false)
                            {
                                None
                            } else {
                                stored_size(&state, item.message_id)
                            },
                        });
                        let moved = match petrel_providers::imap::move_uid(
                            &cfg,
                            &folder,
                            uid,
                            &to,
                            has_move,
                            has_uidplus,
                            stored,
                        )
                        .await
                        {
                            // A destination the server has never heard of —
                            // the folder was made here moments ago. Create it
                            // and try once more; servers signal this as
                            // TRYCREATE but not all of them say the word. Not
                            // over a refused password: that is a second
                            // refused sign-in for nothing.
                            Err(e) if !e.is_sign_in_refused() => {
                                log_sync(&format!(
                                    "move to {to} failed ({e}); creating and retrying"
                                ));
                                match petrel_providers::imap::create_folder(&cfg, &to).await {
                                    Ok(()) => {
                                        petrel_providers::imap::move_uid(
                                            &cfg,
                                            &folder,
                                            uid,
                                            &to,
                                            has_move,
                                            has_uidplus,
                                            stored,
                                        )
                                        .await
                                    }
                                    Err(_) => Err(e),
                                }
                            }
                            ok => ok,
                        };
                        // The server confirmed the move: the source placement
                        // goes now, from here, not from whatever sync pass
                        // next looks. A fetch that raced the delivery may
                        // have re-added it, and a placement the server no
                        // longer backs is how a conversation ends up haunting
                        // both its folder and the inbox.
                        if let Ok(false) = moved {
                            // Copied and flagged, not expunged: no UIDPLUS, and a
                            // bare EXPUNGE would commit other clients' deletions.
                            log_sync(&format!(
                                "{folder}: moved by copy; the source copy is flagged deleted, not expunged (no UIDPLUS)"
                            ));
                        }
                        if moved.is_ok()
                            && let Ok(store) = state.store.lock()
                        {
                            let _ = store.remove_placement(item.message_id, account, &folder);
                        }
                        moved.map(|_| ())
                    }
                    // Already where it belongs, or nowhere to send it.
                    _ => Ok(()),
                }
            }
            // Local-only, so they should never have been queued at all — the
            // store marks them 'local' and this drain only reads 'queued'.
            // Handled here so adding a local action later cannot silently fall
            // into the tag branch and be counted as stuck forever.
            ActionKind::DeleteForever => {
                // The local row is already a tombstone, so its placements are
                // the last record of where the server copy lives. Expunge from
                // the folder it was queued against.
                match petrel_providers::imap::expunge_uid(&cfg, &folder, uid, has_uidplus).await {
                    // Marked \\Deleted but not expunged: the server has no
                    // UIDPLUS, and a bare EXPUNGE would have committed every
                    // other pending deletion in the mailbox too. Worth a line
                    // in the log, because the message outlives the gesture.
                    Ok(false) => {
                        log_sync(&format!(
                            "{folder}: marked deleted but not expunged (no UIDPLUS)"
                        ));
                        Ok(())
                    }
                    Ok(true) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            ActionKind::Snooze | ActionKind::Unsnooze => continue,
            // A tag is a Gmail label on Gmail, an IMAP keyword elsewhere.
            // Only the first is wired; the rest stay queued rather than being
            // marked done, so they deliver when keywords land instead of being
            // silently dropped.
            ActionKind::Tag | ActionKind::Untag => {
                // The action names the tag by id, not by name: a tag can be
                // renamed between queueing and delivery, and the action means
                // "this tag" rather than "whatever it was called at the time".
                let target = serde_json::from_str::<serde_json::Value>(&item.payload_json)
                    .ok()
                    .and_then(|p| p.get("target").and_then(|t| t.as_i64()));
                let name = target.and_then(|id| {
                    state
                        .store
                        .lock()
                        .ok()
                        .and_then(|s| s.tag_name(id).ok())
                        .flatten()
                });
                let Some(name) = name else {
                    // The tag was deleted before its action went out. There is
                    // nothing left to name to the server, and retrying forever
                    // would keep a dead action in the queue.
                    if let Ok(store) = state.store.lock() {
                        let _ = store.mark_action_state(item.action_id, "sent");
                    }
                    continue;
                };
                let adding = matches!(kind, ActionKind::Tag);
                if looks_like_gmail {
                    petrel_providers::imap::store_gmail_labels(&cfg, &folder, uid, &name, adding)
                        .await
                } else {
                    // Everywhere else a tag travels as an IMAP keyword, which
                    // Dovecot persists beside the system flags. These actions
                    // used to sit 'queued' forever with nowhere to go.
                    let keyword = petrel_engine::keywords::tag_keyword(&name);
                    petrel_providers::imap::store_flag(&cfg, &folder, uid, &keyword, adding).await
                }
            }
        };

        match result {
            Ok(()) => {
                delivered += 1;
                // This message reached the server. The action settles when
                // its last message does; settling it here, on the first,
                // threw the rest of a conversation's changes away.
                if let Ok(store) = state.store.lock() {
                    let _ = store.mark_message_outcome(item.action_id, item.message_id, true);
                }
            }
            // The server refused the password. Every other action would be
            // another refused sign-in — the drain makes one connection per
            // action, which is how fail2ban counts its way to a ban — so the
            // pass ends here. What is left stays queued with its attempts
            // untouched: the password is at fault, not the actions, and they
            // go once signing in works again.
            Err(e) if signed_out(&state, account, stop, &e) => {
                refused_at = Some(idx);
                break 'items;
            }
            Err(e) => {
                let text = e.to_string();
                // A refusal retrying cannot fix is counted, and after enough
                // of them the action leaves the queue. Anything else — a
                // broken pipe, a sleeping laptop — is not counted at all:
                // the network comes back, and discarding somebody's change
                // because it went away would be the worse bug.
                let spent = permanent_refusal(&text) && counted.insert(item.action_id) && {
                    match state.store.lock() {
                        Ok(store) => {
                            let n = store.record_attempt(item.action_id).unwrap_or(0);
                            if n >= GIVE_UP_AFTER {
                                let _ = store.mark_action_state(item.action_id, "undeliverable");
                                true
                            } else {
                                false
                            }
                        }
                        Err(_) => false,
                    }
                };
                if spent {
                    undeliverable += 1;
                    log_sync(&format!(
                        "action {}: {text}; asked {GIVE_UP_AFTER} times, marked undeliverable",
                        item.action_id
                    ));
                } else {
                    stuck += 1;
                    log_sync(&format!(
                        "action {} could not be delivered: {text}",
                        item.action_id
                    ));
                }
            }
        }
    }
    let mut tail = if undeliverable > 0 {
        format!(", {undeliverable} undeliverable")
    } else {
        String::new()
    };
    if let Some(at) = refused_at {
        let left = (at..pending.len()).filter(|i| !batched.contains(i)).count();
        stuck += left;
        tail.push_str(&format!(
            "; signed out, {left} not tried until it signs in again"
        ));
    }
    if let Some(at) = stopped_at {
        let left = (at..pending.len()).filter(|i| !batched.contains(i)).count();
        stuck += left;
        tail.push_str(&format!("; stopped, {left} left for the next run"));
    }
    log_sync(&format!(
        "drained {delivered} change(s), {stuck} still queued{tail}"
    ));
    true
}

/// Whether an error is the server refusing the password at sign-in, and if
/// so marks the account as needing to sign in again, which every worker
/// waits on — unless this pass's worker has been told to stop, whose
/// refusal belongs to a password already replaced. Told by the error's
/// type, never its words.
fn signed_out(
    state: &AppState,
    account: i64,
    stop: &tokio::sync::watch::Receiver<bool>,
    error: &petrel_providers::imap::ImapError,
) -> bool {
    if !error.is_sign_in_refused() {
        return false;
    }
    if state.refused_by(account, stop) {
        log_sync(&format!(
            "account {account}: the server refused the password"
        ));
    }
    true
}

#[cfg(test)]
mod refusal_tests {
    use super::permanent_refusal;

    #[test]
    fn a_missing_mailbox_is_permanent() {
        // The exact words a real account produced, 112 times.
        assert!(permanent_refusal(
            "imap: no response: code: None, info: Some(\"Mailbox doesn't exist: glassdoor+3022026 (0.002 secs).\")"
        ));
        assert!(permanent_refusal("[NONEXISTENT] Mailbox does not exist"));
        assert!(permanent_refusal("[TRYCREATE] No such mailbox"));
    }

    #[test]
    fn a_network_failure_is_not() {
        // Counting these would discard a change because a laptop slept.
        assert!(!permanent_refusal("imap: io: Broken pipe (os error 32)"));
        assert!(!permanent_refusal("connection reset by peer"));
        assert!(!permanent_refusal("operation timed out"));
    }

    #[test]
    fn anything_unrecognised_is_retried_rather_than_discarded() {
        assert!(!permanent_refusal("server said something new and strange"));
    }
}

#[cfg(test)]
mod recheck_tests {
    use super::still_queued;
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_engine::store::{NewMessage, Store};

    fn store_with_one_message() -> (Store, i64, i64) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let ids = store
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: 1_000,
                from_addr: "a@example.com".into(),
                from_display: "A".into(),
                to_addr: "me@example.com".into(),
                subject: "m".into(),
                body_text: "body".into(),
            }])
            .unwrap();
        let thread = store.thread_of(ids[0]).unwrap().unwrap_or(-ids[0]);
        (store, account, thread)
    }

    /// The race this closes: the drain snapshots the queue, the person
    /// presses undo, and the drain delivers the cancelled change from its
    /// stale list. Asked again at delivery time, an undone action is skipped.
    #[test]
    fn an_action_undone_after_the_queue_was_read_is_not_delivered() {
        let (store, account, thread) = store_with_one_message();
        let receipt = store
            .apply_thread_action(
                account,
                thread,
                ActionKind::Star,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        // What the drain's snapshot would say.
        assert!(still_queued(&store, receipt.action_id));
        assert!(store.undo_action(receipt.action_id).unwrap());
        assert!(
            !still_queued(&store, receipt.action_id),
            "an undone action must read as gone at delivery time"
        );
        // Settled by another path — the same answer.
        let again = store
            .apply_thread_action(
                account,
                thread,
                ActionKind::Star,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        store.mark_action_state(again.action_id, "sent").unwrap();
        assert!(!still_queued(&store, again.action_id));
        assert!(!still_queued(&store, 9_999), "an id that never existed");
    }
}

#[cfg(test)]
mod batch_tests {
    use petrel_engine::store::PendingAction;

    fn row(kind: &str, folder: &str, uid: Option<u32>, action: i64) -> PendingAction {
        PendingAction {
            action_id: action,
            kind_json: kind.to_string(),
            payload_json: "{}".into(),
            message_id: action,
            uid,
            folder_path: folder.into(),
            msgid: None,
            candidate_paths: Vec::new(),
        }
    }

    #[test]
    fn consecutive_flag_uids_stop_at_a_gap_and_cap_at_a_hundred() {
        let mut pending: Vec<PendingAction> = (0..120)
            .map(|i| row("\"mark_read\"", "INBOX", Some(i as u32 + 1), i))
            .collect();
        pending[40].uid = None;
        let group = super::consecutive_flag_uids(
            &pending,
            0,
            "\"mark_read\"",
            "INBOX",
            1,
            petrel_providers::imap::STORE_FLAG_BATCH,
        );
        assert_eq!(group.len(), 40, "stops before the row with no UID");
        assert_eq!(group.last().map(|(_, _, u)| *u), Some(40));

        let full: Vec<PendingAction> = (0..150)
            .map(|i| row("\"mark_read\"", "INBOX", Some(i as u32 + 1), i))
            .collect();
        let group = super::consecutive_flag_uids(
            &full,
            0,
            "\"mark_read\"",
            "INBOX",
            1,
            petrel_providers::imap::STORE_FLAG_BATCH,
        );
        assert_eq!(group.len(), 100);
    }
}

/// The drain against a scripted server: what a refused password costs, and
/// what a lost UID is healed to.
#[cfg(all(test, feature = "dev-plaintext-imap"))]
mod scripted_tests {
    use super::drain_actions;
    use crate::signin::SignIn;
    use crate::state::{AppState, test_state};
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_providers::imap::{Credential, ImapConfig, Security};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    struct Srv {
        /// The password it takes; anything else is refused.
        password: &'static str,
        /// What CAPABILITY answers. A server without MOVE takes the COPY,
        /// \Deleted and expunge path, with its "already copied?" check.
        caps: &'static str,
        boxes: BTreeMap<String, Vec<(u32, Vec<u8>)>>,
        next: BTreeMap<String, u32>,
        /// Every LOGIN, as accepted or refused.
        logins: Vec<bool>,
        lines: Vec<String>,
    }

    type Shared = Arc<Mutex<Srv>>;

    fn srv(password: &'static str) -> Shared {
        Arc::new(Mutex::new(Srv {
            password,
            caps: "MOVE UIDPLUS",
            boxes: [("INBOX", Vec::new()), ("Archive", Vec::new())]
                .into_iter()
                .map(|(n, v)| (n.to_string(), v))
                .collect(),
            next: [("INBOX", 1), ("Archive", 1)]
                .into_iter()
                .map(|(n, v)| (n.to_string(), v))
                .collect(),
            logins: Vec::new(),
            lines: Vec::new(),
        }))
    }

    fn put(s: &Shared, mailbox: &str, uid: u32, raw: Vec<u8>) {
        let mut g = s.lock().unwrap();
        g.boxes.get_mut(mailbox).unwrap().push((uid, raw));
        let next = g.next[mailbox].max(uid + 1);
        g.next.insert(mailbox.to_string(), next);
    }

    fn uids(s: &Shared, mailbox: &str) -> Vec<u32> {
        s.lock().unwrap().boxes[mailbox]
            .iter()
            .map(|(u, _)| *u)
            .collect()
    }

    fn said(s: &Shared, what: &str) -> bool {
        s.lock()
            .unwrap()
            .lines
            .iter()
            .any(|l| l.to_ascii_uppercase().contains(what))
    }

    fn quoted(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            out.push(after[..end].to_string());
            rest = &after[end + 1..];
        }
        out
    }

    /// The Message-ID header line of a message, as a header-fields fetch
    /// returns it.
    fn id_header(raw: &[u8]) -> String {
        let text = String::from_utf8_lossy(raw);
        let line = text
            .split("\r\n")
            .find(|l| l.to_ascii_lowercase().starts_with("message-id:"))
            .unwrap_or("");
        format!("{line}\r\n\r\n")
    }

    fn answer(s: &Shared, selected: &mut Option<String>, line: &str) -> (String, bool) {
        let tag = line.split_whitespace().next().unwrap_or("*").to_string();
        let up = line.to_ascii_uppercase();
        let mut g = s.lock().unwrap();
        g.lines.push(line.trim_end().to_string());
        if up.contains(" LOGOUT") {
            return (format!("* BYE\r\n{tag} OK bye\r\n"), true);
        }
        if up.contains(" LOGIN ") {
            let ok = quoted(line).get(1).map(String::as_str) == Some(g.password);
            g.logins.push(ok);
            return if ok {
                (format!("{tag} OK signed in\r\n"), false)
            } else {
                (
                    format!("{tag} NO [AUTHENTICATIONFAILED] Authentication failed.\r\n"),
                    false,
                )
            };
        }
        if up.contains(" CAPABILITY") {
            return (
                format!("* CAPABILITY IMAP4rev1 {}\r\n{tag} OK done\r\n", g.caps),
                false,
            );
        }
        if up.contains(" SELECT ") || up.contains(" EXAMINE ") {
            let name = quoted(line).into_iter().next().unwrap_or_default();
            let Some(b) = g.boxes.get(&name) else {
                return (format!("{tag} NO [NONEXISTENT] no such mailbox\r\n"), false);
            };
            *selected = Some(name.clone());
            return (
                format!(
                    "* {} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {}] ok\r\n{tag} OK done\r\n",
                    b.len(),
                    g.next[&name]
                ),
                false,
            );
        }
        let Some(sel) = selected.clone() else {
            return (format!("{tag} OK done\r\n"), false);
        };
        if up.contains(" SEARCH ") {
            // A substring of the Message-ID header, case-insensitive, as the
            // RFC defines it.
            let term = quoted(line).pop().unwrap_or_default().to_lowercase();
            let hits: Vec<String> = g.boxes[&sel]
                .iter()
                .filter(|(_, raw)| {
                    !term.is_empty() && id_header(raw).to_lowercase().contains(&term)
                })
                .map(|(u, _)| u.to_string())
                .collect();
            return (
                format!("* SEARCH {}\r\n{tag} OK done\r\n", hits.join(" ")),
                false,
            );
        }
        if up.contains(" UID FETCH ") {
            let wanted: Vec<u32> = line
                .split_whitespace()
                .nth(3)
                .unwrap_or("")
                .split(',')
                .filter_map(|u| u.parse().ok())
                .collect();
            let whole = up.contains("BODY.PEEK[]");
            let mut out = String::new();
            for (seq, (uid, raw)) in g.boxes[&sel].iter().enumerate() {
                if !wanted.contains(uid) {
                    continue;
                }
                let date = internal_date(raw);
                if whole {
                    let body = String::from_utf8_lossy(raw);
                    out.push_str(&format!(
                        "* {} FETCH (UID {uid} FLAGS () RFC822.SIZE {} INTERNALDATE \"{date}\" BODY[] {{{}}}\r\n{body})\r\n",
                        seq + 1,
                        raw.len(),
                        raw.len()
                    ));
                    continue;
                }
                let header = id_header(raw);
                out.push_str(&format!(
                    "* {} FETCH (UID {uid} RFC822.SIZE {} INTERNALDATE \"{date}\" BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{header})\r\n",
                    seq + 1,
                    raw.len(),
                    header.len()
                ));
            }
            return (format!("{out}{tag} OK done\r\n"), false);
        }
        if up.contains(" UID MOVE ") {
            let uid: u32 = line
                .split_whitespace()
                .nth(3)
                .and_then(|u| u.parse().ok())
                .unwrap_or(0);
            let to = quoted(line).pop().unwrap_or_default();
            if let Some(pos) = g.boxes[&sel].iter().position(|(u, _)| *u == uid) {
                let (_, raw) = g.boxes.get_mut(&sel).unwrap().remove(pos);
                let new_uid = g.next[&to];
                g.next.insert(to.clone(), new_uid + 1);
                g.boxes.get_mut(&to).unwrap().push((new_uid, raw));
            }
            return (format!("{tag} OK done\r\n"), false);
        }
        if up.contains(" UID COPY ") {
            let uid: u32 = line
                .split_whitespace()
                .nth(3)
                .and_then(|u| u.parse().ok())
                .unwrap_or(0);
            let to = quoted(line).pop().unwrap_or_default();
            if let Some(raw) = g.boxes[&sel]
                .iter()
                .find(|(u, _)| *u == uid)
                .map(|(_, r)| r.clone())
            {
                let new_uid = g.next[&to];
                g.next.insert(to.clone(), new_uid + 1);
                g.boxes.get_mut(&to).unwrap().push((new_uid, raw));
            }
            return (format!("{tag} OK done\r\n"), false);
        }
        if up.contains(" UID EXPUNGE ") {
            let uid: u32 = line
                .split_whitespace()
                .nth(3)
                .and_then(|u| u.parse().ok())
                .unwrap_or(0);
            g.boxes.get_mut(&sel).unwrap().retain(|(u, _)| *u != uid);
            return (format!("{tag} OK done\r\n"), false);
        }
        (format!("{tag} OK done\r\n"), false)
    }

    /// When the server says a message arrived: fixed by its bytes, so a COPY
    /// keeps it, as RFC 3501 has a COPY keep the original's, and a different
    /// message has its own.
    fn internal_date(raw: &[u8]) -> String {
        let second = raw.iter().map(|b| *b as u32).sum::<u32>() % 60;
        format!("01-Oct-2026 08:00:{second:02} +0000")
    }

    async fn serve(s: Shared) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let s = Arc::clone(&s);
                tokio::spawn(async move {
                    let (rx, mut tx) = sock.into_split();
                    let mut reader = BufReader::new(rx);
                    let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                    let mut selected = None;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let (reply, close) = answer(&s, &mut selected, &line);
                        if tx.write_all(reply.as_bytes()).await.is_err() || close {
                            let _ = tx.shutdown().await;
                            return;
                        }
                    }
                });
            }
        });
        port
    }

    fn cfg(port: u16, password: &str) -> ImapConfig {
        ImapConfig {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            credential: Credential::password(password),
            security: Security::InsecurePlaintext,
        }
    }

    fn raw(id: &str) -> Vec<u8> {
        raw_with(id, &format!("body {id}"))
    }

    fn raw_with(id: &str, body: &str) -> Vec<u8> {
        format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: {id}\r\n\
             Message-ID: <{id}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    fn folders(state: &AppState) -> i64 {
        let account = state.account_id;
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Archive".into(), Some("archive".into())),
                ],
            )
            .unwrap();
        store.folder_for_role(account, "inbox").unwrap().unwrap()
    }

    /// Archives the conversation `message` is in, locally, queuing it.
    fn archive(state: &AppState, message: i64) {
        let account = state.account_id;
        let store = state.store.lock().unwrap();
        let thread = store.thread_of(message).unwrap().unwrap_or(-message);
        store
            .apply_thread_action(
                account,
                thread,
                ActionKind::Archive,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }

    fn drain(state: &Arc<AppState>, cfg: ImapConfig) -> bool {
        let account = state.account_id;
        tauri::async_runtime::block_on(drain_actions(
            Arc::clone(state),
            account,
            cfg,
            false,
            false,
            false,
            &state.stop_signal(account),
        ))
    }

    /// A refused password stops the drain at the first sign-in.
    ///
    /// The drain makes a connection per action, and used to keep going after
    /// the server said no: ten archives were ten refused sign-ins, and twenty
    /// with the "create the folder and try again" path after each — which is
    /// how fail2ban and cPanel's cPHulk count their way to a ban. What was
    /// queued stays queued, its attempts untouched: a refusal is the
    /// password's fault, not the action's.
    #[test]
    fn a_refused_password_stops_the_drain_at_the_first_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let s = srv("right");
        let inbox = folders(&state);
        for uid in 1..=10u32 {
            let id = format!("m{uid}@x.example");
            put(&s, "INBOX", uid, raw(&id));
            let ingested = state
                .store
                .lock()
                .unwrap()
                .ingest_raw(&state.blobs, account, Some(inbox), Some(uid), &raw(&id))
                .unwrap();
            archive(&state, ingested.message_id);
        }
        let queued = |state: &AppState| {
            let store = state.store.lock().unwrap();
            let actions: std::collections::BTreeSet<i64> = store
                .pending_actions(account)
                .unwrap()
                .into_iter()
                .map(|p| p.action_id)
                .collect();
            actions
        };
        let before = queued(&state);
        assert_eq!(before.len(), 10);
        let port = tauri::async_runtime::block_on(serve(Arc::clone(&s)));

        assert!(drain(&state, cfg(port, "revoked")));
        let refused = s.lock().unwrap().logins.iter().filter(|ok| !**ok).count();
        assert_eq!(refused, 1, "one refused sign-in for the whole pass");
        assert!(!said(&s, " CREATE "), "no folder is made over a refusal");
        assert_eq!(state.signin(account), Some(SignIn::Refused));
        assert_eq!(queued(&state), before, "everything stays queued");
        {
            let store = state.store.lock().unwrap();
            for action in &before {
                assert_eq!(
                    store.action_state(*action).unwrap().as_deref(),
                    Some("queued")
                );
            }
        }
        assert_eq!(uids(&s, "INBOX").len(), 10, "nothing moved");

        // A second pass while still refused asks nothing at all: the account
        // has stood down, and the pass stops before its first action. It
        // used to cost one more refused sign-in, each pass.
        assert!(drain(&state, cfg(port, "revoked")));
        let refused = s.lock().unwrap().logins.iter().filter(|ok| !**ok).count();
        assert_eq!(refused, 1);
        assert_eq!(queued(&state), before, "still queued");

        // And once the password is right, all ten go, none the worse.
        state.clear_signin(account);
        assert!(drain(&state, cfg(port, "right")));
        assert!(queued(&state).is_empty(), "delivered");
        assert!(uids(&s, "INBOX").is_empty());
        assert_eq!(uids(&s, "Archive").len(), 10);
        let store = state.store.lock().unwrap();
        for action in &before {
            assert_eq!(
                store.action_state(*action).unwrap().as_deref(),
                Some("sent")
            );
            // Never counted against: the next attempt is its first.
            assert_eq!(store.record_attempt(*action).unwrap(), 1);
        }
    }

    /// A message whose UID was lost is healed only to a copy that is it.
    ///
    /// The heal asks which UIDs carry the message's Message-ID. The search is
    /// a substring match, and the id went out bare, so `5@x.example` found
    /// `<15@x.example>` too and the heal took the last hit: the archive
    /// moved the other message.
    #[test]
    fn a_lost_uid_is_healed_only_to_its_own_message() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let s = srv("right");
        let inbox = folders(&state);
        put(&s, "INBOX", 5, raw("5@x.example"));
        put(&s, "INBOX", 15, raw("15@x.example"));
        // Held here with no UID: the number was lost.
        let ingested = state
            .store
            .lock()
            .unwrap()
            .ingest_raw(
                &state.blobs,
                account,
                Some(inbox),
                None,
                &raw("5@x.example"),
            )
            .unwrap();
        archive(&state, ingested.message_id);
        let port = tauri::async_runtime::block_on(serve(Arc::clone(&s)));
        assert!(drain(&state, cfg(port, "right")));
        assert!(said(&s, "UID MOVE 5 "), "{:?}", s.lock().unwrap().lines);
        assert_eq!(uids(&s, "INBOX"), vec![15], "the other message stays");
        assert!(
            said(&s, "HEADER MESSAGE-ID \"<5@X.EXAMPLE>\""),
            "asked for the id itself"
        );
    }

    /// The one copy carrying the id is another message — a forgery of an
    /// invoice, a list's altered copy — and is not healed onto.
    #[test]
    fn a_heal_onto_a_different_message_under_the_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let s = srv("right");
        let inbox = folders(&state);
        put(
            &s,
            "INBOX",
            5,
            raw_with("5@x.example", "pay to 99-9999-9999, and quickly"),
        );
        let ingested = state
            .store
            .lock()
            .unwrap()
            .ingest_raw(
                &state.blobs,
                account,
                Some(inbox),
                None,
                &raw_with("5@x.example", "pay to 11-1111-1111"),
            )
            .unwrap();
        archive(&state, ingested.message_id);
        let port = tauri::async_runtime::block_on(serve(Arc::clone(&s)));
        assert!(drain(&state, cfg(port, "right")));
        assert!(!said(&s, "UID MOVE"), "{:?}", s.lock().unwrap().lines);
        assert_eq!(uids(&s, "INBOX"), vec![5]);
    }

    /// Two copies carrying the id: the heal does not guess between them.
    #[test]
    fn a_heal_with_two_candidates_moves_neither() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let s = srv("right");
        let inbox = folders(&state);
        put(&s, "INBOX", 5, raw("5@x.example"));
        put(&s, "INBOX", 6, raw("5@x.example"));
        let ingested = state
            .store
            .lock()
            .unwrap()
            .ingest_raw(
                &state.blobs,
                account,
                Some(inbox),
                None,
                &raw("5@x.example"),
            )
            .unwrap();
        archive(&state, ingested.message_id);
        let port = tauri::async_runtime::block_on(serve(Arc::clone(&s)));
        assert!(drain(&state, cfg(port, "right")));
        assert!(!said(&s, "UID MOVE"), "{:?}", s.lock().unwrap().lines);
        assert_eq!(uids(&s, "INBOX"), vec![5, 6]);
    }

    /// A message sent to yourself is one row (one message, two copies) that
    /// holds the Sent copy's bytes, with a placement in Sent and one in INBOX
    /// whose server copy also carries Received, so its size is not the
    /// stored one. Its INBOX number lost, Delete forever's heal refused the
    /// INBOX copy for its size and took the Sent copy instead, which settled
    /// the action: the INBOX copy was never deleted, and the next fetch of it
    /// brought the message back (docs/25 review, finding 3). The copy is
    /// read and known as this message by its content.
    fn sent_to_yourself() -> (Vec<u8>, Vec<u8>) {
        let sender = b"From: Me <me@x.example>\r\nTo: me@x.example\r\nSubject: Notes\r\n\
Date: Mon, 7 Sep 2026 09:00:00 +0000\r\nMessage-ID: <self-1@x.example>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nnotes for friday\r\n"
            .to_vec();
        let mut delivered = b"Received: from submit.x.example by mx.x.example\r\n".to_vec();
        delivered.extend_from_slice(&sender);
        (sender, delivered)
    }

    fn with_sent(state: &AppState, s: &Shared) -> (i64, i64) {
        {
            let mut g = s.lock().unwrap();
            g.boxes.insert("Sent".into(), Vec::new());
            g.next.insert("Sent".into(), 1);
        }
        let account = state.account_id;
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Archive".into(), Some("archive".into())),
                    ("Sent".into(), Some("sent".into())),
                ],
            )
            .unwrap();
        (
            store.folder_for_role(account, "inbox").unwrap().unwrap(),
            store.folder_for_role(account, "sent").unwrap().unwrap(),
        )
    }

    #[test]
    fn a_folded_rows_lost_uid_is_healed_onto_its_own_copy() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let s = srv("right");
        let (inbox, sent) = with_sent(&state, &s);
        let (sender, delivered) = sent_to_yourself();
        put(&s, "Sent", 3, sender.clone());
        put(&s, "INBOX", 7, delivered.clone());
        let id = {
            let mut store = state.store.lock().unwrap();
            let a = store
                .ingest_raw(&state.blobs, account, Some(sent), Some(3), &sender)
                .unwrap();
            // The INBOX copy, its number lost (a renumbering, say).
            let b = store
                .ingest_raw(&state.blobs, account, Some(inbox), None, &delivered)
                .unwrap();
            assert_eq!(a.message_id, b.message_id, "one row");
            a.message_id
        };
        state
            .store
            .lock()
            .unwrap()
            .apply_message_action(
                account,
                id,
                ActionKind::DeleteForever,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        let port = tauri::async_runtime::block_on(serve(Arc::clone(&s)));
        assert!(drain(&state, cfg(port, "right")));
        assert!(said(&s, "UID STORE 7 "), "{:?}", s.lock().unwrap().lines);
        assert!(uids(&s, "INBOX").is_empty(), "the INBOX copy is gone");
        assert!(
            said(&s, "BODY.PEEK[]"),
            "the copy of another size was read before it was taken"
        );
        assert!(
            state
                .store
                .lock()
                .unwrap()
                .pending_actions(account)
                .unwrap()
                .is_empty()
        );
    }

    /// The same folded row, archived on a server without MOVE after a COPY
    /// that landed and a STORE that did not. The landed copy is the INBOX
    /// copy's, whose size is not the stored one: the stored size refused it,
    /// and the retry copied again. With no other row under the Message-ID,
    /// the source's own size and arrival time say it is the copy.
    #[test]
    fn a_folded_rows_landed_copy_is_not_copied_again() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let s = srv("right");
        s.lock().unwrap().caps = "UIDPLUS";
        let (inbox, sent) = with_sent(&state, &s);
        let (sender, delivered) = sent_to_yourself();
        put(&s, "Sent", 3, sender.clone());
        put(&s, "INBOX", 7, delivered.clone());
        // The earlier attempt's COPY, which landed.
        put(&s, "Archive", 1, delivered.clone());
        let id = {
            let mut store = state.store.lock().unwrap();
            let a = store
                .ingest_raw(&state.blobs, account, Some(sent), Some(3), &sender)
                .unwrap();
            let b = store
                .ingest_raw(&state.blobs, account, Some(inbox), Some(7), &delivered)
                .unwrap();
            assert_eq!(a.message_id, b.message_id, "one row");
            a.message_id
        };
        // The message itself, as its own menu archives it: a conversation's
        // Archive leaves a member that sits in Sent where it is.
        state
            .store
            .lock()
            .unwrap()
            .apply_message_action(
                account,
                id,
                ActionKind::Archive,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        let port = tauri::async_runtime::block_on(serve(Arc::clone(&s)));
        assert!(drain(&state, cfg(port, "right")));
        assert!(!said(&s, "UID COPY"), "{:?}", s.lock().unwrap().lines);
        assert_eq!(uids(&s, "Archive"), vec![1], "one copy in the Archive");
        assert!(uids(&s, "INBOX").is_empty());
    }
}

#[cfg(all(test, feature = "dev-plaintext-imap"))]
mod signed_out_tests {
    //! The drain and the account's sign-in, against a server that counts
    //! every LOGIN (`scripted_imap`).
    use super::drain_actions;
    use crate::scripted_imap::{Srv, raw, serve};
    use crate::signin::refused_password_tests::{plain, queued_archives};
    use crate::state::test_state;
    use std::sync::Arc;

    /// The second review's R2-D. Dovecot ends its tagged replies with the
    /// command's timing, so a MOVE refused after 535 ms reads "(0.535 + 0.000
    /// secs)." — and a bare "535 " in the words was read as SMTP's refused
    /// password: the whole account stood down for an hour, and the folder was
    /// never made. A refused password is told by the sign-in's own verdict.
    #[test]
    fn a_move_refused_with_dovecot_timing_is_not_a_refused_password() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let srv = Srv::new(
            "MOVE UIDPLUS",
            &[("INBOX", ""), ("Archive", "\\Archive")],
            &["pass"],
        );
        *srv.move_reply.lock().unwrap() =
            Some("NO [TRYCREATE] Mailbox doesn't exist: Archive (0.535 + 0.000 secs).".into());
        queued_archives(
            &state,
            &Srv::new("", &[("INBOX", ""), ("Archive", "")], &[]),
            1,
        );
        srv.put("INBOX", 1, raw(1));
        let said = tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            let stop = state.stop_signal(account);
            drain_actions(
                Arc::clone(&state),
                account,
                plain(port, "pass"),
                true,
                true,
                false,
                &stop,
            )
            .await;
            srv.seen.lock().unwrap().clone()
        });
        let created = said
            .iter()
            .any(|l| l.to_ascii_uppercase().contains(" CREATE "));
        assert_eq!(
            state.signin(account),
            None,
            "a NO to MOVE stood the account down"
        );
        assert!(
            created,
            "the folder was made and the move tried again: {said:?}"
        );
    }

    /// A pass of a worker that has been told to stop goes no further: Sign
    /// in again stops the old workers between actions, so the old password's
    /// pass cannot deliver the queue somewhere the new settings no longer
    /// point, or come back with a refusal over the new password.
    #[test]
    fn a_stopped_pass_delivers_nothing_more() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let srv = Srv::new(
            "MOVE UIDPLUS",
            &[("INBOX", ""), ("Archive", "\\Archive")],
            &["pass"],
        );
        queued_archives(
            &state,
            &Srv::new("", &[("INBOX", ""), ("Archive", "")], &[]),
            3,
        );
        for uid in 1..=3 {
            srv.put("INBOX", uid, raw(uid));
        }
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            let stop = state.stop_signal(account);
            state.stop_workers(account);
            drain_actions(
                Arc::clone(&state),
                account,
                plain(port, "pass"),
                true,
                true,
                false,
                &stop,
            )
            .await;
        });
        let queued = state
            .store
            .lock()
            .unwrap()
            .pending_actions(account)
            .unwrap()
            .len();
        assert_eq!(srv.logins.lock().unwrap().len(), 0, "nothing signed in");
        assert_eq!(queued, 3, "the queue waits for the run that replaced it");
    }
}
