//! Acting on mail: triage and its undo, tags, and folders.

use crate::config::imap_config_from_servers;
use crate::diag::log_sync;
use crate::signin::{SIGN_IN_FIRST, SignIn, refused_or, server_for};
use crate::state::{AppState, active_account, note_ui_touch};
use petrel_engine::actions::{ActionKind, ActionReceipt};
use petrel_engine::store::FolderSummary;
use std::sync::Arc;
use tauri::State;

/// Applies a triage action locally and queues it. Returns the receipt the UI
/// needs to offer undo, so the frontend holds no state of its own about what it
/// just did.
///
/// `message_id` names one message instead, for the view that lists per message.
/// Drafts is the only one, and there the conversation is the wrong unit: a
/// pushed draft shares its conversation's thread, so a verb aimed at the draft
/// row used to file the whole correspondence. Sent by the window rather than
/// inferred here, because which view the click came from is the window's to
/// know. Placement verbs only; the store refuses the rest.
#[tauri::command(async)]
pub fn triage(
    thread_id: i64,
    kind: ActionKind,
    target: Option<i64>,
    message_id: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<ActionReceipt, String> {
    let store = state.store()?;
    let account = triage_account(&store, thread_id, message_id)?;
    // The provider's placement model, not a per-call guess: on Gmail an
    // archive removes one label, on a classic server it replaces the folder.
    let policy = store.placement_policy(account).map_err(|e| e.to_string())?;
    let receipt = match message_id {
        Some(id) => store
            .apply_message_action(account, id, kind, target, policy)
            .map_err(|e| e.to_string())?,
        None => store
            .apply_thread_action(account, thread_id, kind, target, policy)
            .map_err(|e| e.to_string())?,
    };
    // Local change done; ask for it to be delivered. The lock is released as
    // this returns, so the drain is never waiting on the caller.
    state.nudge_drain(account);
    Ok(receipt)
}

/// The account a triage acts for: the one the conversation or message is in,
/// not whichever the rail shows now.
///
/// A pop-out left open across an account switch, and a batch still running
/// when the account changed, both send ids from the account they were
/// showing. Acted on under the active account, that mail was filed into this
/// account's folders and queued for this account's drain, which then moved
/// or expunged its own mail by the other account's UIDs. The active account
/// is the fallback only for an id with nothing live behind it, which the
/// store then answers as it always has.
fn triage_account(
    store: &petrel_engine::store::Store,
    thread_id: i64,
    message_id: Option<i64>,
) -> Result<i64, String> {
    let owner = match message_id {
        Some(id) => store.account_of_message(id),
        None => store.thread_account(thread_id),
    }
    .map_err(|e| e.to_string())?;
    match owner {
        Some(account) => Ok(account),
        None => active_account(store),
    }
}

/// The account a folder belongs to, whose server its commands go to.
///
/// Not the account on screen. A confirmation opened on one account's folder
/// stayed open across ⌘2, and its command then took the other account's
/// server and the folder's path: Delete renamed a folder of the same name
/// over there, and Move all to Trash binned its mail. The folder says whose
/// it is.
pub(crate) fn folder_account(
    store: &petrel_engine::store::Store,
    folder_id: i64,
) -> Result<i64, String> {
    for account in store.account_ids().map_err(|e| e.to_string())? {
        if store
            .account_owns_folder(account, folder_id)
            .map_err(|e| e.to_string())?
        {
            return Ok(account);
        }
    }
    Err("no such folder".into())
}

/// An account the window named, as long as it still exists.
///
/// For the commands that act on a whole account rather than on an item that
/// carries one: Empty Trash and the identity. They are asked for the account
/// the dialog or pane was opened on, because by the time the person confirms,
/// the rail may be showing another — and "permanently empty the Trash" must
/// never land on a Trash nobody looked at.
pub(crate) fn named_account(
    store: &petrel_engine::store::Store,
    account_id: i64,
) -> Result<i64, String> {
    if store
        .account_ids()
        .map_err(|e| e.to_string())?
        .contains(&account_id)
    {
        Ok(account_id)
    } else {
        Err("that account is no longer here".into())
    }
}

#[tauri::command(async)]
pub fn undo_triage(action_id: i64, state: State<Arc<AppState>>) -> Result<bool, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    let undone = store.undo_action(action_id).map_err(|e| e.to_string())?;
    // An undo can leave other queued work behind it, and the row it cancelled
    // is gone from the queue — either way the server's picture just changed.
    state.nudge_drain(account);
    Ok(undone)
}

/// Creates a tag, or returns the one already there — same shape as folders.
#[tauri::command(async)]
pub fn create_tag(name: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store
        .ensure_tag(account, &name, None)
        .map_err(|e| e.to_string())
}

/// Corrects a tag's name. The colour and every tagged message come with it.
#[tauri::command(async)]
pub fn rename_tag(tag_id: i64, name: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    store.rename_tag(tag_id, &name).map_err(|e| e.to_string())
}

/// Sets a tag's colour. Local by design: no provider has a field for it.
#[tauri::command(async)]
pub fn set_tag_colour(
    tag_id: i64,
    colour: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    store
        .set_tag_colour(tag_id, &colour)
        .map_err(|e| e.to_string())
}

/// Removes a tag from the account and from every message carrying it.
#[tauri::command(async)]
pub fn delete_tag(tag_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    store.delete_tag(tag_id).map_err(|e| e.to_string())
}

/// Folders for the move picker (V).
///
/// `account` names one explicitly. Absent it means the account on screen,
/// which is what every list in the window wants — but the export pane offers
/// a row per account, and a folder list borrowed from whichever one happens
/// to be active would name places that account's export cannot find.
#[tauri::command(async)]
pub fn list_folders(
    account: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<Vec<FolderSummary>, String> {
    note_ui_touch(&state);
    let store = state.store()?;
    let account = match account {
        Some(id) => id,
        None => match store.active_account().map_err(|e| e.to_string())? {
            Some(id) => id,
            None => return Ok(Vec::new()),
        },
    };
    store.folders(account).map_err(|e| e.to_string())
}

/// Creates a folder the user named, or returns the one already there. The
/// picker offers this on the end of the same keystroke as choosing one.
///
/// Here only. The server's copy is `push_folder`, which the caller awaits or
/// not as suits it — the picker has mail to file and the id is all it needs,
/// while the rail's New folder has nothing to do but say whether it worked.
/// This used to fire the server's create off on its own and forget it: a
/// failure reached the log and nowhere else, and the next sync, not finding
/// the folder on the server, deleted it here as well.
#[tauri::command(async)]
pub fn create_folder(path: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store
        .ensure_named_folder(account, &path)
        .map_err(|e| e.to_string())
}

/// Puts a folder made here on the server, and subscribes to it so webmail
/// shows it too. Ok means the server has it.
///
/// A folder the server already has, or a local one, is a no-op; so is one
/// belonging to an account other than the one on screen, which the sync of
/// its own account will create instead. A failure is returned rather than
/// only logged, so the person hears about it — and the folder stays waiting,
/// which is what makes the next sync try again.
#[tauri::command]
pub async fn push_folder(folder_id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    push_folder_to_server(&state, folder_id).await
}

/// `push_folder`, for any state.
async fn push_folder_to_server(state: &AppState, folder_id: i64) -> Result<(), String> {
    let (servers, path) = {
        let store = state.store()?;
        // Its own account's server, whichever account is on screen now.
        let Ok(account) = folder_account(&store, folder_id) else {
            return Ok(());
        };
        let waiting = !store
            .folder_is_local(folder_id)
            .map_err(|e| e.to_string())?
            && store
                .folder_awaits_server(folder_id)
                .map_err(|e| e.to_string())?;
        if !waiting {
            return Ok(());
        }
        // Signed out, the folder stays waiting, and the sync after the
        // account signs in again creates it, as it does after any failure.
        // Said, not passed off as done: Ok here read as "Created" for a
        // folder the server never got.
        if state.signin(account).is_some() {
            return Err(SIGN_IN_FIRST.into());
        }
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        let servers = store.account_servers(account).map_err(|e| e.to_string())?;
        (servers.map(|s| (account, s)), path)
    };
    let Some((account, servers)) = servers.filter(|(_, s)| !s.imap_host.is_empty()) else {
        return Ok(());
    };
    let stop = state.stop_signal(account);
    // Outside the lock: the keychain may ask, and nothing should queue behind it.
    let Some(cfg) = imap_config_from_servers(account, servers) else {
        // Servers and no password Petrel can read: not a folder made, but an
        // account that needs its password, as `server_for` has it.
        state.set_signin(account, SignIn::Missing);
        return Err(SIGN_IN_FIRST.into());
    };
    match petrel_providers::imap::create_folder(&cfg, &path).await {
        Ok(()) => {
            let store = state.store()?;
            store
                .confirm_folder_on_server(folder_id)
                .map_err(|e| e.to_string())
        }
        Err(e) => {
            log_sync(&format!(
                "server create {path} failed, next sync retries: {e}"
            ));
            Err(refused_or(state, account, &stop, e))
        }
    }
}

/// Whether a folder lives only here, so renaming or deleting it has nothing
/// to ask the server: a local one never goes there, and one still waiting
/// has not arrived. Asking anyway was refused as a mailbox that does not
/// exist, and that refusal stopped the local change as well — a folder whose
/// create had failed could be neither renamed nor deleted.
fn only_here(store: &petrel_engine::store::Store, folder_id: i64) -> Result<bool, String> {
    Ok(store
        .folder_is_local(folder_id)
        .map_err(|e| e.to_string())?
        || store
            .folder_awaits_server(folder_id)
            .map_err(|e| e.to_string())?)
}

/// Renames a folder — on the server first, then locally, so the two cannot
/// disagree with the server holding the older name.
///
/// One still waiting for the server is renamed here only, and keeps waiting:
/// the sync creates it under whatever it is called by then.
#[tauri::command]
pub async fn rename_folder(
    folder_id: i64,
    new_path: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let (cfg, old_path, account) = {
        let store = state.store()?;
        let account = folder_account(&store, folder_id)?;
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        let cfg = match only_here(&store, folder_id)? {
            true => None,
            false => server_for(&state, &store, account)?,
        };
        (cfg, path, account)
    };
    if let Some(cfg) = cfg {
        let stop = state.stop_signal(account);
        petrel_providers::imap::rename_folder(&cfg, &old_path, &new_path)
            .await
            .map_err(|e| refused_or(&state, account, &stop, e))?;
    }
    let mut store = state.store()?;
    store
        .rename_folder(folder_id, &new_path)
        .map_err(|e| e.to_string())
}

/// Deletes a folder — on the server first. The server also deletes whatever
/// mail the folder still holds, which is why the UI confirms in those words;
/// the store keeps its message rows and blobs regardless, so nothing already
/// synced is destroyed. One that lives only here is deleted only here.
#[tauri::command]
pub async fn delete_folder(folder_id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let (cfg, path, account) = {
        let store = state.store()?;
        let account = folder_account(&store, folder_id)?;
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        let cfg = match only_here(&store, folder_id)? {
            true => None,
            false => server_for(&state, &store, account)?,
        };
        (cfg, path, account)
    };
    if let Some(cfg) = cfg {
        let stop = state.stop_signal(account);
        petrel_providers::imap::delete_folder(&cfg, &path)
            .await
            .map_err(|e| refused_or(&state, account, &stop, e))?;
    }
    let mut store = state.store()?;
    let took = store.remove_folder(folder_id).map_err(|e| e.to_string())?;
    if took > 0 {
        log_sync(&format!(
            "folder deleted: {took} message(s) that lived only there went with it"
        ));
    }
    Ok(())
}

/// Empties the bin: everything in Trash, and in any folder filed under it,
/// expunged on the server and tombstoned here.
///
/// The one action in the app with no undo, which is why it is a button
/// someone presses in the Trash itself and not a thing that happens on a
/// timer. Retention already reaps *tombstones* after their grace period;
/// this is the person saying "now", about mail they can see.
///
/// A message the server refuses to expunge stays — reported, not pretended
/// away. Emptying half a bin and saying it is empty would be the one
/// outcome worse than not emptying it.
#[tauri::command]
pub async fn empty_trash(
    account_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let (account, items) = {
        let store = state.store()?;
        let account = named_account(&store, account_id)?;
        let items = store.trash_contents(account).map_err(|e| e.to_string())?;
        (account, items)
    };
    let (gone, kept) = destroy_trashed(&state, account, items).await?;
    Ok(format!("{gone}/{kept}"))
}

/// Expunges a set of trashed messages and tombstones them here.
///
/// Shared by the button and the clock so they cannot drift: emptying the
/// bin by hand and emptying it by expiry are the same act on a different
/// selection, and two implementations of "destroy this mail" is one too
/// many. Returns (removed, kept).
pub(crate) async fn destroy_trashed(
    state: &Arc<AppState>,
    account: i64,
    items: Vec<(String, u32, i64)>,
) -> Result<(usize, usize), String> {
    if items.is_empty() {
        return Ok((0, 0));
    }
    // Asked only while the account can sign in: one expunge per message is
    // one sign-in per message, and a 500-message Trash emptied while the
    // password was refused was 500 refused sign-ins.
    let (cfg, uidplus) = {
        let store = state.store()?;
        (
            server_for(state, &store, account)?,
            state.caps(account).has_uidplus,
        )
    };
    let stop = state.stop_signal(account);
    let mut gone = 0usize;
    let mut kept = 0usize;
    let mut refused = false;
    for (path, uid, message_id) in items {
        let removed = match &cfg {
            // Refused part-way, the rest stay: nothing more is asked of a
            // server that has said no.
            Some(_) if refused => false,
            Some(cfg) => {
                match petrel_providers::imap::expunge_uid(cfg, &path, uid, uidplus).await {
                    Ok(_) => true,
                    Err(e) => {
                        log_sync(&format!("empty trash: {path} uid {uid}: {e}"));
                        if e.is_sign_in_refused() {
                            state.refused_by(account, &stop);
                            refused = true;
                        }
                        false
                    }
                }
            }
            // No server for this account: local-only mail is ours to drop.
            None => true,
        };
        if removed {
            if let Ok(store) = state.store.lock() {
                let _ = store.tombstone_message(message_id);
            }
            gone += 1;
        } else {
            kept += 1;
        }
    }
    log_sync(&format!("trash: {gone} removed, {kept} kept"));
    if refused && gone == 0 {
        return Err(SIGN_IN_FIRST.into());
    }
    Ok((gone, kept))
}

/// The order somebody dragged their folders into.
///
/// Local only, and it never touches the server: IMAP has no notion of an
/// order, so there is nothing to push and nothing that can come back to
/// contradict it. That also makes this the rare folder command that cannot
/// half-fail, which is why it has no rollback.
#[tauri::command(async)]
pub fn reorder_folders(ids: Vec<i64>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.reorder_folders(&ids).map_err(|e| e.to_string())
}

/// The order somebody dragged their tags into. Local, for the same reason.
#[tauri::command(async)]
pub fn reorder_tags(ids: Vec<i64>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.reorder_tags(&ids).map_err(|e| e.to_string())
}

/// How many messages a folder holds, so a confirmation can name the number.
#[tauri::command(async)]
pub fn folder_message_count(folder_id: i64, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    store
        .folder_message_count(folder_id)
        .map_err(|e| e.to_string())
}

/// Marks everything in a folder read, or unread.
///
/// Done here rather than through the action queue, which is per message: a
/// folder with ten thousand messages in it would put ten thousand rows in that
/// queue and spend ten thousand round trips draining them. IMAP will set the
/// whole mailbox in one command, so that is what this sends, the same way
/// Empty Trash does its own work rather than queuing it.
///
/// Local first, then the server, which is the opposite of `rename_folder` and
/// deliberately so: this is not destructive, the local half is instant, and
/// somebody who marks a folder read wants the number to move now rather than
/// after a round trip. A server that refuses leaves the two disagreeing until
/// the next sync reconciles, which is the ordinary state of every flag here.
#[tauri::command]
pub async fn mark_folder_read(
    folder_id: i64,
    read: bool,
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let (cfg, paths, changed, account) = {
        let store = state.store()?;
        let account = folder_account(&store, folder_id)?;
        // Asked before the local half: signed out, nothing changes here
        // either, rather than marking read here what the server never hears.
        // A folder that lives only here needs no server.
        let cfg = match only_here(&store, folder_id)? {
            true => None,
            false => server_for(&state, &store, account)?,
        };
        // The subtree, because that is what "all" means on a row with folders
        // under it. IMAP has no recursive STORE, so this is one command per
        // mailbox — sixteen for a real Archive, against the ten thousand a
        // per-message queue would have sent.
        let paths: Vec<String> = store
            .folder_subtree(folder_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        let changed = store
            .mark_folder_seen(folder_id, read)
            .map_err(|e| e.to_string())?;
        (cfg, paths, changed, account)
    };
    if let Some(cfg) = cfg {
        let stop = state.stop_signal(account);
        let mut total = 0u32;
        for path in &paths {
            match petrel_providers::imap::store_flag_all(&cfg, path, "\\Seen", read).await {
                Ok(n) => total += n,
                // Reported, not swallowed. The local half already happened and
                // the next sync will notice the disagreement; what must not
                // happen is silence about a server that said no.
                Err(e) => return Err(refused_or(&state, account, &stop, e)),
            }
        }
        log_sync(&format!(
            "marked {} across {} mailbox(es), {total} message(s)",
            if read { "read" } else { "unread" },
            paths.len()
        ));
    }
    Ok(changed)
}

/// Moves everything in a folder to the Trash.
///
/// The folder stays; only its contents go. Recoverable exactly as any other
/// binning is — the mail is in the Trash until somebody empties it — which is
/// why this is a confirm rather than the undo the per-message actions get:
/// capturing prior state for ten thousand messages to make one undo entry is
/// a lot of database for a gesture whose inverse is "drag it back".
#[tauri::command]
pub async fn trash_folder_contents(
    folder_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let (cfg, from_paths, to_path, to_id, has_move, account) = {
        let store = state.store()?;
        let account = folder_account(&store, folder_id)?;
        let from_paths: Vec<String> = store
            .folder_subtree(folder_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        let to_id = store
            .folder_for_role(account, "trash")
            .map_err(|e| e.to_string())?
            .ok_or("this account has no Trash")?;
        let to_path = store
            .folder_path(to_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        (
            server_for(&state, &store, account)?,
            from_paths,
            to_path,
            to_id,
            state.caps(account).has_move,
            account,
        )
    };
    if from_paths.contains(&to_path) {
        return Err("that is the Trash".into());
    }
    // Server first here, unlike marking read: this one moves mail, and a local
    // move that the server refused would show an empty folder that is still
    // full on every other client.
    if let Some(cfg) = cfg {
        let stop = state.stop_signal(account);
        let mut moved = 0u32;
        for from in &from_paths {
            moved += petrel_providers::imap::move_all(&cfg, from, &to_path, has_move)
                .await
                .map_err(|e| refused_or(&state, account, &stop, e))?;
        }
        log_sync(&format!(
            "moved {moved} message(s) from {} mailbox(es) to {to_path}",
            from_paths.len()
        ));
    }
    let mut store = state.store()?;
    store
        .move_folder_contents(folder_id, to_id)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{folder_account, named_account, triage_account};
    use petrel_engine::store::{NewMessage, Store};

    fn one_message(store: &mut Store, account: i64) -> i64 {
        store
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: 1_000,
                from_addr: "a@example.com".into(),
                from_display: "A".into(),
                to_addr: "me@example.com".into(),
                subject: "hello".into(),
                body_text: "body".into(),
            }])
            .unwrap()[0]
    }

    #[test]
    fn a_triage_acts_for_the_account_the_mail_is_in_not_the_one_on_screen() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store.ensure_test_account().unwrap();
        let mine = one_message(&mut store, a);
        let thread = store.thread_of(mine).unwrap().unwrap_or(-mine);
        // The window has moved on to B, as an account switch does.
        store.set_active_account(b).unwrap();

        assert_eq!(triage_account(&store, thread, None), Ok(a));
        assert_eq!(triage_account(&store, thread, Some(mine)), Ok(a));
    }

    #[test]
    fn a_folder_command_acts_for_the_account_the_folder_is_in() {
        let store = Store::open_in_memory().unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store.ensure_test_account().unwrap();
        // The same name in both accounts, as "Contracts" was in the probe.
        let in_a = store.ensure_named_folder(a, "Contracts").unwrap();
        let in_b = store.ensure_named_folder(b, "Contracts").unwrap();
        // The window has moved on to B, as ⌘2 under an open dialog did.
        store.set_active_account(b).unwrap();

        assert_eq!(folder_account(&store, in_a), Ok(a));
        assert_eq!(folder_account(&store, in_b), Ok(b));
        assert!(folder_account(&store, 987_654).is_err());
    }

    #[test]
    fn a_named_account_is_taken_as_named_while_it_exists() {
        let store = Store::open_in_memory().unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store.ensure_test_account().unwrap();
        store.set_active_account(b).unwrap();

        assert_eq!(named_account(&store, a), Ok(a));
        store.remove_account(a).unwrap();
        assert!(named_account(&store, a).is_err());
    }

    #[test]
    fn an_id_with_nothing_behind_it_falls_back_to_the_account_on_screen() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store.ensure_test_account().unwrap();
        one_message(&mut store, a);
        store.set_active_account(b).unwrap();
        assert_eq!(triage_account(&store, 987_654, None), Ok(b));
        assert_eq!(triage_account(&store, 987_654, Some(987_654)), Ok(b));
    }
}

#[cfg(test)]
mod signed_out_tests {
    //! The review's probe C. Empty Trash, and the bin's own expiry, made one
    //! connection per message, each a sign-in, without looking at whether
    //! the account could sign in: a 500-message Trash emptied while the
    //! password was refused was 500 refused sign-ins. And an account with
    //! servers but no password Petrel could read counted as one with no
    //! server: its Trash was tombstoned here and reported gone while the
    //! server kept every message.
    use super::{destroy_trashed, push_folder_to_server};
    use crate::scripted_imap::{Srv, raw, serve};
    use crate::signin::test_support::{servers_at, unkeyed_account};
    use crate::signin::{SIGN_IN_FIRST, SignIn};
    use crate::state::test_state;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    /// A Trash of `n` messages on `account`, each a placement with a UID.
    fn full_trash(state: &crate::state::AppState, account: i64, n: u32) -> Vec<(String, u32, i64)> {
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Trash".into(), Some("trash".into())),
                ],
            )
            .unwrap();
        let trash = store.folder_for_role(account, "trash").unwrap().unwrap();
        for uid in 1..=n {
            store
                .ingest_raw(&state.blobs, account, Some(trash), Some(uid), &raw(uid))
                .unwrap();
        }
        store.trash_contents(account).unwrap()
    }

    #[test]
    fn empty_trash_on_a_refused_account_asks_the_server_nothing() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let srv = Srv::new("UIDPLUS", &[("INBOX", ""), ("Trash", "\\Trash")], &[]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            servers_at(&state, account, port);
            crate::config::remember_password(account, "revoked");
            let items = full_trash(&state, account, 12);
            assert_eq!(items.len(), 12);
            state.set_signin(account, SignIn::Refused);
            let result = destroy_trashed(&state, account, items).await;
            let conns = srv.conns.load(Ordering::SeqCst);
            let left = state
                .store
                .lock()
                .unwrap()
                .trash_contents(account)
                .unwrap()
                .len();
            crate::config::forget_password(account);
            assert_eq!(result, Err(SIGN_IN_FIRST.to_string()));
            assert_eq!(conns, 0, "sign-in attempts for one Empty Trash");
            assert_eq!(left, 12, "nothing is tombstoned here either");
        });
    }

    #[test]
    fn an_account_with_servers_and_no_password_is_not_a_local_account() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let srv = Srv::new("UIDPLUS", &[("INBOX", ""), ("Trash", "\\Trash")], &[]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            servers_at(&state, account, port);
            // No password anywhere for this id.
            crate::config::forget_password(account);
            let items = full_trash(&state, account, 3);
            let result = destroy_trashed(&state, account, items).await;
            let left = state
                .store
                .lock()
                .unwrap()
                .trash_contents(account)
                .unwrap()
                .len();
            assert_eq!(result, Err(SIGN_IN_FIRST.to_string()));
            assert_eq!(left, 3, "the server still has them, so they stay");
            assert_eq!(state.signin(account), Some(SignIn::Missing));
            assert_eq!(srv.conns.load(Ordering::SeqCst), 0);
        });
    }

    /// The second UI review's finding 5. A folder made while the account is
    /// signed out stays here, waiting, and the push says so instead of Ok —
    /// which the window read as "Created" for a folder the server never got.
    /// Nor does it ask the server.
    #[test]
    fn a_folder_made_while_signed_out_is_not_said_to_be_on_the_server() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let srv = Srv::new("UIDPLUS", &[("INBOX", "")], &[]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            servers_at(&state, account, port);
            crate::config::remember_password(account, "revoked");
            state.set_signin(account, SignIn::Refused);
            let folder = state
                .store
                .lock()
                .unwrap()
                .ensure_named_folder(account, "Projects")
                .unwrap();
            let result = push_folder_to_server(&state, folder).await;
            let waiting = state
                .store
                .lock()
                .unwrap()
                .folder_awaits_server(folder)
                .unwrap();
            crate::config::forget_password(account);
            assert_eq!(result, Err(SIGN_IN_FIRST.to_string()));
            assert_eq!(srv.conns.load(Ordering::SeqCst), 0, "asked the server");
            assert!(waiting, "kept here, for the sync after signing in");
        });
    }

    /// An account with no server at all is Petrel's own, and its Trash is
    /// emptied here.
    #[test]
    fn a_local_accounts_trash_is_emptied_here() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let items = full_trash(&state, account, 2);
        let result = tauri::async_runtime::block_on(destroy_trashed(&state, account, items));
        assert_eq!(result, Ok((2, 0)));
    }
}
