//! What a server can do, learned whenever it answers; and Gmail's All Mail
//! found by what it is rather than by its English name.
//!
//! The server-facing ones run against a small scripted IMAP server that
//! keeps its mailboxes in memory, so a MOVE really leaves the source and a
//! COPY really does not. It speaks plaintext, so they need
//! `--features dev-plaintext-imap`.

use super::*;
use crate::state::{ServerCaps, test_state};

/// After an offline launch the IDLE watchers start anyway and wait to hear
/// whether the server can IDLE, rather than leaving the account on the
/// two-minute poll for the rest of the session.
#[test]
fn the_watchers_wait_for_the_server_to_be_known() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    tauri::async_runtime::block_on(async {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let waiting = {
            let state = Arc::clone(&state);
            let mut rx = rx.clone();
            tokio::spawn(async move { idle_known(&state, account, &mut rx).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!waiting.is_finished(), "nothing is known yet");
        state.set_caps(
            account,
            ServerCaps {
                has_idle: true,
                known: true,
                ..Default::default()
            },
        );
        assert!(waiting.await.unwrap());

        // Known and without IDLE: the watcher has nothing to do.
        state.set_caps(
            account,
            ServerCaps {
                known: true,
                ..Default::default()
            },
        );
        let mut rx2 = rx.clone();
        assert!(!idle_known(&state, account, &mut rx2).await);

        // And a stopped account stops waiting.
        let other = account + 1;
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let waiting = {
            let state = Arc::clone(&state);
            tokio::spawn(async move { idle_known(&state, other, &mut stop_rx).await })
        };
        stop_tx.send_replace(true);
        assert!(!waiting.await.unwrap());
    });
}

#[cfg(feature = "dev-plaintext-imap")]
mod scripted {
    use super::*;
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_providers::imap::{Credential, Security};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    struct Mailbox {
        /// LIST attributes after `\HasNoChildren`, e.g. `\Archive`.
        attrs: &'static str,
        msgs: Vec<(u32, Vec<u8>)>,
        next: u32,
    }

    struct World {
        caps: &'static str,
        boxes: BTreeMap<String, Mailbox>,
        seen: Vec<String>,
    }

    impl World {
        fn new(caps: &'static str, boxes: &[(&str, &'static str)]) -> Arc<Mutex<World>> {
            Arc::new(Mutex::new(World {
                caps,
                boxes: boxes
                    .iter()
                    .map(|(name, attrs)| {
                        (
                            name.to_string(),
                            Mailbox {
                                attrs,
                                msgs: Vec::new(),
                                next: 1,
                            },
                        )
                    })
                    .collect(),
                seen: Vec::new(),
            }))
        }

        fn put(world: &Arc<Mutex<World>>, mailbox: &str, uid: u32, raw: Vec<u8>) {
            let mut w = world.lock().unwrap();
            let b = w.boxes.get_mut(mailbox).unwrap();
            b.msgs.push((uid, raw));
            b.next = b.next.max(uid + 1);
        }

        fn uids(world: &Arc<Mutex<World>>, mailbox: &str) -> Vec<u32> {
            world.lock().unwrap().boxes[mailbox]
                .msgs
                .iter()
                .map(|(u, _)| *u)
                .collect()
        }

        fn said(world: &Arc<Mutex<World>>, what: &str) -> bool {
            world
                .lock()
                .unwrap()
                .seen
                .iter()
                .any(|l| l.to_ascii_uppercase().contains(what))
        }
    }

    /// The quoted strings on a command line, in order.
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

    fn answer(
        world: &Arc<Mutex<World>>,
        selected: &mut Option<String>,
        line: &str,
    ) -> (String, bool) {
        let tag = line.split_whitespace().next().unwrap_or("*").to_string();
        let up = line.to_ascii_uppercase();
        let mut w = world.lock().unwrap();
        w.seen.push(line.trim_end().to_string());
        if up.contains(" LOGOUT") {
            return (format!("* BYE\r\n{tag} OK bye\r\n"), true);
        }
        if up.contains(" CAPABILITY") {
            return (
                format!("* CAPABILITY IMAP4rev1 {}\r\n{tag} OK done\r\n", w.caps),
                false,
            );
        }
        if up.contains(" LIST ") {
            let mut out = String::new();
            for (name, b) in &w.boxes {
                out.push_str(&format!(
                    "* LIST (\\HasNoChildren{}{}) \"/\" \"{name}\"\r\n",
                    if b.attrs.is_empty() { "" } else { " " },
                    b.attrs
                ));
            }
            return (format!("{out}{tag} OK done\r\n"), false);
        }
        if up.contains(" SELECT ") || up.contains(" EXAMINE ") {
            let name = quoted(line).into_iter().next().unwrap_or_default();
            return match w.boxes.get(&name) {
                Some(b) => {
                    *selected = Some(name);
                    (
                        format!(
                            "* {} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {}] ok\r\n{tag} OK done\r\n",
                            b.msgs.len(),
                            b.next
                        ),
                        false,
                    )
                }
                None => (
                    format!("{tag} NO [NONEXISTENT] Unknown Mailbox: {name} (Failure)\r\n"),
                    false,
                ),
            };
        }
        let Some(sel) = selected.clone() else {
            return (format!("{tag} OK done\r\n"), false);
        };
        if up.contains(" SEARCH ") {
            let uids: Vec<String> = w.boxes[&sel]
                .msgs
                .iter()
                .map(|(u, _)| u.to_string())
                .collect();
            return (
                format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" ")),
                false,
            );
        }
        if up.contains(" UID MOVE ") || up.contains(" UID COPY ") {
            let uid: u32 = line.split_whitespace().nth(3).unwrap().parse().unwrap();
            let to = quoted(line).pop().unwrap_or_default();
            let moving = up.contains(" UID MOVE ");
            let Some(pos) = w.boxes[&sel].msgs.iter().position(|(u, _)| *u == uid) else {
                return (format!("{tag} OK nothing\r\n"), false);
            };
            let raw = if moving {
                w.boxes.get_mut(&sel).unwrap().msgs.remove(pos).1
            } else {
                w.boxes[&sel].msgs[pos].1.clone()
            };
            let dest = w.boxes.get_mut(&to).unwrap();
            let new_uid = dest.next;
            dest.next += 1;
            dest.msgs.push((new_uid, raw));
            return (format!("{tag} OK done\r\n"), false);
        }
        if up.contains(" UID FETCH ") {
            let set = line.split_whitespace().nth(3).unwrap_or("");
            let wanted: Vec<u32> = set.split(',').filter_map(|u| u.parse().ok()).collect();
            let mut out = String::new();
            for (seq, (uid, raw)) in w.boxes[&sel].msgs.iter().enumerate() {
                if !wanted.contains(uid) {
                    continue;
                }
                out.push_str(&format!(
                    "* {} FETCH (UID {uid} FLAGS () BODY[] {{{}}}\r\n{})\r\n",
                    seq + 1,
                    raw.len(),
                    String::from_utf8_lossy(raw)
                ));
            }
            return (format!("{out}{tag} OK done\r\n"), false);
        }
        (format!("{tag} OK done\r\n"), false)
    }

    async fn serve(world: Arc<Mutex<World>>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let world = Arc::clone(&world);
                tokio::spawn(async move {
                    let (rx, mut tx) = sock.into_split();
                    let mut reader = BufReader::new(rx);
                    let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                    let mut line = String::new();
                    let mut selected = None;
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let (reply, close) = answer(&world, &mut selected, &line);
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

    fn cfg(port: u16) -> ImapConfig {
        ImapConfig {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            credential: Credential::password("p"),
            security: Security::InsecurePlaintext,
        }
    }

    fn raw(n: u32) -> Vec<u8> {
        format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: m{n}\r\n\
             Message-ID: <m{n}@x.example>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain\r\n\r\nbody {n}\r\n"
        )
        .into_bytes()
    }

    /// INBOX holding `uids`, both locally and on the server, and an Archive.
    fn mailbox_with(state: &AppState, world: &Arc<Mutex<World>>, uids: &[u32]) -> (i64, Vec<i64>) {
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
        let threads = uids
            .iter()
            .map(|uid| {
                World::put(world, "INBOX", *uid, raw(*uid));
                store
                    .ingest_raw(&state.blobs, account, Some(inbox), Some(*uid), &raw(*uid))
                    .unwrap();
                let id = store.message_id_at(inbox, *uid).unwrap().unwrap();
                store.thread_of(id).unwrap().unwrap_or(-id)
            })
            .collect();
        (inbox, threads)
    }

    /// The launch never reached the server, so nothing about it is known. An
    /// archive is still a move, the server's own copy leaves the inbox, and the
    /// reconcile has nothing to walk back.
    #[test]
    fn an_archive_after_an_offline_launch_is_a_move_and_stays_archived() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let world = World::new(
            "MOVE UIDPLUS IDLE",
            &[("INBOX", ""), ("Archive", "\\Archive")],
        );
        let (inbox, threads) = mailbox_with(&state, &world, &[5, 6]);
        let id5 = state
            .store
            .lock()
            .unwrap()
            .message_id_at(inbox, 5)
            .unwrap()
            .unwrap();
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&world)).await;
            let caps = state.caps(account);
            assert!(!caps.known, "an offline launch learned nothing");
            state
                .store
                .lock()
                .unwrap()
                .apply_thread_action(
                    account,
                    threads[0],
                    ActionKind::Archive,
                    None,
                    PlacementPolicy::Exclusive,
                )
                .unwrap();
            assert!(
                drain::drain_actions(
                    Arc::clone(&state),
                    account,
                    cfg(port),
                    caps.has_move,
                    caps.has_uidplus,
                    caps.is_gmail,
                    &state.stop_signal(account),
                )
                .await
            );
            assert!(World::said(&world, "UID MOVE 5"));
            assert!(!World::said(&world, "UID COPY"));
            assert!(!World::said(&world, "\\DELETED"));
            assert_eq!(World::uids(&world, "INBOX"), vec![6]);
            reconcile_ghost_placements(&state, account, &cfg(port)).await;
            let folders = state.store.lock().unwrap().folders_of(id5).unwrap();
            assert!(
                !folders.contains(&inbox),
                "walked back into the inbox: {folders:?}"
            );
        });
    }

    /// A sweep's survey is as good as the launch probe: it records what the
    /// server can do, for a launch that had no network to ask.
    #[test]
    fn a_later_survey_records_what_the_launch_could_not() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let world = World::new(
            "MOVE UIDPLUS IDLE",
            &[("INBOX", ""), ("Archive", "\\Archive")],
        );
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&world)).await;
            let learned =
                refresh_folders(&state, account, &cfg(port), &state.stop_signal(account)).await;
            let caps = state.caps(account);
            assert!(caps.known && caps.has_move && caps.has_uidplus && caps.has_idle);
            assert!(!caps.is_gmail);
            assert_eq!(learned.map(|c| c.has_idle), Some(true));
            assert!(state.surveyed(account));
        });
    }

    /// Gmail translates its folder names. A German account's All Mail is
    /// `[Gmail]/Alle Nachrichten`, and in some countries the prefix is
    /// `[Google Mail]`; the sweeps asked for `[Gmail]/All Mail` by name and
    /// failed on every cycle.
    #[test]
    fn the_gmail_sweeps_read_all_mail_wherever_it_is() {
        for all_mail in [
            "[Gmail]/Alle Nachrichten",
            "[Google Mail]/All Mail",
            "[Gmail]/All Mail",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = test_state(dir.path());
            let account = state.account_id;
            {
                let mut store = state.store.lock().unwrap();
                store.set_account_kind(account, "gmail").unwrap();
                store
                    .sync_folders(
                        account,
                        &[
                            ("INBOX".into(), Some("inbox".into())),
                            (all_mail.into(), Some("archive".into())),
                        ],
                    )
                    .unwrap();
            }
            let world = World::new(
                "IDLE CONDSTORE X-GM-EXT-1",
                &[("INBOX", ""), (all_mail, "\\All")],
            );
            tauri::async_runtime::block_on(async {
                let port = serve(Arc::clone(&world)).await;
                run_label_sweep(&state, account, &cfg(port)).await;
                run_thrid_sweep(&state, account, &cfg(port)).await;
            });
            let opened: Vec<String> = world
                .lock()
                .unwrap()
                .seen
                .iter()
                .filter(|l| l.to_ascii_uppercase().contains("EXAMINE"))
                .cloned()
                .collect();
            assert_eq!(opened.len(), 2, "{all_mail}: {opened:?}");
            assert!(
                opened
                    .iter()
                    .all(|l| l.contains(&format!("\"{all_mail}\""))),
                "{all_mail}: {opened:?}"
            );
        }
    }

    /// No All Mail known, nothing asked: the sweeps used to open the English
    /// name and fail twice a cycle.
    #[test]
    fn the_gmail_sweeps_skip_an_account_with_no_all_mail() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let world = World::new("IDLE", &[("INBOX", "")]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&world)).await;
            run_label_sweep(&state, account, &cfg(port)).await;
            run_thrid_sweep(&state, account, &cfg(port)).await;
        });
        assert!(world.lock().unwrap().seen.is_empty());
    }
}
