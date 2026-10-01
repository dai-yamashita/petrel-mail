//! Signing in again: what Petrel knows about an account it cannot sign in to,
//! and how the workers stand down while it cannot.
//!
//! Two ways an account ends up here. Its password is missing: no keychain
//! item Petrel can read, as an account brought in by Import settings has, or
//! one whose keychain prompt was refused. Nothing syncs for it until one is
//! entered. Or the server refused the password Petrel has, and then asking
//! again every couple of minutes is the worst thing to do: fail2ban and
//! cPanel's cPHulk block the account, or the address, after a handful of
//! failures — often the person's whole home network, phone included.
//! Thunderbird stops and asks. Petrel stands the account down, asks the
//! server again once an hour in case the refusal was the server's mistake,
//! and starts over at once when a new password is saved.

use crate::state::{AppState, now_ms, stopped};
use petrel_engine::store::Store;
use petrel_providers::imap::ImapConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::watch::Receiver;

/// What a command says when the account it would reach the server for is
/// signed out. English, like the shell's other refusals; the window puts it
/// in the person's language (`lib/signin-refusal.ts`).
pub(crate) const SIGN_IN_FIRST: &str = "sign in to this account again first";

/// Why Petrel cannot sign in to an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SignIn {
    /// No password Petrel can read for it.
    Missing,
    /// The server refused the password Petrel has.
    Refused,
}

/// What an account could not do while it was signed out, done once it can.
///
/// A draft's push and the removal of a sent or discarded draft's server
/// copies wait here instead of asking a server that refuses the password —
/// the composer's autosave alone was a refused sign-in every thirty seconds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    /// Drafts whose push is waiting.
    pub(crate) drafts: std::collections::BTreeSet<i64>,
    /// Server copies still to remove, by folder path and UID.
    pub(crate) drops: Vec<(String, u32)>,
}

/// How long a refused account waits before asking the server again. An
/// hour is about one attempt where the old loop made fifty-five to
/// eighty-five, and still recovers on its own from a server that refused
/// in error.
pub(crate) const REFUSED_RETRY: Duration = Duration::from_secs(60 * 60);

impl AppState {
    /// Why this account cannot sign in, if it cannot.
    pub(crate) fn signin(&self, account: i64) -> Option<SignIn> {
        self.signin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&account)
            .map(|(why, _)| *why)
    }

    /// Records that the account cannot sign in, as of now. A refusal seen
    /// again restarts its hour.
    pub(crate) fn set_signin(&self, account: i64, why: SignIn) {
        self.signin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account, (why, now_ms()));
        self.signin_changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// The account can sign in again: a new password was saved, or the
    /// server took the old one after all.
    pub(crate) fn clear_signin(&self, account: i64) {
        let had = self
            .signin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&account)
            .is_some();
        if had {
            self.signin_changed.send_modify(|n| *n = n.wrapping_add(1));
        }
    }

    /// Every account that cannot sign in, and why, by id: the window offers
    /// "Sign in again" for one that is not on screen too.
    pub(crate) fn signins(&self) -> Vec<(i64, SignIn)> {
        let mut all: Vec<(i64, SignIn)> = self
            .signin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, (why, _))| (*id, *why))
            .collect();
        all.sort_by_key(|(id, _)| *id);
        all
    }

    /// Records a refusal one of the account's workers met, unless that
    /// worker has been told to stop. Returns whether it was recorded.
    ///
    /// Looked at and written under one lock. A loop stopped by Sign in again
    /// could hear its old password refused a moment after the new one had
    /// been saved, and marked the account refused over it: the new workers
    /// then stood down for an hour on a password that worked, and the window
    /// said "the server refused the password" right after "Signed in". A
    /// restart flips the old switch before it clears the state, so a write
    /// either lands before the flip, and is cleared, or sees it.
    pub(crate) fn refused_by(&self, account: i64, stop: &Receiver<bool>) -> bool {
        {
            let mut all = self.signin.lock().unwrap_or_else(|p| p.into_inner());
            if *stop.borrow() {
                return false;
            }
            all.insert(account, (SignIn::Refused, now_ms()));
        }
        self.signin_changed.send_modify(|n| *n = n.wrapping_add(1));
        true
    }

    /// Clears a refusal on a worker's word that the server took the
    /// password, unless that worker has been told to stop.
    pub(crate) fn cleared_by(&self, account: i64, stop: &Receiver<bool>) {
        let had = {
            let mut all = self.signin.lock().unwrap_or_else(|p| p.into_inner());
            !*stop.borrow() && all.remove(&account).is_some()
        };
        if had {
            self.signin_changed.send_modify(|n| *n = n.wrapping_add(1));
        }
    }

    /// Forgets what this session learned about the account's server: what it
    /// can do, and whether its folders were surveyed.
    ///
    /// For an account starting over. A restart that kept the last run's
    /// capabilities read them as known, so when the new first pass found no
    /// network, the IDLE watchers were never started for the rest of the
    /// session. And after new server settings, or an id reused by another
    /// account, the old server's answers described a different server.
    pub(crate) fn forget_server(&self, account: i64) {
        self.caps
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&account);
        self.surveyed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&account);
    }

    /// The account's turn: one change to its sign-in at a time. Signing in
    /// again and removing the account take it, so neither can land halfway
    /// through the other.
    pub(crate) fn account_turn(&self, account: i64) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.turns
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry(account)
                .or_default(),
        )
    }

    /// A draft whose push waits for the account to sign in.
    pub(crate) fn hold_draft(&self, account: i64, draft: i64) {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(account)
            .or_default()
            .drafts
            .insert(draft);
    }

    /// Server copies whose removal waits for the account to sign in.
    pub(crate) fn hold_drops(&self, account: i64, copies: Vec<(String, u32)>) {
        let mut all = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let drops = &mut all.entry(account).or_default().drops;
        for copy in copies {
            if !drops.contains(&copy) {
                drops.push(copy);
            }
        }
    }

    /// Everything held for the account, taken: its caller does it now.
    pub(crate) fn take_held(&self, account: i64) -> Held {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&account)
            .unwrap_or_default()
    }

    /// How much longer a refused account waits before its next attempt.
    /// `None` when it may try now: it was never refused, or its hour is up.
    pub(crate) fn refused_wait(&self, account: i64, now: i64) -> Option<Duration> {
        let all = self.signin.lock().unwrap_or_else(|p| p.into_inner());
        match all.get(&account) {
            Some((SignIn::Refused, since)) => {
                let due = since.saturating_add(REFUSED_RETRY.as_millis() as i64);
                (due > now).then(|| Duration::from_millis((due - now) as u64))
            }
            _ => None,
        }
    }

    /// Marks the account's first pass as running, and returns the mark.
    ///
    /// Per account, because the window shows one account and its first sync
    /// is what it is waiting for: one global flag was set at launch and
    /// never again, so an account added later — every account, on a first
    /// run — synced with the window believing nothing was happening. It
    /// never reloaded the folders, identity or saved searches that pass
    /// stored, and said "Inbox is clear" while mail was arriving.
    ///
    /// The mark is what ends it. A pass stopped by an account's removal or
    /// a new password ends late, after the pass that replaced it has made
    /// its own mark, and must not end that one.
    pub(crate) fn mark_seeding(&self, account: i64) -> u64 {
        let mark = self.seeding_marks.fetch_add(1, Ordering::Relaxed) + 1;
        self.seeding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account, mark);
        mark
    }

    /// The first pass that made `mark` is over.
    pub(crate) fn end_seeding(&self, account: i64, mark: u64) {
        let mut all = self.seeding.lock().unwrap_or_else(|p| p.into_inner());
        if all.get(&account) == Some(&mark) {
            all.remove(&account);
        }
    }

    /// Whether the account's first pass is still running.
    pub(crate) fn is_seeding(&self, account: i64) -> bool {
        self.seeding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&account)
    }
}

/// Waits while the account cannot sign in.
///
/// True at once when it can; false if the account was stopped meanwhile.
/// The IDLE watchers and the backfill wait here, so only the sweep loop's
/// hourly question reaches the server while a password is refused.
pub(crate) async fn wait_for_signin(
    state: &AppState,
    account: i64,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    loop {
        // A stopped worker goes no further, signed in or not: a restart
        // clears the state, and the worker it replaced must not read that as
        // leave to sign in once more with the old password.
        if *stop.borrow() {
            return false;
        }
        // Subscribed before looking, so a change between the two is seen.
        let mut changed = state.signin_changed.subscribe();
        if state.signin(account).is_none() {
            return true;
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = stopped(stop) => return false,
        }
    }
}

/// Resolves once the account cannot sign in. An IDLE session armed before a
/// password change outlives it on Dovecot and cPanel, for up to its
/// twenty-minute ceiling; the watchers end theirs here instead, so nothing
/// it wakes asks the server again with the refused password.
pub(crate) async fn until_signed_out(state: &AppState, account: i64) {
    let mut changed = state.signin_changed.subscribe();
    loop {
        if state.signin(account).is_some() {
            return;
        }
        if changed.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Puts a sync error on the banner, unless the worker saying it has been told
/// to stop: the same fence as `refused_by`, for the same late answer.
pub(crate) fn say_sync_error(state: &AppState, stop: &Receiver<bool>, text: Option<String>) {
    let mut banner = state.sync_error.lock().unwrap_or_else(|p| p.into_inner());
    if !*stop.borrow() {
        *banner = text;
    }
}

/// The account's server, for a command that has to reach it: `Ok(None)` for
/// an account with no server at all, whose mail is Petrel's own, and
/// `SIGN_IN_FIRST` while it cannot sign in.
///
/// An account with servers and no password Petrel can read is not a local
/// account. It was treated as one: Empty Trash tombstoned everything here and
/// said it was gone while the server kept it, and folder changes were made
/// here only. It is marked as needing its password instead.
pub(crate) fn server_for(
    state: &AppState,
    store: &Store,
    account: i64,
) -> Result<Option<ImapConfig>, String> {
    if state.signin(account).is_some() {
        return Err(SIGN_IN_FIRST.into());
    }
    let has_servers = store
        .account_servers(account)
        .ok()
        .flatten()
        .is_some_and(|s| !s.imap_host.is_empty());
    if !has_servers {
        return Ok(None);
    }
    match crate::config::imap_config_for(store, account) {
        Some(cfg) => Ok(Some(cfg)),
        None => {
            state.set_signin(account, SignIn::Missing);
            Err(SIGN_IN_FIRST.into())
        }
    }
}

/// A command's failure, marking the account refused when the server refused
/// the password at sign-in. The command then says "sign in again first"
/// rather than the server's words. Told by the error's type, never its
/// words: any other failure, however it is worded, is only itself.
pub(crate) fn refused_or(
    state: &AppState,
    account: i64,
    stop: &Receiver<bool>,
    error: petrel_providers::imap::ImapError,
) -> String {
    if error.is_sign_in_refused() {
        state.refused_by(account, stop);
        return SIGN_IN_FIRST.into();
    }
    error.to_string()
}

/// Splits the accounts that have servers into those Petrel can sign in to
/// and those whose password it cannot read.
///
/// The second kind used to drop out of the launch without a word: the
/// window showed "Inbox is clear" over an account that would never sync,
/// with no banner and nothing to press.
pub(crate) fn split_by_password<S, C>(
    rows: Vec<(i64, S)>,
    mut config_for: impl FnMut(i64, S) -> Option<C>,
) -> (Vec<(i64, C)>, Vec<i64>) {
    let mut ready = Vec::new();
    let mut missing = Vec::new();
    for (id, servers) in rows {
        match config_for(id, servers) {
            Some(cfg) => ready.push((id, cfg)),
            None => missing.push(id),
        }
    }
    (ready, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_state;
    use std::sync::Arc;

    #[test]
    fn a_refusal_holds_the_account_for_an_hour_and_no_longer() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        assert_eq!(state.signin(7), None);
        assert_eq!(
            state.refused_wait(7, now_ms()),
            None,
            "never refused: may try"
        );
        state.set_signin(7, SignIn::Refused);
        let since = now_ms();
        let wait = state.refused_wait(7, since).expect("waits after a refusal");
        assert!(wait > Duration::from_secs(59 * 60) && wait <= REFUSED_RETRY);
        let after_the_hour = since + REFUSED_RETRY.as_millis() as i64 + 1_000;
        assert_eq!(
            state.refused_wait(7, after_the_hour),
            None,
            "the hour is up: one attempt"
        );
        // A missing password is not on a clock: it waits for one.
        state.set_signin(8, SignIn::Missing);
        assert_eq!(state.refused_wait(8, now_ms()), None);
        assert_eq!(state.signin(8), Some(SignIn::Missing));
        state.clear_signin(7);
        assert_eq!(state.signin(7), None);
        assert_eq!(state.signin(8), Some(SignIn::Missing), "per account");
    }

    #[test]
    fn a_worker_waits_while_signed_out_and_resumes_when_signed_in() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let mut stop = state.stop_signal(3);
        tauri::async_runtime::block_on(async {
            let quick = Duration::from_secs(2);
            assert!(
                tokio::time::timeout(quick, wait_for_signin(&state, 3, &mut stop))
                    .await
                    .expect("nothing to wait for"),
            );
            state.set_signin(3, SignIn::Refused);
            let held = tokio::time::timeout(
                Duration::from_millis(300),
                wait_for_signin(&state, 3, &mut stop),
            )
            .await;
            assert!(held.is_err(), "held while refused");

            let later = Arc::clone(&state);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                later.clear_signin(3);
            });
            assert!(
                tokio::time::timeout(quick, wait_for_signin(&state, 3, &mut stop))
                    .await
                    .expect("resumes once signed in"),
            );

            state.set_signin(3, SignIn::Missing);
            let later = Arc::clone(&state);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                later.stop_workers(3);
            });
            assert!(
                !tokio::time::timeout(quick, wait_for_signin(&state, 3, &mut stop))
                    .await
                    .expect("ends when the account stops"),
            );
        });
    }

    /// A pass stopped by a new password ends after the pass that replaced
    /// it began, and must not end that one's mark.
    #[test]
    fn a_first_pass_ends_only_its_own_mark() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        assert!(!state.is_seeding(1));
        let stopped_pass = state.mark_seeding(1);
        let fresh_pass = state.mark_seeding(1);
        state.end_seeding(1, stopped_pass);
        assert!(
            state.is_seeding(1),
            "the stopped pass ended the fresh one's mark"
        );
        assert!(!state.is_seeding(2), "one account's pass is not another's");
        state.end_seeding(1, fresh_pass);
        assert!(!state.is_seeding(1));
    }

    /// docs/25 #84: an account added on a first run synced while the window
    /// was told nothing was happening. Seeding was one flag, set when the app
    /// launched and never again, so the window never re-read the folders,
    /// identity and saved searches that pass stored. Every first pass is
    /// marked now, from the moment it is spawned, and ends its own mark.
    #[test]
    fn a_first_pass_is_seeding_from_the_moment_it_is_spawned() {
        use petrel_providers::imap::{Credential, ImapConfig, Security};
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        // A port nothing listens on: the pass fails at once, without a network.
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let cfg = ImapConfig {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            credential: Credential::password("p"),
            security: Security::Tls,
        };
        tauri::async_runtime::block_on(async {
            assert!(!state.is_seeding(account));
            crate::sync::spawn_real_sync(Arc::clone(&state), account, cfg);
            assert!(state.is_seeding(account), "marked before anything ran");
            let started = std::time::Instant::now();
            while state.is_seeding(account) && started.elapsed() < Duration::from_secs(20) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let still = state.is_seeding(account);
            state.stop_workers(account);
            assert!(!still, "the first pass ends its own mark");
        });
    }

    #[test]
    fn an_account_without_a_readable_password_is_set_aside_not_dropped() {
        let rows = vec![(1, "has"), (2, "none"), (3, "has")];
        let (ready, missing) = split_by_password(rows, |id, s| (s == "has").then_some(id * 10));
        assert_eq!(ready, vec![(1, 10), (3, 30)]);
        assert_eq!(missing, vec![2]);
    }
}

/// Test support shared by the signed-out tests in this crate.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::state::AppState;
    use petrel_engine::store::AccountServers;

    /// An account whose id no keychain item can have.
    ///
    /// A cache miss falls through to the keychain, and with no store named
    /// that is the legacy `account-N` item: a developer's machine may hold a
    /// real password there. Ids this high were never handed out, so a read
    /// finds nothing, and finds it without a consent dialog.
    pub(crate) fn unkeyed_account(state: &AppState) -> i64 {
        let store = state.store().unwrap();
        loop {
            let id = store.ensure_test_account().unwrap();
            if id >= 9_000 {
                return id;
            }
        }
    }

    /// Servers on `account` pointing at a local port: the configuration
    /// built from them is TLS, so a server there counts connections.
    pub(crate) fn servers_at(state: &AppState, account: i64, port: u16) {
        state
            .store()
            .unwrap()
            .set_account_servers(
                account,
                &AccountServers {
                    imap_host: "127.0.0.1".into(),
                    imap_port: port,
                    smtp_host: "127.0.0.1".into(),
                    smtp_port: port,
                    username: "u".into(),
                    provider: String::new(),
                },
            )
            .unwrap();
    }
}

#[cfg(all(test, feature = "dev-plaintext-imap"))]
pub(crate) mod refused_password_tests {
    //! The stand-down, measured against a server that counts every LOGIN
    //! (`scripted_imap`). The loop's clocks come from the environment, which
    //! every test in the process shares, so each of these sets the same two
    //! values (`fast_clocks`) and none can slow another down.
    use crate::scripted_imap::{Srv, raw, serve};
    use crate::signin::SignIn;
    use crate::state::{AppState, test_state};
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_providers::imap::{Credential, ImapConfig, Security};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A 15-second poll and a 30-second sweep, the shortest the loop allows.
    pub(crate) fn fast_clocks() {
        unsafe {
            std::env::set_var("PETREL_POLL_SECONDS", "15");
            std::env::set_var("PETREL_SWEEP_SECONDS", "30");
        }
    }

    pub(crate) fn plain(port: u16, pass: &str) -> ImapConfig {
        ImapConfig {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            credential: Credential::password(pass),
            security: Security::InsecurePlaintext,
        }
    }

    /// INBOX and Archive in the store, `n` messages in INBOX on both sides,
    /// and an archive of each queued locally: what a person triaging while
    /// the account is signed out leaves behind, since the drain waits for a
    /// password that works and the queue grows meanwhile.
    pub(crate) fn queued_archives(state: &AppState, srv: &Srv, n: u32) {
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
        let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
        store.set_backfill_floor(inbox, 1).unwrap();
        for uid in 1..=n {
            srv.put("INBOX", uid, raw(uid));
            store
                .ingest_raw(&state.blobs, account, Some(inbox), Some(uid), &raw(uid))
                .unwrap();
            let id = store.message_id_at(inbox, uid).unwrap().unwrap();
            let thread = store.thread_of(id).unwrap().unwrap_or(-id);
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
    }

    /// docs/25 #93, and the review's probe A. A launch whose password the
    /// server refuses, with ten archives queued from before.
    ///
    /// Nothing used to stand the account down until the first pass's sync
    /// cycle had failed as well, and between the two the first pass drained
    /// the queue: a connection per archive, and a second for the "make the
    /// folder and try again" path. Twenty-two refused sign-ins in a fifth of
    /// a second, at every launch, which is what gets a home network banned.
    /// The launch's own probe is the one sign-in now.
    #[test]
    fn a_launch_with_a_refused_password_signs_in_once_however_much_is_queued() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let srv = Srv::new(
            "MOVE UIDPLUS",
            &[("INBOX", ""), ("Archive", "\\Archive")],
            &[],
        );
        queued_archives(&state, &srv, 10);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            let t0 = Instant::now();
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "revoked"));
            // Past the first 15-second poll, which used to sign in twice more.
            tokio::time::sleep(Duration::from_secs(17)).await;
            let refused = srv.refused_since(t0);
            let why = state.signin(account);
            let banner = state.sync_error.lock().unwrap().clone();
            state.stop_workers(account);
            assert_eq!(why, Some(SignIn::Refused));
            assert_eq!(refused, 1, "refused sign-ins: {}", srv.timeline());
            assert!(
                banner.is_some_and(|b| b.contains("Sign-in was refused")),
                "the banner says why"
            );
            let queued = state
                .store
                .lock()
                .unwrap()
                .pending_actions(account)
                .unwrap()
                .len();
            assert_eq!(queued, 10, "the archives wait for a password that works");
        });
    }

    /// The review's probe B. The IDLE watcher meets the refusal while the
    /// loop waits for its sweep.
    ///
    /// The loop looked at the sign-in state only at the top, before its
    /// wait, so when the sweep timer fired it ran a whole cycle with the
    /// refused password anyway: the queue drained as refused sign-ins, then
    /// the survey and the sync. Twenty-two of them, after the account had
    /// been marked.
    #[test]
    fn the_loop_runs_no_cycle_once_a_watcher_has_stood_the_account_down() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let srv = Srv::new(
            "IDLE MOVE UIDPLUS",
            &[("INBOX", ""), ("Archive", "\\Archive")],
            &["good"],
        );
        state
            .store
            .lock()
            .unwrap()
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Archive".into(), Some("archive".into())),
                ],
            )
            .unwrap();
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "good"));
            // The first pass over, IDLE armed.
            tokio::time::sleep(Duration::from_secs(4)).await;
            assert!(!state.is_seeding(account), "the first pass is over");
            // The provider changes the password and ends its sessions.
            srv.accept.lock().unwrap().clear();
            srv.epoch.send_modify(|e| *e += 1);
            // The person carries on working; the archives queue.
            queued_archives(&state, &srv, 10);
            let mut marked = None;
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if state.signin(account) == Some(SignIn::Refused) {
                    marked = Some(Instant::now());
                    break;
                }
            }
            let marked = marked.expect("the IDLE watcher's reconnect was refused");
            // Past the sweep timer, 30 seconds from the end of the first pass.
            tokio::time::sleep(Duration::from_secs(32)).await;
            let after = srv.refused_since(marked);
            state.stop_workers(account);
            assert_eq!(after, 0, "refused after the mark: {}", srv.timeline());
        });
    }

    /// The review's probe G. Exchange, Courier and Zimbra refuse a password
    /// with `NO LOGIN failed.`, which carries none of the words the refusal
    /// used to be recognised by: the account was never stood down, and was
    /// asked twice a cycle for as long as the app ran.
    #[test]
    fn a_refusal_in_the_servers_own_words_stands_the_account_down() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        state
            .store
            .lock()
            .unwrap()
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Sent".into(), Some("sent".into())),
                ],
            )
            .unwrap();
        let srv = Srv::new("UIDPLUS", &[("INBOX", ""), ("Sent", "\\Sent")], &[]);
        *srv.refusal.lock().unwrap() = "NO LOGIN failed.".into();
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            let t0 = Instant::now();
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "revoked"));
            tokio::time::sleep(Duration::from_secs(17)).await;
            let refused = srv.refused_since(t0);
            let why = state.signin(account);
            state.stop_workers(account);
            assert_eq!(why, Some(SignIn::Refused), "stood down");
            assert_eq!(refused, 1, "{}", srv.timeline());
        });
    }

    /// Mail queued while signed out, and the hour coming up: the loop asks
    /// once, and once the server takes the password it catches up at once,
    /// queue first.
    #[test]
    fn signed_in_again_the_account_catches_up_at_once() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let srv = Srv::new(
            "MOVE UIDPLUS",
            &[("INBOX", ""), ("Archive", "\\Archive")],
            &[],
        );
        queued_archives(&state, &srv, 2);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "pass"));
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert_eq!(state.signin(account), Some(SignIn::Refused));
            // The server takes the password after all, and the hour is up.
            srv.accept.lock().unwrap().insert("pass".into());
            state.signin.lock().unwrap().insert(
                account,
                (SignIn::Refused, crate::state::now_ms() - 2 * 3_600_000),
            );
            state.signin_changed.send_modify(|n| *n += 1);
            let mut drained = false;
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if state
                    .store
                    .lock()
                    .unwrap()
                    .pending_actions(account)
                    .unwrap()
                    .is_empty()
                {
                    drained = true;
                    break;
                }
            }
            let why = state.signin(account);
            state.stop_workers(account);
            assert_eq!(why, None, "signed in");
            assert!(drained, "the queue went as soon as the password worked");
        });
    }

    fn inbox_and_sent(state: &AppState) {
        state
            .store
            .lock()
            .unwrap()
            .sync_folders(
                state.account_id,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Sent".into(), Some("sent".into())),
                ],
            )
            .unwrap();
    }

    /// The second review's R2-B. Gmail answers a sign-in over its
    /// fifteen-connection cap with `NO [ALERT] Too many simultaneous
    /// connections.`: the password is right, and the next sign-in, once
    /// another client lets a connection go, is taken. Any NO at sign-in used
    /// to read as a refused password, and the launch stood the account down
    /// for an hour, telling the person to make a new app password.
    #[test]
    fn a_connection_cap_at_sign_in_does_not_stand_the_account_down() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        inbox_and_sent(&state);
        let srv = Srv::new(
            "IDLE UIDPLUS",
            &[("INBOX", ""), ("Sent", "\\Sent")],
            &["app-pass"],
        );
        *srv.transient.lock().unwrap() =
            "NO [ALERT] Too many simultaneous connections. (Failure)".into();
        srv.refuse_next
            .store(1, std::sync::atomic::Ordering::SeqCst);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "app-pass"));
            // Past the first 15-second poll.
            tokio::time::sleep(Duration::from_secs(19)).await;
            let why = state.signin(account);
            let taken = srv.logins.lock().unwrap().iter().filter(|l| l.1).count();
            let timeline = srv.timeline();
            state.stop_workers(account);
            assert_eq!(why, None, "stood down over a connection cap: {timeline}");
            assert!(
                taken > 0,
                "the password works, and was asked again: {timeline}"
            );
        });
    }

    /// The second review's R2-C. The password changes at the provider while
    /// backfill is walking history; Dovecot and cPanel keep open sessions, so
    /// the IDLE watcher stays connected and the loop waits for its sweep.
    /// Backfill met the refusal and retried on its own 2, 4, 8 … second
    /// backoff, never standing the account down: six refused sign-ins in a
    /// minute, past fail2ban's five, before Petrel said anything.
    #[test]
    fn backfill_stands_the_account_down_at_its_first_refusal() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        inbox_and_sent(&state);
        let srv = Srv::new(
            "IDLE UIDPLUS",
            &[("INBOX", ""), ("Sent", "\\Sent")],
            &["good"],
        );
        // History still being walked: the newest message is held, the
        // hundred thousand below it are not yet.
        {
            let mut store = state.store.lock().unwrap();
            let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
            srv.put("INBOX", 100_000, raw(100_000));
            store
                .ingest_raw(
                    &state.blobs,
                    account,
                    Some(inbox),
                    Some(100_000),
                    &raw(100_000),
                )
                .unwrap();
        }
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "good"));
            tokio::time::sleep(Duration::from_secs(5)).await;
            assert!(!state.is_seeding(account), "the first pass is over");
            // The password changes at the provider; open sessions stay up.
            srv.accept.lock().unwrap().clear();
            let changed = Instant::now();
            // Short of the loop's 30-second sweep: what is refused here is
            // backfill's alone.
            tokio::time::sleep(Duration::from_secs(20)).await;
            let refused = srv.refused_since(changed);
            let why = state.signin(account);
            let timeline = srv.timeline();
            state.stop_workers(account);
            assert_eq!(why, Some(SignIn::Refused), "stood down: {timeline}");
            assert_eq!(refused, 1, "refused sign-ins after the change: {timeline}");
        });
    }

    /// The second review's R2-E. The folder on screen kept signing in after
    /// the account had stood down: its IDLE outlives a password change on
    /// Dovecot for up to twenty minutes, and every new message in the folder
    /// ran a one-folder cycle with the refused password.
    #[test]
    fn the_folder_on_screen_signs_in_nothing_once_the_account_has_stood_down() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let lists = {
            let mut store = state.store.lock().unwrap();
            store
                .sync_folders(
                    account,
                    &[
                        ("INBOX".into(), Some("inbox".into())),
                        ("Lists".into(), None),
                    ],
                )
                .unwrap();
            store
                .folders(account)
                .unwrap()
                .into_iter()
                .find(|f| f.path == "Lists")
                .map(|f| f.id)
                .expect("Lists stored")
        };
        let srv = Srv::new("IDLE UIDPLUS", &[("INBOX", ""), ("Lists", "")], &["good"]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "good"));
            tokio::time::sleep(Duration::from_secs(3)).await;
            // The person opens Lists; the folder watch idles there.
            state
                .open_view
                .send_replace(Some((account, format!("folder:{lists}"))));
            tokio::time::sleep(Duration::from_secs(2)).await;
            // The password changes at the provider; open sessions stay up.
            srv.accept.lock().unwrap().clear();
            // New mail in the inbox: the loop's wake cycle is refused.
            *srv.push_to.lock().unwrap() = "INBOX".into();
            srv.push.send_modify(|n| *n += 1);
            let mut marked = None;
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if state.signin(account).is_some() {
                    marked = Some(Instant::now());
                    break;
                }
            }
            let marked = marked.expect("the inbox wake stood the account down");
            // Four messages arrive in Lists, the folder on screen.
            *srv.push_to.lock().unwrap() = "Lists".into();
            for _ in 0..4 {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                srv.push.send_modify(|n| *n += 1);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            let after = srv.refused_since(marked);
            let timeline = srv.timeline();
            state.stop_workers(account);
            assert_eq!(
                after, 0,
                "refused sign-ins after the stand-down: {timeline}"
            );
        });
    }

    /// The second review's finding 5. What waited for the account to sign in
    /// went only after a cycle in which every folder succeeded: one folder
    /// that fails every cycle stranded it for the session. It goes now when
    /// signing in works again, whatever the folders do.
    #[test]
    fn held_work_goes_when_signing_in_works_again_whatever_the_folders_do() {
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        // Servers in the store, so an id no keychain item has: a password
        // missing from the cache must not be looked for in a real one.
        let account = crate::signin::test_support::unkeyed_account(&state);
        // A folder the server lists and will not open: every cycle fails
        // there.
        state
            .store
            .lock()
            .unwrap()
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Drafts".into(), Some("drafts".into())),
                    ("Locked".into(), None),
                ],
            )
            .unwrap();
        let srv = Srv::new(
            "UIDPLUS",
            &[("INBOX", ""), ("Drafts", "\\Drafts"), ("Locked", "")],
            &[],
        );
        srv.locked.lock().unwrap().insert("Locked".into());
        let _turn = crate::config::cache_turn();
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            // The store's own servers point here too: a held draft's push
            // builds its configuration from them (TLS), which this server
            // counts as a hello.
            crate::signin::test_support::servers_at(&state, account, port);
            crate::config::remember_password(account, "pass");
            let draft = state
                .store
                .lock()
                .unwrap()
                .save_draft_full(
                    account,
                    None,
                    "dana@example.com",
                    "",
                    "Numbers",
                    "words",
                    "",
                    &petrel_engine::store::DraftEnvelope::default(),
                )
                .unwrap();
            crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port, "pass"));
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert_eq!(state.signin(account), Some(SignIn::Refused));
            // Signed out, the draft's push waits.
            crate::sync::drafts::push_draft_to_server(&state, draft)
                .await
                .unwrap();
            assert_eq!(srv.tls_hellos.load(std::sync::atomic::Ordering::SeqCst), 0);
            // The server takes the password after all, and the hour is up.
            srv.accept.lock().unwrap().insert("pass".into());
            state.signin.lock().unwrap().insert(
                account,
                (SignIn::Refused, crate::state::now_ms() - 2 * 3_600_000),
            );
            state.signin_changed.send_modify(|n| *n += 1);
            let mut pushed = false;
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if srv.tls_hellos.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                    pushed = true;
                    break;
                }
            }
            let why = state.signin(account);
            state.stop_workers(account);
            crate::config::forget_password(account);
            assert_eq!(why, None, "signed in");
            assert!(pushed, "the held draft went once the password worked");
        });
    }
}
