//! A move or an expunge asks the server what it can do when the caller
//! does not know.
//!
//! The shell learns a server's capabilities from its launch probe. A launch
//! with no network never learns them, and "unknown" used to read as "none":
//! for the rest of the session every archive went out as COPY plus a
//! `\Deleted` flag with no expunge, the flagged copy stayed in the inbox on
//! the server, and the next reconcile filed the message back into the inbox
//! here. Delete forever and Empty Trash only flagged. The connection doing
//! the work can simply ask, so that is what these pin.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, ImapConfig, Security, Stored, expunge_uid, move_all, move_uid,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// A server advertising `caps` on CAPABILITY (or refusing it when `None`).
/// Every command line it reads is recorded in `seen`.
async fn server(seen: Arc<Mutex<Vec<String>>>, caps: Option<&'static str>) -> u16 {
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
                        match caps {
                            Some(c) => format!("* CAPABILITY IMAP4rev1 {c}\r\n{tag} OK done\r\n"),
                            None => format!("{tag} BAD not today\r\n"),
                        }
                    } else if up.contains(" SELECT ") || up.contains(" EXAMINE ") {
                        format!(
                            "* 3 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 9] ok\r\n{tag} OK done\r\n"
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

fn said(seen: &Arc<Mutex<Vec<String>>>, what: &str) -> bool {
    seen.lock()
        .unwrap()
        .iter()
        .any(|l| l.to_ascii_uppercase().contains(what))
}

#[tokio::test]
async fn a_move_the_caller_knows_nothing_about_asks_and_moves() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), Some("MOVE UIDPLUS IDLE")).await;
    let moved = move_uid(
        &cfg(port),
        "INBOX",
        5,
        "Archive",
        false,
        false,
        Some(Stored {
            message_id: "m5@x",
            size: None,
        }),
    )
    .await
    .unwrap();
    assert!(moved, "{:?}", seen.lock().unwrap());
    assert!(said(&seen, "UID MOVE 5"), "{:?}", seen.lock().unwrap());
    assert!(!said(&seen, "UID COPY"), "{:?}", seen.lock().unwrap());
    assert!(!said(&seen, "\\DELETED"), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn an_expunge_the_caller_knows_nothing_about_asks_and_expunges() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), Some("UIDPLUS")).await;
    let expunged = expunge_uid(&cfg(port), "Trash", 4, false).await.unwrap();
    assert!(expunged, "{:?}", seen.lock().unwrap());
    assert!(said(&seen, "UID EXPUNGE 4"), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn moving_a_folder_the_caller_knows_nothing_about_asks_and_moves() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), Some("MOVE")).await;
    move_all(&cfg(port), "Old", "Trash", false).await.unwrap();
    assert!(said(&seen, "UID MOVE 1:*"), "{:?}", seen.lock().unwrap());
    assert!(!said(&seen, "UID COPY"), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_server_without_move_or_uidplus_still_gets_the_careful_path() {
    // Control: what the slow path was for. Copy, flag, and no bare EXPUNGE,
    // which would commit other clients' deletions.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), Some("IDLE")).await;
    let moved = move_uid(
        &cfg(port),
        "INBOX",
        5,
        "Archive",
        false,
        false,
        Some(Stored {
            message_id: "m5@x",
            size: None,
        }),
    )
    .await
    .unwrap();
    assert!(!moved);
    assert!(said(&seen, "UID COPY 5"), "{:?}", seen.lock().unwrap());
    assert!(said(&seen, "\\DELETED"), "{:?}", seen.lock().unwrap());
    assert!(!said(&seen, "EXPUNGE"), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_refused_capability_keeps_the_careful_path() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), None).await;
    let moved = move_uid(
        &cfg(port),
        "INBOX",
        5,
        "Archive",
        false,
        false,
        Some(Stored {
            message_id: "m5@x",
            size: None,
        }),
    )
    .await
    .unwrap();
    assert!(!moved);
    assert!(!said(&seen, "UID MOVE"), "{:?}", seen.lock().unwrap());
    assert!(!said(&seen, "EXPUNGE"), "{:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn a_caller_that_knows_does_not_pay_for_asking() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = server(Arc::clone(&seen), Some("MOVE UIDPLUS")).await;
    move_uid(&cfg(port), "INBOX", 5, "Archive", true, true, None)
        .await
        .unwrap();
    expunge_uid(&cfg(port), "Trash", 4, true).await.unwrap();
    move_all(&cfg(port), "Old", "Trash", true).await.unwrap();
    assert!(!said(&seen, " CAPABILITY"), "{:?}", seen.lock().unwrap());
}
