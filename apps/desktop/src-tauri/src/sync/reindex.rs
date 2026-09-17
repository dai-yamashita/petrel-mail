//! Re-extraction that does not sit in front of receiving mail.
//!
//! A change to the extractor used to walk every held message before the
//! first IMAP probe. On a mailbox of hundreds of thousands that is half an
//! hour of lock time, and new mail waits behind it. A few newest-first
//! slices repair what is on screen; the rest yields like backfill.

use crate::diag::log_sync;
use crate::state::{AppState, stopped};
use crate::sync::backfill::yield_to_user;
use petrel_engine::store::ReindexProgress;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const SLICE: usize = 250;
/// A thousand newest rows: enough for the inbox, not enough to delay probe.
const STARTUP_SLICES: usize = 4;

/// Newest-first slices so the list on screen is repaired, then return.
/// `true` means the whole mailbox is done and there is nothing to spawn.
pub(crate) async fn run_startup(
    state: &Arc<AppState>,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let mut rewritten = 0usize;
    for _ in 0..STARTUP_SLICES {
        if *stop.borrow() {
            return false;
        }
        match one_slice(state) {
            None => return false,
            Some(p) if p.finished => {
                finish(state, rewritten.saturating_add(p.rewritten)).await;
                return true;
            }
            Some(p) => rewritten = rewritten.saturating_add(p.rewritten),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if rewritten > 0 {
        state.extraction_gen.fetch_add(1, Ordering::Relaxed);
        log_sync(&format!(
            "re-indexed {rewritten} recent message(s); the rest continues in the background"
        ));
    }
    false
}

/// The rest of the mailbox, on its own clock, after the first fetch.
///
/// One task, however many accounts are signed in. `spawn_real_sync` runs per
/// account, so a second mailbox signing in used to put a second walker on the
/// same cursor: not repeating the first one's rows, but interleaving with them,
/// and both reaching the end to bump the generation and checkpoint. The claim
/// is released however the task leaves, so an account that signs in later still
/// finds the work available.
pub(crate) fn spawn_remainder(state: Arc<AppState>, stop: tokio::sync::watch::Receiver<bool>) {
    if state
        .reindexing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        walk_the_rest(&state, stop).await;
        state.reindexing.store(false, Ordering::Release);
    });
}

async fn walk_the_rest(state: &Arc<AppState>, mut stop: tokio::sync::watch::Receiver<bool>) {
    {
        let mut rewritten = 0usize;
        let mut since_checkpoint = 0usize;
        loop {
            if *stop.borrow() {
                return;
            }
            yield_to_user(state).await;
            match one_slice(state) {
                None => return,
                Some(p) if p.finished => {
                    finish(state, rewritten.saturating_add(p.rewritten)).await;
                    return;
                }
                Some(p) => {
                    rewritten = rewritten.saturating_add(p.rewritten);
                    since_checkpoint = since_checkpoint.saturating_add(p.rewritten);
                    if since_checkpoint >= 2_000 {
                        checkpoint_wal(state).await;
                        since_checkpoint = 0;
                    }
                    let nap = if p.rewritten > 0 {
                        std::time::Duration::from_millis(200)
                    } else {
                        std::time::Duration::from_millis(50)
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(nap) => {}
                        _ = stopped(&mut stop) => return,
                    }
                }
            }
        }
    }
}

fn one_slice(state: &AppState) -> Option<ReindexProgress> {
    match state.store.lock() {
        Ok(mut store) => match store.reindex_batch(&state.blobs, SLICE) {
            Ok(p) => Some(p),
            Err(e) => {
                log_sync(&format!("re-index failed: {e}"));
                None
            }
        },
        Err(_) => None,
    }
}

async fn finish(state: &AppState, rewritten: usize) {
    if rewritten > 0 {
        log_sync(&format!(
            "re-indexed {rewritten} message(s) after an extraction change"
        ));
        state.extraction_gen.fetch_add(1, Ordering::Relaxed);
        checkpoint_wal(state).await;
    }
}

async fn checkpoint_wal(state: &AppState) {
    for _ in 0..20 {
        match state.checkpoint_wal_truncate() {
            Ok(r) if r.busy => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Ok(r) => {
                log_sync(&format!(
                    "wal checkpoint after re-index: busy={} log={} checkpointed={}",
                    r.busy, r.log, r.checkpointed
                ));
                return;
            }
            Err(e) => {
                log_sync(&format!("wal checkpoint after re-index: {e}"));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    /// One walk, however many accounts sign in.
    ///
    /// `spawn_real_sync` runs per account and each copy used to spawn its own
    /// walker. They share a cursor, so they interleave rather than repeat, and
    /// both reach the end — two generation bumps, two checkpoints, and twice
    /// the pressure on the one lock every listing also wants.
    #[test]
    fn only_one_account_claims_the_walk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = crate::state::test_state(dir.path());

        assert!(!state.reindexing.load(Ordering::Acquire), "claimed already");
        let first =
            state
                .reindexing
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
        assert!(first.is_ok(), "the first account must get the walk");
        let second =
            state
                .reindexing
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
        assert!(second.is_err(), "a second account must not start another");

        // Released however the walk ends, so a mailbox added later still finds
        // the work available rather than a flag nobody ever cleared.
        state.reindexing.store(false, Ordering::Release);
        let later =
            state
                .reindexing
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
        assert!(later.is_ok(), "the claim was never given back");
    }
}
