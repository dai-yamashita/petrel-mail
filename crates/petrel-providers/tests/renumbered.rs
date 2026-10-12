//! A command by UID in a folder renumbered since the UID was learned does
//! nothing.
//!
//! The drain acts on numbers it stored, some of them hours old: a change made
//! while the account was offline or signed out goes when it can sign in
//! again. A server that renumbered the folder meanwhile (a new UIDVALIDITY:
//! a mailbox migrated, an index rebuilt) has given those numbers to other
//! messages, and the archive moved one of them, Delete forever erased one.
//! RFC 3501 §2.3.1.1 says a UID means nothing under another UIDVALIDITY, so
//! the SELECT's is compared with the one the number was learned under.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, ImapConfig, ImapError, Security, Stored, expunge_uid, move_uid, store_flags,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// A server whose every folder is at UIDVALIDITY 2, recording each command.
async fn server(seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
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
                    let up = line.to_ascii_uppercase();
                    let reply = if up.contains(" CAPABILITY") {
                        format!("* CAPABILITY IMAP4rev1 MOVE UIDPLUS\r\n{tag} OK done\r\n")
                    } else if up.contains(" SELECT ") || up.contains(" EXAMINE ") {
                        format!(
                            "* 3 EXISTS\r\n* OK [UIDVALIDITY 2] ok\r\n* OK [UIDNEXT 4] ok\r\n{tag} OK done\r\n"
                        )
                    } else if up.contains(" SEARCH ") {
                        format!("* SEARCH\r\n{tag} OK done\r\n")
                    } else if up.contains(" LOGOUT") {
                        let _ = tx
                            .write_all(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes())
                            .await;
                        let _ = tx.shutdown().await;
                        return;
                    } else {
                        format!("{tag} OK done\r\n")
                    };
                    if tx.write_all(reply.as_bytes()).await.is_err() {
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

/// The commands that would have changed something on the server, without
/// their tags.
fn acted(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|l| l.split_once(' ').map(|(_, command)| command.to_string()))
        .filter(|command| {
            ["UID MOVE", "UID COPY", "UID STORE", "UID EXPUNGE"]
                .iter()
                .any(|c| command.to_ascii_uppercase().starts_with(c))
        })
        .collect()
}

fn renumbered(result: Result<impl std::fmt::Debug, ImapError>) {
    match result {
        Err(ImapError::Renumbered {
            expected: 1,
            now: 2,
        }) => {}
        other => panic!("refused as renumbered, 1 -> 2: {other:?}"),
    }
}

#[tokio::test]
async fn a_move_learned_under_another_numbering_moves_nothing() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen)).await;
    renumbered(move_uid(&cfg(port), "INBOX", 2, Some(1), "Archive", true, true, None).await);
    assert!(acted(&seen).is_empty(), "{:?}", seen.lock().unwrap());
    // Without MOVE too: the copy, the flag and the expunge are all by UID.
    renumbered(
        move_uid(
            &cfg(port),
            "INBOX",
            2,
            Some(1),
            "Archive",
            false,
            true,
            Some(Stored {
                message_id: "<a@example.com>",
                size: None,
            }),
        )
        .await,
    );
    assert!(acted(&seen).is_empty(), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_delete_forever_learned_under_another_numbering_erases_nothing() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen)).await;
    renumbered(expunge_uid(&cfg(port), "Trash", 2, Some(1), true).await);
    assert!(acted(&seen).is_empty(), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_flag_change_learned_under_another_numbering_flags_nothing() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen)).await;
    renumbered(store_flags(&cfg(port), "INBOX", &[1, 2], Some(1), "\\Seen", true).await);
    renumbered(store_flags(&cfg(port), "INBOX", &[2], Some(1), "\\Seen", true).await);
    assert!(acted(&seen).is_empty(), "{:?}", seen.lock().unwrap());
}

/// The numbering the UID was learned under, or none known: the command goes.
#[tokio::test]
async fn the_same_numbering_or_an_unknown_one_acts_as_before() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen)).await;
    move_uid(&cfg(port), "INBOX", 2, Some(2), "Archive", true, true, None)
        .await
        .expect("the numbering is the one the UID was learned under");
    expunge_uid(&cfg(port), "Trash", 3, None, true)
        .await
        .expect("a UID with no known numbering cannot be checked, as before");
    assert_eq!(
        acted(&seen),
        vec![
            "UID MOVE 2 \"Archive\"".to_string(),
            "UID STORE 3 +FLAGS.SILENT (\\Deleted)".to_string(),
            "UID EXPUNGE 3".to_string(),
        ]
    );
}
