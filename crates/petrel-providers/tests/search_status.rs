//! A SEARCH the server refuses must not count as a SEARCH that found nothing.
//!
//! The typed search stops at the tagged reply without reading its status, so
//! `NO [SERVERBUG] internal error` arrived as an empty answer. Asked "which of
//! these UIDs are still here", an empty answer means "none of them": the
//! removal check dropped every placement in the folder, and on a classic
//! account the messages went with them, with nothing to fetch them back. The
//! same misreading told the ambiguous-send check a message was absent (and
//! so due another delivery) and told the drain a queued action had no server
//! copy left to act on.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, FolderPass, ImapConfig, PassOutcome, RemovalCheck, Security, find_message_id,
    move_uid, sync_pass, uids_for_message_id, uids_in_folder,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// What the server does with one command line.
enum Reply {
    Bytes(String),
    /// Write this and hang up.
    CloseAfter(String),
}

/// LOGIN, CAPABILITY, STATUS, EXAMINE/SELECT and LOGOUT, answered as a
/// server would, for a folder of `exists` messages below `uid_next`.
fn baseline(tag: &str, line: &str, exists: u32, uid_next: u32) -> Option<Reply> {
    let upper = line.to_ascii_uppercase();
    if upper.contains(" LOGIN ") {
        return Some(Reply::Bytes(format!("{tag} OK signed in\r\n")));
    }
    if upper.contains(" CAPABILITY") {
        return Some(Reply::Bytes(format!(
            "* CAPABILITY IMAP4rev1 CONDSTORE UIDPLUS\r\n{tag} OK done\r\n"
        )));
    }
    if upper.contains(" STATUS ") {
        return Some(Reply::Bytes(format!(
            "* STATUS \"INBOX\" (MESSAGES {exists} UIDNEXT {uid_next} UIDVALIDITY 1 HIGHESTMODSEQ 7)\r\n{tag} OK done\r\n"
        )));
    }
    if upper.contains(" EXAMINE ") || upper.contains(" SELECT ") {
        return Some(Reply::Bytes(format!(
            "* {exists} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {uid_next}] ok\r\n\
             * OK [HIGHESTMODSEQ 7] ok\r\n{tag} OK done\r\n"
        )));
    }
    if upper.contains(" LOGOUT") {
        return Some(Reply::CloseAfter(format!("* BYE\r\n{tag} OK bye\r\n")));
    }
    None
}

/// A scripted server. Every command line it reads is recorded in `seen`.
async fn server<F>(seen: Arc<Mutex<Vec<String>>>, handler: F) -> u16
where
    F: Fn(&str, &str) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    seen.lock().unwrap().push(line.trim_end().to_string());
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    match handler(&tag, &line) {
                        Reply::Bytes(b) => {
                            if tx.write_all(b.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        Reply::CloseAfter(b) => {
                            let _ = tx.write_all(b.as_bytes()).await;
                            let _ = tx.shutdown().await;
                            return;
                        }
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

fn is_search(line: &str) -> bool {
    line.to_ascii_uppercase().contains(" SEARCH ")
}

/// The store holds five placements here, and the server now counts three:
/// the removal check fires and asks which UIDs survive.
fn suspicious_pass(since_uidnext: u32) -> Vec<FolderPass> {
    vec![FolderPass {
        path: "INBOX".into(),
        since_uid: 5,
        expected_validity: Some(1),
        since_uidnext: Some(since_uidnext),
        since_modseq: Some(7),
        seed_window: 50,
        removal: Some(RemovalCheck {
            held: 5,
            last_seen: Some((5, 6)),
        }),
    }]
}

async fn one_pass(port: u16, passes: Vec<FolderPass>) -> PassOutcome {
    let mut out = sync_pass(&cfg(port), &passes, false, |_, _, _, _| {})
        .await
        .expect("the pass itself completes");
    out.remove(0)
}

#[tokio::test]
async fn a_refused_search_names_no_survivors() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("{tag} NO [SERVERBUG] Internal error occurred\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    match one_pass(port, suspicious_pass(6)).await {
        PassOutcome::Unchanged {
            survivors, seen, ..
        } => {
            assert!(
                survivors.is_none(),
                "a refused SEARCH is not an empty folder: {survivors:?}"
            );
            assert!(
                seen.is_none(),
                "and the counts are not adopted, so the next pass looks again"
            );
        }
        other => panic!("a quiet folder stays Unchanged: {other:?}"),
    }
    assert!(
        seen.lock().unwrap().iter().any(|l| is_search(l)),
        "the removal check did ask"
    );
}

#[tokio::test]
async fn a_search_answered_bad_names_no_survivors() {
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("{tag} BAD Command Argument Error. 11\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    match one_pass(port, suspicious_pass(6)).await {
        PassOutcome::Unchanged { survivors, .. } => assert!(survivors.is_none(), "{survivors:?}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_refused_search_after_a_fetch_names_no_survivors() {
    // New mail arrived (UIDNEXT 6 → 7) and a message left: the pass fetches,
    // then asks, and the ask is refused.
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 4, 7) {
            return r;
        }
        let upper = line.to_ascii_uppercase();
        if is_search(line) {
            return Reply::Bytes(format!("{tag} NO [SERVERBUG] Internal error occurred\r\n"));
        }
        if upper.contains("FETCH") {
            let raw = "From: a@example.com\r\nTo: b@example.com\r\nSubject: m6\r\n\
                       Message-ID: <m6@example.com>\r\n\r\nbody\r\n";
            return Reply::Bytes(format!(
                "* 4 FETCH (UID 6 FLAGS (\\Seen) BODY[] {{{}}}\r\n{raw})\r\n{tag} OK fetched\r\n",
                raw.len()
            ));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    match one_pass(port, suspicious_pass(6)).await {
        PassOutcome::Fetched {
            survivors, seen, ..
        } => {
            assert!(survivors.is_none(), "{survivors:?}");
            assert!(seen.is_none());
        }
        other => panic!("the new message is still fetched: {other:?}"),
    }
}

#[tokio::test]
async fn an_answered_search_still_names_its_survivors() {
    // The control, with the answer split over two lines and an unsolicited
    // EXISTS in between, both of which servers do.
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!(
                "* SEARCH 1 3\r\n* 3 EXISTS\r\n* SEARCH 5\r\n{tag} OK search done\r\n"
            ));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    match one_pass(port, suspicious_pass(6)).await {
        PassOutcome::Unchanged { survivors, .. } => {
            let s = survivors.expect("an answered search names its survivors");
            assert_eq!(s.uids, vec![1, 3, 5]);
            assert_eq!(s.below, 6);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_empty_answer_is_still_an_empty_folder() {
    // `OK` with no `* SEARCH` line at all is a real answer: nothing is left.
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 0, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("* SEARCH\r\n{tag} OK search done\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    match one_pass(port, suspicious_pass(6)).await {
        PassOutcome::Unchanged { survivors, .. } => {
            assert_eq!(survivors.expect("answered").uids, Vec::<u32>::new());
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_sweep_is_told_the_search_failed() {
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("{tag} NO [SERVERBUG] Internal error occurred\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;
    let got = uids_in_folder(&cfg(port), "INBOX").await;
    assert!(
        got.is_err(),
        "the reconcile sweep must not read {got:?} as empty"
    );
}

#[tokio::test]
async fn a_search_cut_off_by_bye_is_an_error() {
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::CloseAfter("* SEARCH 1\r\n* BYE server shutting down\r\n".into());
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;
    let got = uids_in_folder(&cfg(port), "INBOX").await;
    assert!(got.is_err(), "half an answer is not an answer: {got:?}");
}

#[tokio::test]
async fn a_refused_message_id_search_is_an_error_not_absence() {
    // The ambiguous-send check reads "absent" as "send it again", and the
    // drain reads it as "no server copy left": a refusal must be neither.
    let port = server(Arc::new(Mutex::new(Vec::new())), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("{tag} NO [UNAVAILABLE] Temporary failure\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;
    let by_seq = find_message_id(&cfg(port), "Sent", "<m1@example.com>").await;
    assert!(by_seq.is_err(), "{by_seq:?}");
    let by_uid = uids_for_message_id(&cfg(port), "Drafts", "<m1@example.com>").await;
    assert!(by_uid.is_err(), "{by_uid:?}");
}

#[tokio::test]
async fn an_answered_message_id_search_names_its_hits() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("* SEARCH 4 2\r\n{tag} OK search done\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;
    let hits = uids_for_message_id(&cfg(port), "Drafts", "<m1@example.com>")
        .await
        .expect("answered");
    assert_eq!(
        hits,
        vec![2, 4],
        "sorted, as callers take the last as newest"
    );
    let lines = seen.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("UID SEARCH HEADER Message-ID \"<m1@example.com>\"")),
        "asked in UIDs: {lines:?}"
    );
}

#[tokio::test]
async fn a_move_retry_whose_search_is_refused_still_copies() {
    // Without MOVE, a retried move first asks whether its copy already
    // landed. A refusal cannot say; the move goes ahead rather than never
    // happening, as it did before the refusal was visible.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("{tag} NO [SERVERBUG] Internal error occurred\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;
    let expunged = move_uid(
        &cfg(port),
        "INBOX",
        3,
        "Archive",
        false,
        true,
        Some("<m3@example.com>"),
    )
    .await
    .expect("the move completes");
    assert!(expunged);
    let lines = seen.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("UID COPY 3")),
        "the copy was made: {lines:?}"
    );
}

#[tokio::test]
async fn a_move_retry_whose_copy_landed_does_not_copy_again() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), |tag, line| {
        if let Some(r) = baseline(tag, line, 3, 6) {
            return r;
        }
        if is_search(line) {
            return Reply::Bytes(format!("* SEARCH 9\r\n{tag} OK search done\r\n"));
        }
        Reply::Bytes(format!("{tag} OK done\r\n"))
    })
    .await;
    move_uid(
        &cfg(port),
        "INBOX",
        3,
        "Archive",
        false,
        true,
        Some("<m3@example.com>"),
    )
    .await
    .expect("the move completes");
    let lines = seen.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("UID COPY")),
        "no second copy: {lines:?}"
    );
}
