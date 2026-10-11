//! Sending: one attempt at a time, the outbox clock that schedules them, and the reconciliation that decides what an uncertain outcome was.

use crate::commands::compose::guess_content_type;
use crate::config::{imap_config_from_env, imap_config_from_servers};
use crate::diag::{friendly_sync_error_for, log_sync, without_addresses};
use crate::state::{AppState, now_ms, stopped};
use crate::sync::drafts::drop_server_draft_using;
use petrel_engine::store::{Identity, Store};
use std::sync::Arc;
use std::sync::{MutexGuard, TryLockError};

/// The address mail goes out as: the account's own, never its login name.
///
/// The login is whatever the provider wants typed into the password box —
/// an Apple ID, a university id, a cPanel `user@host` — and it was being put
/// in `MAIL FROM` and the `From:` header for everyone whose provider does
/// not simply use the address. The identity holds the address the account
/// was set up with; the login is only for AUTH, and only stands in here when
/// the identity somehow has no address at all.
/// What an outbox row records about a failure before anything was
/// committed: the stage, then the server's words.
fn record_failure(stage: &str, detail: &str) -> String {
    format!("{stage}: {detail}")
}

/// Whether a recorded failure is the server refusing the password. The
/// sign-in banner is raised on this, so it is the one string the send worker
/// and the outbox row have to agree on.
fn is_refused_password(recorded: &str) -> bool {
    recorded.starts_with("auth:")
}

/// How a row's reason starts when the message names someone it cannot be
/// sent to. The entries follow, so the person can see which one to fix. The
/// Outbox says it in the person's language (`heldReason` in the UI), matching
/// these words, so they change together.
const NOT_AN_ADDRESS: &str = "not an address";

/// A row's reason when the message names nobody at all. Matched by the
/// Outbox, as `NOT_AN_ADDRESS` is.
const NO_RECIPIENT: &str = "a message needs at least one recipient";

/// A recorded failure as the log may keep it. A server's words lose their
/// addresses; entries refused before sending lose everything, because what
/// is not an address is usually a name.
fn loggable(detail: &str) -> String {
    if detail.starts_with(NOT_AN_ADDRESS) {
        NOT_AN_ADDRESS.to_string()
    } else {
        without_addresses(detail)
    }
}

pub(crate) fn sender_address(identity: Option<&Identity>, login: &str) -> String {
    identity
        .map(|i| i.address.trim())
        .filter(|a| !a.is_empty())
        .unwrap_or(login)
        .to_string()
}

/// The domain a Message-ID is minted under: the sender's, as the RFC has it.
pub(crate) fn address_domain(address: &str) -> String {
    address
        .split('@')
        .nth(1)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or("localhost")
        .to_string()
}

/// Splits a recipient field into its entries: at a comma, a semicolon or a
/// line break, but never inside a quoted name or inside `<…>`. Each entry is
/// trimmed, and blanks are dropped.
///
/// The composer's rule (`splitRecipients` in the UI), so the send goes to the
/// recipients the person saw as chips. Split at every comma, the chip
/// `"Wu, Dana" <dana@example.com>` was two entries, and `"Wu` is no address:
/// the message was refused, and it never went. A line break is where a column
/// pasted one address to a line divides. A quote or a `<` protects only when
/// it closes, and a `<` closes at the next `>`, before any other `<`: one
/// typed and never closed made a single entry of every recipient after it.
pub(crate) fn split_recipients(field: &str) -> Vec<String> {
    let chars: Vec<char> = field.chars().collect();
    // Where a quote or a `<` opened that never closed: read as a plain
    // character from then on. Each pass finds at most one more.
    let mut plain = std::collections::HashSet::new();
    loop {
        match split_pass(&chars, &plain) {
            Ok(entries) => {
                return entries
                    .iter()
                    .map(|e| e.trim())
                    .filter(|e| !e.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            Err(unclosed) => {
                plain.insert(unclosed);
            }
        }
    }
}

/// One reading of a recipient field: its entries, or where the first quote
/// or `<` that never closes was opened.
fn split_pass(
    chars: &[char],
    plain: &std::collections::HashSet<usize>,
) -> Result<Vec<String>, usize> {
    let mut entries = Vec::new();
    let mut entry = String::new();
    let (mut quote_at, mut bracket_at): (Option<usize>, Option<usize>) = (None, None);
    for (i, &c) in chars.iter().enumerate() {
        if plain.contains(&i) {
            entry.push(c);
            continue;
        }
        match c {
            '"' => quote_at = if quote_at.is_none() { Some(i) } else { None },
            '<' if quote_at.is_none() => {
                // An address holds no `<`, so the one before never closed.
                if let Some(at) = bracket_at {
                    return Err(at);
                }
                bracket_at = Some(i);
            }
            '>' if quote_at.is_none() => bracket_at = None,
            ',' | ';' | '\n' | '\r' if quote_at.is_none() && bracket_at.is_none() => {
                entries.push(std::mem::take(&mut entry));
                continue;
            }
            _ => {}
        }
        entry.push(c);
    }
    if let Some(at) = quote_at.or(bracket_at) {
        return Err(at);
    }
    entries.push(entry);
    Ok(entries)
}

/// The store lock, without pinning a tokio worker on `Mutex::lock`.
///
/// Ingest and backfill hold this lock across FTS and fsync. A send that
/// blocked on `lock()` sat in `Transmitting` with no SMTP socket and no
/// timeout, because the worker was stuck in the kernel and could not poll
/// the SMTP deadlines. Yielding between tries lets those deadlines run, and
/// lets the inbox keep answering.
async fn wait_store(state: &AppState) -> Option<MutexGuard<'_, Store>> {
    for i in 0..500 {
        match state.store.try_lock() {
            Ok(g) => return Some(g),
            Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => {}
        }
        if i == 0 {
            log_sync("outbox: store is busy, waiting");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    log_sync("outbox: store stayed busy");
    None
}

/// Sends whatever is due, without waiting for IMAP drain.
///
/// Send now used to raise the drain signal. The drain worker always finished
/// `drain_actions` first, so a mailbox with a backlog of STORE/MOVE held SMTP
/// for as long as that backlog took — two minutes, live, for fifteen actions.
/// This worker is the other half of that split: triage still has its signal;
/// a send wakes this one.
pub(crate) fn spawn_send_worker(
    state: Arc<AppState>,
    account: i64,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let signals = state.outbox_signals(account);
    tauri::async_runtime::spawn(async move {
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                _ = signals.send.notified() => {}
                _ = stopped(&mut stop) => break,
            }
            // Two rows marked due a few milliseconds apart should be one pass.
            // Not the drain's 900ms: that is for triage bursts, and it is the
            // wait this worker exists to avoid.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // As the drain does: mail that is due waits for a password that
            // works, rather than asking the server again with one it refused.
            // It goes the moment signing in works, from the hourly retry or a
            // password entered in Sign in again.
            if !crate::signin::wait_for_signin(&state, account, &mut stop).await {
                break;
            }
            send_due(Arc::clone(&state), account).await;
        }
        log_sync(&format!("account {account}: send worker stopped"));
    });
}

/// Wakes the send worker when a queued message's time comes.
///
/// Nothing else in the system is clock-driven. The send used to ride the
/// drain, and with IDLE the sync loop sleeps until the server pushes
/// something, so a message scheduled for twenty seconds out waited for
/// *unrelated mail to arrive*. Observed on the live account: due at t+20s,
/// still untouched at t+64s.
///
/// Sleeps to the exact instant rather than polling, so an empty outbox costs
/// nothing. A new schedule aborts that sleep via the account's clock signal —
/// without it, a send queued during the empty-outbox nap waited until the nap
/// ended, or until someone pressed Send now. The one-minute cap is for the
/// clock being wrong — a laptop lid closed through the scheduled time.
pub(crate) fn spawn_outbox_clock(
    state: Arc<AppState>,
    account: i64,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let signals = state.outbox_signals(account);
    tauri::async_runtime::spawn(async move {
        loop {
            if *stop.borrow() {
                break;
            }
            let next = wait_store(&state)
                .await
                .and_then(|s| s.next_due_ms(account).ok())
                .flatten();
            let wait_ms = match next {
                Some(at) => (at - now_ms()).clamp(0, 60_000),
                None => 60_000,
            };
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(wait_ms as u64)) => {}
                _ = signals.clock.notified() => continue,
                _ = stopped(&mut stop) => break,
            }
            if next.is_some_and(|at| at <= now_ms()) {
                signals.send.notify_one();
                // Give the send worker its head before asking again, or this
                // loop sees the same due row and fires a second time for nothing.
                tokio::time::sleep(std::time::Duration::from_millis(2_000)).await;
            }
        }
    });
}

/// Builds and sends one message, then files a copy in Sent.
///
/// Shared by the composer and the scheduled-send worker so there is one
/// definition of what sending means — two would drift, and the half that
/// drifted would be the one nobody watches.
/// What one attempt to send came to.
///
/// Carries the transport's verdict rather than collapsing it to an error
/// string, because the four verdicts call for four different responses: done,
/// try again, stop and tell the user, or — the one that matters — *find out*.
struct Attempt {
    /// None when it was refused before there was a message to send.
    message_id: Option<String>,
    outcome: petrel_engine::outbox::AttemptOutcome,
    /// The server's words, for the row to show when it stopped.
    detail: String,
}

/// Sends once and reports what happened. Never retries, never files a copy in
/// Sent on an uncertain outcome — that is the caller's decision to make, with
/// the state machine's help.
///
/// `account` is the account the message was written from, and the only one it
/// may be sent from. It used to be read from `active_account()` instead —
/// whichever account happened to be selected in the window when the undo
/// window ran out. The outbox is per account and `send_due` already knew
/// which one; this threw that away and asked the UI. With two accounts set
/// up, a message written from one went out over the other's SMTP server, as
/// the other's address, carrying the other's signature, and was filed in the
/// other's Sent folder — with every layer reporting success.
/// The password inside a config, for the SMTP fallback that derives its
/// settings from the IMAP host.
///
/// Empty for a token account, which is correct rather than convenient: that
/// fallback exists for the environment-driven developer account, and an
/// account signing in with OAuth has real SMTP settings of its own.
fn cfg_password(cfg: &petrel_providers::imap::ImapConfig) -> &str {
    match &cfg.credential {
        petrel_providers::imap::Credential::Password(p) => p,
        petrel_providers::imap::Credential::Bearer(_) => "",
    }
}

#[allow(clippy::too_many_arguments)]
async fn attempt(
    state: &Arc<AppState>,
    account: i64,
    cfg: &petrel_providers::imap::ImapConfig,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    body: String,
    html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<String>,
) -> Result<Attempt, String> {
    use petrel_engine::outbox::AttemptOutcome;
    use petrel_providers::smtp::{Attachment, Outgoing, SendResult, SmtpConfig, send_tls_with};

    log_sync("outbox: assembling");
    // Servers under the lock, password after it drops: a keychain consent
    // dialog must not pin every other command behind the store.
    let servers = wait_store(state)
        .await
        .and_then(|st| st.account_servers(account).ok().flatten());
    let smtp = servers
        .and_then(|s| crate::config::smtp_config_from_servers(account, s))
        .unwrap_or_else(|| SmtpConfig::for_imap_host(&cfg.host, &cfg.user, cfg_password(cfg)));

    let identity = {
        let store = wait_store(state)
            .await
            .ok_or("could not read the account while the mailbox is busy")?;
        store.identity(account).ok()
    };
    // The account's address, for the envelope and the header alike. The
    // login name is the SMTP config's business and nobody else's.
    let from_addr = sender_address(identity.as_ref(), &cfg.user);
    let domain = address_domain(&from_addr);
    // Read here rather than shuttled through the bridge. A 20MB file becomes a
    // 27MB JSON string on the way across, held twice in memory and slow enough
    // to notice; the path costs nothing and the file is read once.
    let mut files = Vec::new();
    for path in &attachments {
        let p = std::path::Path::new(path);
        let filename = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        // The name, not the path: this text lands in the outbox row and in
        // the log, and a directory is nobody's business but the sender's.
        let bytes =
            std::fs::read(p).map_err(|e| format!("Could not read attachment {filename}: {e}"))?;
        files.push(Attachment {
            filename,
            content_type: guess_content_type(p),
            bytes,
        });
    }

    let msg = Outgoing {
        from_addr,
        from_name: identity.map(|i| i.display_name).unwrap_or_default(),
        to,
        cc,
        bcc,
        subject,
        body_text: body,
        body_html: html.filter(|h| !h.trim().is_empty()),
        in_reply_to,
        references,
        attachments: files,
    };
    // Refused here rather than narrowed to the people who were named
    // properly. At the wire a half-typed address is simply left out, and the
    // person would never learn the message did not reach them; the reason
    // lands on the outbox row instead, as a server's refusal would have. And
    // held there as a refusal, for an edit: the message reads the same on
    // every try, and retried, its row only ever said "Nothing was sent".
    let bad = msg.unsendable();
    let refused = if !bad.is_empty() {
        Some(format!("{NOT_AN_ADDRESS}: {}", bad.join(", ")))
    } else if msg.recipients().is_empty() {
        Some(NO_RECIPIENT.to_string())
    } else {
        None
    };
    if let Some(detail) = refused {
        return Ok(Attempt {
            message_id: None,
            outcome: AttemptOutcome::RejectedPermanently,
            detail,
        });
    }
    let (message_id, raw) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| msg.render(&domain)))
            .map_err(|_| "could not assemble the message".to_string())?;

    log_sync(&format!("outbox: connecting smtp :{}", smtp.port));
    let (outcome, detail) = match send_tls_with(&smtp, &msg, &raw, |s| {
        log_sync(&format!("outbox: smtp {}", without_addresses(s)));
    })
    .await
    {
        SendResult::Committed { response } => (AttemptOutcome::Accepted, response),
        SendResult::RejectedPermanently { response } => {
            log_sync(&format!("send rejected: {}", without_addresses(&response)));
            (AttemptOutcome::RejectedPermanently, response)
        }
        SendResult::FailedBeforeCommit { stage, detail } => {
            log_sync(&format!(
                "send failed at {stage}: {}",
                without_addresses(&detail)
            ));
            (
                AttemptOutcome::FailedBeforeCommit,
                record_failure(stage, &detail),
            )
        }
        SendResult::UnknownAfterTransmit { detail } => {
            // Spike S5's case: the body went, the acknowledgement did not. The
            // message may well have been delivered, so a retry could duplicate
            // it. Reported as exactly that, for the caller to resolve by
            // looking rather than by guessing.
            log_sync(&format!(
                "send outcome unknown: {detail} (message-id {message_id})"
            ));
            (AttemptOutcome::UnknownAfterTransmit, detail)
        }
    };
    if outcome != AttemptOutcome::Accepted {
        return Ok(Attempt {
            message_id: Some(message_id),
            outcome,
            detail,
        });
    }

    // Filed second, and separately: a failure here has not lost the message.
    let sent_path = {
        let store = wait_store(state).await;
        store.and_then(|store| {
            store
                .folder_for_role(account, "sent")
                .ok()
                .flatten()
                .and_then(|fid| store.folder_path(fid).ok().flatten())
        })
    };
    if let Some(path) = sent_path {
        let filed = match tokio::time::timeout(
            std::time::Duration::from_secs(60),
            // The sender's copy: what went out, plus the Bcc header the wire
            // copy leaves off, so Sent still says who was blind-copied.
            petrel_providers::imap::append_message(
                cfg,
                &path,
                Some("(\\Seen)"),
                &msg.sender_copy(&raw),
            ),
        )
        .await
        {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some("timed out".to_string()),
        };
        if let Some(why) = filed {
            // The message went; only the copy did not. Said in the log and
            // to the person: a Sent folder missing what they know they sent
            // is the kind of gap that makes someone send it again. The
            // window carries the words; this only raises the flag.
            log_sync(&format!(
                "sent, but could not file a copy in Sent: {}",
                without_addresses(&why)
            ));
            state.raise_alert("sent-copy-failed");
        }
    }
    log_sync(&format!("sent {message_id}"));
    Ok(Attempt {
        message_id: Some(message_id),
        outcome,
        detail,
    })
}

/// Sends anything in the outbox whose time has come.
///
/// Runs on the same signal as the action drain, so a scheduled send goes out
/// promptly rather than on whatever the sync cadence happens to be. Failures
/// leave the message in the outbox: a send that did not happen is not one to
/// forget about, and the next pass will try again.
/// The retry ladder, in milliseconds: 30s, 2m, 8m, 30m, then hourly.
///
/// Exponential and capped, as the spec asks, and starting short because the
/// common failure is a connection that comes back in seconds. Past the cap
/// there is no giving up: a message waiting for a network that is genuinely
/// away for a day should still go when it returns, not be abandoned because
/// it waited too long.
fn retry_delay_ms(attempts: i64) -> i64 {
    const LADDER: [i64; 4] = [30_000, 120_000, 480_000, 1_800_000];
    LADDER
        .get(attempts.max(1) as usize - 1)
        .copied()
        .unwrap_or(3_600_000)
}

/// Looks in Sent for a message whose send outcome the transport could not
/// report.
///
/// This is the whole of the ambiguous-outcome rule's second half. SMTP has no
/// way to ask "did you get that?", so the answer has to come from evidence:
/// the copy a delivering server places in Sent. Found means it went. Searched
/// and absent means it did not, and a retry is safe. Unable to search — which
/// is the likely case straight after a dropped connection — means nobody
/// knows, and that is the one answer that must reach a person unchanged.
pub(crate) async fn sent_folder_evidence(
    state: &Arc<AppState>,
    cfg: &petrel_providers::imap::ImapConfig,
    account: i64,
    message_id: &str,
) -> petrel_engine::outbox::ServerEvidence {
    use petrel_engine::outbox::ServerEvidence;
    let sent = wait_store(state).await.and_then(|s| {
        s.folder_for_role(account, "sent")
            .ok()
            .flatten()
            .and_then(|fid| s.folder_path(fid).ok().flatten())
    });
    let Some(path) = sent else {
        // No Sent folder known for this account: there is nowhere to look.
        return ServerEvidence::Indeterminate;
    };
    match petrel_providers::imap::find_message_id(cfg, &path, message_id).await {
        Ok(uids) => evidence_from_search(!uids.is_empty(), server_files_sent_copies(cfg)),
        Err(e) => {
            log_sync(&format!("could not check Sent for {message_id}: {e}"));
            ServerEvidence::Indeterminate
        }
    }
}

/// Whether this server places a copy of every submitted message in Sent
/// itself. Gmail does. Almost nobody else does: on a plain IMAP/SMTP host the
/// only copy in Sent is the one Petrel appends after the send is confirmed —
/// which, after an ambiguous send, is exactly the copy that was never made.
fn server_files_sent_copies(cfg: &petrel_providers::imap::ImapConfig) -> bool {
    crate::sync::account_is_gmail(cfg)
}

/// What a Sent search proves. Found is found. Absent means "did not go" only
/// where the server would have filed a copy on its own; anywhere else an
/// empty Sent folder says nothing, and the answer has to reach a person
/// rather than start a retry that may deliver the message twice.
fn evidence_from_search(
    found: bool,
    server_files_sent: bool,
) -> petrel_engine::outbox::ServerEvidence {
    use petrel_engine::outbox::ServerEvidence;
    if found {
        ServerEvidence::Found
    } else if server_files_sent {
        ServerEvidence::Absent
    } else {
        ServerEvidence::Indeterminate
    }
}

/// Sends whatever is due, and records where each message ended up.
///
/// This is the outbox's state machine in motion. Each attempt's outcome goes
/// through `reconcile`, which is the only place that decides what a message's
/// state becomes — and for the uncertain case it decides by *looking*, in the
/// Sent folder, rather than by guessing. A message the engine cannot prove
/// either way is held for a person and is never picked up here again.
pub(crate) async fn send_due(state: Arc<AppState>, account: i64) {
    use petrel_engine::outbox::{AttemptOutcome, SendState, ServerEvidence, reconcile};

    // One pass at a time per account, by construction: the account's send
    // worker is the only caller, and it awaits each pass before taking the
    // next wake-up. Two accounts may transmit at once; they hold different
    // rows.
    let due = {
        let Some(store) = wait_store(&state).await else {
            return;
        };
        store.due_sends(account, now_ms()).unwrap_or_default()
    };
    if due.is_empty() {
        return;
    }
    log_sync(&format!("{} queued message(s) due", due.len()));
    let cfg = {
        let servers = wait_store(&state)
            .await
            .and_then(|s| s.account_servers(account).ok().flatten());
        servers
            .and_then(|s| imap_config_from_servers(account, s))
            .or_else(imap_config_from_env)
    };
    let Some(cfg) = cfg else {
        log_sync("outbox: no account configured, leaving the queue as-is");
        return;
    };

    for queued in due {
        let id = queued.id;
        // Claim first. If the store stays busy we leave the row queued rather
        // than transmit without a durable Transmitting mark, or mark it and
        // then fail to start SMTP. The guard must die before the next await:
        // MutexGuard is not Send.
        //
        // Only if it is still due. The list was read when the pass began, and
        // a message sent before this one can take a while: one undone,
        // discarded or given a new time in the meantime was claimed anyway and
        // sent. And what goes is what the row holds now, not what the pass
        // read: undone, edited and sent again, it is the new text.
        let d = {
            let Some(store) = wait_store(&state).await else {
                log_sync("outbox: store stayed busy, leaving the row queued");
                continue;
            };
            match store.claim_send(id, now_ms()) {
                Ok(true) => {}
                Ok(false) => {
                    log_sync("outbox: a message pulled back since the pass began, left alone");
                    continue;
                }
                Err(e) => {
                    log_sync(&format!(
                        "outbox: could not claim a message, leaving it queued: {e}"
                    ));
                    continue;
                }
            }
            match store.load_draft(id) {
                Ok(d) => d,
                Err(e) => {
                    // Claimed and then unreadable: back in the queue for the
                    // next pass, rather than held as if it were on the wire.
                    log_sync(&format!(
                        "outbox: could not read a claimed message, requeued: {e}"
                    ));
                    let _ = store.resend_now(id, now_ms() + 60_000);
                    continue;
                }
            }
        };
        let to = split_recipients(&d.to);
        let attempts = {
            wait_store(&state)
                .await
                .and_then(|s| s.outbox(account).ok())
                .and_then(|rows| rows.into_iter().find(|r| r.id == id).map(|r| r.attempts))
                .unwrap_or(0)
        };

        let cc = split_recipients(&d.cc);
        let bcc = split_recipients(&d.envelope.bcc);
        // The whole message, not only its text: a reply that waited in the
        // undo window still threads into its conversation, and still carries
        // what was attached to it.
        let result = attempt(
            &state,
            account,
            &cfg,
            to,
            cc,
            bcc,
            d.subject,
            d.body,
            // Stored alongside the text, so a message posted hours ago goes
            // out in the form it was written rather than flattened by the wait.
            Some(d.html).filter(|h| !h.trim().is_empty()),
            d.envelope.in_reply_to,
            d.envelope.references,
            d.envelope.attachments,
        )
        .await;

        let (outcome, detail, message_id) = match result {
            Ok(a) => (a.outcome, a.detail, a.message_id),
            // Could not even assemble the message — a missing attachment, no
            // account. Not a transport verdict, so it is treated as a failure
            // before anything was committed: safe to retry once it is fixed.
            Err(e) => (AttemptOutcome::FailedBeforeCommit, e, None),
        };

        // Evidence is consulted only when the transport could not say.
        let evidence = if outcome == AttemptOutcome::UnknownAfterTransmit {
            match &message_id {
                Some(m) => sent_folder_evidence(&state, &cfg, account, m).await,
                None => ServerEvidence::Indeterminate,
            }
        } else {
            ServerEvidence::Indeterminate
        };
        // A password the server refused is not a transport hiccup: the ladder
        // will keep the message and keep trying, and without this the only
        // sign would be mail that never leaves. The same banner the sync loop
        // raises says what is wrong and where to fix it.
        if is_refused_password(&detail) {
            *state.sync_error.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(friendly_sync_error_for(&cfg.host, &detail));
        }

        let next = reconcile(outcome, evidence);
        log_sync(&format!("outbox: outcome {next:?}"));

        // Must not leave Transmitting with no buttons. Keep asking until the
        // store is free, then write the real outcome rather than guessing.
        let store = loop {
            if let Some(store) = wait_store(&state).await {
                break store;
            }
            log_sync("outbox: could not record the send outcome, retrying");
        };
        match next {
            SendState::Sent => {
                log_sync(&format!(
                    "queued send delivered {}",
                    message_id.as_deref().unwrap_or("?")
                ));
                drop_server_draft_using(&state, &store, id);
                let _ = store.delete_draft(id);
            }
            SendState::RetryQueued => {
                let n = attempts + 1;
                let wait = retry_delay_ms(n);
                log_sync(&format!(
                    "queued send failed, retrying in {}s: {}",
                    wait / 1000,
                    loggable(&detail)
                ));
                let _ = store.set_send_state(
                    id,
                    SendState::RetryQueued,
                    Some(&detail),
                    Some(now_ms() + wait),
                    message_id.as_deref(),
                );
            }
            SendState::FailedPermanent => {
                log_sync(&format!("queued send rejected: {}", loggable(&detail)));
                let _ = store.set_send_state(
                    id,
                    SendState::FailedPermanent,
                    Some(&detail),
                    None,
                    message_id.as_deref(),
                );
            }
            SendState::NeedsAttention => {
                // The one outcome no amount of engineering resolves. Said in
                // the row, and raised as a notification, because silence is
                // the one response that loses mail.
                log_sync(&format!(
                    "queued send needs attention: {}",
                    loggable(&detail)
                ));
                let _ = store.set_send_state(
                    id,
                    SendState::NeedsAttention,
                    Some(&detail),
                    None,
                    message_id.as_deref(),
                );
            }
            SendState::UndoWindow | SendState::Transmitting => {}
        }
    }
}

#[cfg(test)]
mod sender_tests {
    use super::{address_domain, sender_address};
    use petrel_engine::store::Identity;

    fn identity(address: &str) -> Identity {
        Identity {
            address: address.into(),
            display_name: "Me".into(),
            signature: String::new(),
            signature_on_reply: false,
        }
    }

    /// The bug: mail went out as the login name. An Apple ID, a university
    /// id or a cPanel `user@host` is not an address anyone can reply to.
    #[test]
    fn mail_goes_out_as_the_accounts_address_not_its_login() {
        let me = identity("me@example.com");
        assert_eq!(
            sender_address(Some(&me), "apple.id.12345"),
            "me@example.com"
        );
        assert_eq!(
            sender_address(Some(&me), "someone@cpanel.host.example"),
            "me@example.com"
        );
        // And the Message-ID domain follows the address, not the login.
        assert_eq!(
            address_domain(&sender_address(Some(&me), "u123")),
            "example.com"
        );
    }

    #[test]
    fn the_login_stands_in_only_when_there_is_no_address_at_all() {
        assert_eq!(sender_address(None, "me@example.com"), "me@example.com");
        assert_eq!(
            sender_address(Some(&identity("  ")), "me@example.com"),
            "me@example.com"
        );
        assert_eq!(address_domain("nobody"), "localhost");
        assert_eq!(address_domain("x@"), "localhost");
    }
}

#[cfg(test)]
mod evidence_tests {
    use super::evidence_from_search;
    use petrel_engine::outbox::ServerEvidence;

    /// The rule the outbox lives by: absence is proof only where the server
    /// files its own copies. Everywhere else an empty Sent after a dropped
    /// connection means nobody knows, and "nobody knows" reaches a person.
    #[test]
    fn an_empty_sent_folder_is_evidence_only_where_the_server_files_copies() {
        assert_eq!(evidence_from_search(true, false), ServerEvidence::Found);
        assert_eq!(evidence_from_search(true, true), ServerEvidence::Found);
        assert_eq!(evidence_from_search(false, true), ServerEvidence::Absent);
        assert_eq!(
            evidence_from_search(false, false),
            ServerEvidence::Indeterminate
        );
    }
}

#[cfg(test)]
mod auth_banner_tests {
    use super::{is_refused_password, record_failure};

    /// The banner is raised on the recorded string, so the recording and the
    /// check have to agree. If the providers crate ever renames the stage,
    /// or the recording changes shape, this is what notices — otherwise a
    /// refused password goes back to being invisible, with the outbox
    /// quietly retrying against a server that will keep saying no.
    #[test]
    fn a_refused_password_is_recognisable_from_what_the_row_records() {
        let recorded = record_failure("auth", "535 5.7.8 Username and Password not accepted");
        assert!(is_refused_password(&recorded), "{recorded}");
        // A failure at any other stage must not raise it: a refused
        // recipient is about the message, not the account.
        assert!(!is_refused_password(&record_failure(
            "rcpt",
            "550 no such user"
        )));
        assert!(!is_refused_password(&record_failure("connect", "refused")));
    }
}

#[cfg(test)]
mod recipient_tests {
    use super::{loggable, send_due, split_recipients};
    use crate::state::{AppState, now_ms, test_state};
    use petrel_engine::store::{AccountServers, DraftEnvelope, OutboxRow};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// The composer's rule: a comma, a semicolon or a line break ends an
    /// entry, except inside a quoted name or inside `<…>`.
    #[test]
    fn a_field_splits_where_the_composer_splits_it() {
        assert_eq!(
            split_recipients("\"Wu, Dana\" <dana@example.com>, alex@example.com"),
            ["\"Wu, Dana\" <dana@example.com>", "alex@example.com"]
        );
        assert_eq!(
            split_recipients("\"Wu; Dana\" <dana@example.com>; alex@example.com"),
            ["\"Wu; Dana\" <dana@example.com>", "alex@example.com"]
        );
        // A column pasted from a spreadsheet, one to a line.
        assert_eq!(
            split_recipients("ann@example.com\nbob@example.com\r\ncy@example.com"),
            ["ann@example.com", "bob@example.com", "cy@example.com"]
        );
        // A quoted local part is still one address.
        assert_eq!(
            split_recipients("Dana <\"wu,dana\"@example.com>, alex@example.com"),
            ["Dana <\"wu,dana\"@example.com>", "alex@example.com"]
        );
        // Trimmed, and the blanks that trailing separators leave are gone.
        assert_eq!(
            split_recipients("  ann@example.com ,, ;\n bob@example.com , "),
            ["ann@example.com", "bob@example.com"]
        );
        assert!(split_recipients("").is_empty());
        assert!(split_recipients(" ,;\n ").is_empty());
    }

    /// A quote or a `<` that never closes protects nothing: the separators
    /// after it still separate, as the composer's chips do. Kept whole, one
    /// typed `<` made a single entry of every recipient after it. Nothing is
    /// guessed at either way: what is left with the stray character is no
    /// address, so the message is still refused, with its text on the row.
    #[test]
    fn an_unclosed_quote_or_bracket_protects_nothing() {
        let quote = split_recipients("\"Wu, Dana <dana@example.com>, alex@example.com");
        assert_eq!(
            quote,
            ["\"Wu", "Dana <dana@example.com>", "alex@example.com"]
        );
        assert_eq!(
            split_recipients("Dana <dana@example.com, bob@example.com"),
            ["Dana <dana@example.com", "bob@example.com"]
        );
        // A `<` closes at the next `>`, not at one after another `<`.
        assert_eq!(
            split_recipients("Dana <dana@example.com, Bob <bob@example.com>; cy@example.com"),
            [
                "Dana <dana@example.com",
                "Bob <bob@example.com>",
                "cy@example.com"
            ]
        );
        // What closes still holds together.
        assert_eq!(
            split_recipients("Dana <dana,wu@example.com>, Bob <bob@example.com"),
            ["Dana <dana,wu@example.com>", "Bob <bob@example.com"]
        );
        assert_eq!(
            split_recipients("Dana <\"wu,dana\"@example.com>, \"Lee, Al <al@example.com>"),
            [
                "Dana <\"wu,dana\"@example.com>",
                "\"Lee",
                "Al <al@example.com>"
            ]
        );
        assert_eq!(
            petrel_providers::smtp::unsendable_recipients(&quote),
            ["\"Wu"],
            "the stray quote is no address, so nothing goes"
        );
        assert_eq!(
            petrel_providers::smtp::unsendable_recipients(&split_recipients(
                "Dana <dana@example.com, bob@example.com"
            )),
            ["Dana <dana@example.com"]
        );
    }

    /// Everyone the fields name reaches the envelope, and the recipient sees
    /// the name as it was written.
    #[test]
    fn everyone_named_goes_on_the_envelope() {
        let msg = petrel_providers::smtp::Outgoing {
            from_addr: "sam@example.com".into(),
            from_name: "Sam".into(),
            to: split_recipients("\"Wu, Dana\" <dana@example.com>, alex@example.com"),
            cc: split_recipients(""),
            bcc: split_recipients("\"Nair, Priya\" <priya@example.net>"),
            subject: "Numbers".into(),
            body_text: "In.".into(),
            body_html: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        };
        assert!(msg.unsendable().is_empty(), "{:?}", msg.unsendable());
        assert_eq!(
            msg.recipients(),
            ["dana@example.com", "alex@example.com", "priya@example.net"]
        );
        let (_, raw) = msg.render("example.com");
        let parsed = petrel_mime::parse_message(&raw).unwrap();
        assert_eq!(
            parsed.to,
            vec![
                (Some("Wu, Dana".to_string()), "dana@example.com".to_string()),
                (None, "alex@example.com".to_string()),
            ]
        );
        assert!(parsed.bcc.is_empty(), "no Bcc on the wire");
    }

    /// What is not an address is usually a name, and the log keeps no names.
    #[test]
    fn the_log_keeps_no_entry_that_was_refused() {
        assert_eq!(loggable("not an address: Dana Wu, \"Wu"), "not an address");
        assert_eq!(
            loggable("550 5.1.1 <dana@example.com>: no such user"),
            "550 5.1.1 <address> no such user"
        );
    }

    /// Every line a submission server was sent, in order.
    type Heard = Arc<Mutex<Vec<String>>>;

    /// A submission server that greets, offers STARTTLS and agrees to it.
    ///
    /// A loopback socket cannot present a certificate this client would
    /// accept, so the conversation ends at the handshake, as it does in
    /// petrel-providers' STARTTLS tests. What it shows is whether a message
    /// was handed to the server at all: one refused before sending never
    /// opens a connection.
    async fn submission_server() -> (u16, Heard) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&heard);
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"220 mail.example ESMTP\r\n").await;
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    log.lock().unwrap().push(line.trim_end().to_string());
                    let upper = line.to_ascii_uppercase();
                    if upper.starts_with("EHLO") {
                        let _ = tx
                            .write_all(b"250-mail.example\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n")
                            .await;
                    } else if upper.starts_with("STARTTLS") {
                        let _ = tx.write_all(b"220 2.0.0 Ready to start TLS\r\n").await;
                        break;
                    } else {
                        let _ = tx.write_all(b"250 ok\r\n").await;
                    }
                    line.clear();
                }
            }
        });
        (port, heard)
    }

    /// The account sends through `port` on loopback, its password cached so
    /// no keychain is asked.
    fn wire(state: &AppState, port: u16) {
        state
            .store()
            .unwrap()
            .set_account_servers(
                state.account_id,
                &AccountServers {
                    imap_host: "127.0.0.1".into(),
                    imap_port: 993,
                    smtp_host: "127.0.0.1".into(),
                    smtp_port: port,
                    username: "sam".into(),
                    provider: String::new(),
                },
            )
            .unwrap();
        crate::config::remember_password(state.account_id, "pw");
    }

    /// A message due now, its fields as the composer stores them: the chips
    /// joined by ", ".
    fn queue(state: &AppState, to: &str, bcc: &str) -> i64 {
        let store = state.store().unwrap();
        let id = store
            .save_draft_full(
                state.account_id,
                None,
                to,
                "",
                "Numbers",
                "In.",
                "",
                &DraftEnvelope {
                    bcc: bcc.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store.schedule_send(id, Some(now_ms() - 1)).unwrap();
        id
    }

    fn row(state: &AppState, id: i64) -> OutboxRow {
        state
            .store()
            .unwrap()
            .outbox(state.account_id)
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .expect("still in the outbox")
    }

    /// Fifth audit #174 (pass 2's probe). The composer keeps
    /// `"Wu, Dana" <dana@example.com>` as one chip; the send split it at the
    /// comma, refused `"Wu` as no address, and retried for ever without ever
    /// reaching the server. "Last, First" is how corporate address books write
    /// names, and how Thunderbird copies them.
    #[test]
    fn a_quoted_name_with_a_comma_is_one_recipient() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let (row, heard) = tauri::async_runtime::block_on(async {
            let (port, heard) = submission_server().await;
            wire(&state, port);
            let id = queue(
                &state,
                "\"Wu, Dana\" <dana@example.com>, alex@example.com",
                "\"Nair, Priya\" <priya@example.net>",
            );
            send_due(Arc::clone(&state), state.account_id).await;
            (row(&state, id), heard.lock().unwrap().clone())
        });
        crate::config::forget_password(state.account_id);
        println!(
            "server heard {heard:?}; row {:?} {:?}",
            row.state, row.error
        );
        assert!(
            heard.iter().any(|l| l.starts_with("STARTTLS")),
            "the message never reached the server: {:?}",
            row.error
        );
        assert!(
            !row.error
                .as_deref()
                .unwrap_or("")
                .contains("not an address"),
            "{:?}",
            row.error
        );
    }

    /// A name typed where an address belongs. No retry changes what the
    /// message says, so it is held for the person with the reason on its row,
    /// instead of "Nothing was sent. Trying again…" every half hour for good.
    #[test]
    fn an_entry_that_is_not_an_address_holds_the_message_and_says_why() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let (row, heard) = tauri::async_runtime::block_on(async {
            let (port, heard) = submission_server().await;
            wire(&state, port);
            let id = queue(&state, "Dana Wu, alex@example.com", "");
            send_due(Arc::clone(&state), state.account_id).await;
            (row(&state, id), heard.lock().unwrap().clone())
        });
        crate::config::forget_password(state.account_id);
        println!(
            "server heard {heard:?}; row {:?} {:?}, next try {:?}",
            row.state, row.error, row.next_ms
        );
        assert!(heard.is_empty(), "nothing goes with a recipient missing");
        assert_eq!(row.state, "FailedPermanent", "{:?}", row.error);
        assert_eq!(row.next_ms, None, "and nothing is retried");
        assert!(
            row.error.as_deref().unwrap_or("").contains("Dana Wu"),
            "the row names the entry to fix: {:?}",
            row.error
        );
    }
}
