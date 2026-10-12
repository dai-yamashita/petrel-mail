//! STATUS asks for HIGHESTMODSEQ only on a server that has CONDSTORE.
//!
//! HIGHESTMODSEQ is a status item CONDSTORE defines (RFC 7162). The pass
//! worked out whether the server had CONDSTORE and then asked for the item in
//! every STATUS anyway, and a strict server without it answers BAD: every
//! folder failed, every cycle, and nothing ever synced. GreenMail is one such
//! server (docs/25 #96).
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, FolderPass, ImapConfig, PassOutcome, Security, sync_pass,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// A server advertising `caps`, which answers BAD to a STATUS naming an item
/// it does not know: HIGHESTMODSEQ, unless it has CONDSTORE.
async fn strict_server(caps: &'static str, seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let condstore = caps.contains("CONDSTORE");
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK ready\r\n").await;
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
                        format!("* CAPABILITY IMAP4rev1 {caps}\r\n{tag} OK done\r\n")
                    } else if up.contains(" STATUS ") {
                        let asked = line
                            .split_whitespace()
                            .nth(2)
                            .map(|w| w.trim_matches('"').to_string())
                            .unwrap_or_default();
                        if up.contains("HIGHESTMODSEQ") && !condstore {
                            format!("{tag} BAD Unknown status item: 'HIGHESTMODSEQ'\r\n")
                        } else if condstore {
                            format!(
                                "* STATUS \"{asked}\" (MESSAGES 0 UIDNEXT 9 UIDVALIDITY 1 HIGHESTMODSEQ 4)\r\n{tag} OK done\r\n"
                            )
                        } else {
                            format!(
                                "* STATUS \"{asked}\" (MESSAGES 0 UIDNEXT 9 UIDVALIDITY 1)\r\n{tag} OK done\r\n"
                            )
                        }
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

/// Three quiet folders, each already synced once at UIDVALIDITY 1.
async fn pass_over(caps: &'static str) -> (Vec<PassOutcome>, Vec<String>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let port = strict_server(caps, Arc::clone(&seen)).await;
    let cfg = ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p"),
        security: Security::InsecurePlaintext,
    };
    let passes: Vec<FolderPass> = ["INBOX", "Sent", "Archive"]
        .iter()
        .map(|path| FolderPass {
            path: (*path).into(),
            since_uid: 8,
            expected_validity: Some(1),
            since_uidnext: Some(9),
            since_modseq: None,
            seed_window: 50,
            removal: None,
        })
        .collect();
    let outcomes = sync_pass(&cfg, &passes, true, |_, _, _, _| {})
        .await
        .expect("the pass runs");
    let statuses = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.to_ascii_uppercase().contains(" STATUS "))
        .cloned()
        .collect();
    (outcomes, statuses)
}

#[tokio::test]
async fn a_server_without_condstore_is_not_asked_for_highestmodseq() {
    let (outcomes, statuses) = pass_over("IDLE MOVE UIDPLUS SPECIAL-USE").await;
    assert_eq!(statuses.len(), 3, "{statuses:?}");
    assert!(
        statuses
            .iter()
            .all(|l| !l.to_ascii_uppercase().contains("HIGHESTMODSEQ")),
        "{statuses:?}"
    );
    for outcome in &outcomes {
        assert!(
            matches!(outcome, PassOutcome::Unchanged { .. }),
            "every folder answered, and none failed: {outcome:?}"
        );
    }
}

#[tokio::test]
async fn a_condstore_server_is_still_asked_for_it() {
    let (outcomes, statuses) = pass_over("IDLE MOVE UIDPLUS CONDSTORE").await;
    assert!(
        statuses
            .iter()
            .all(|l| l.to_ascii_uppercase().contains("HIGHESTMODSEQ")),
        "{statuses:?}"
    );
    for outcome in &outcomes {
        match outcome {
            PassOutcome::Unchanged {
                highest_modseq: Some(4),
                ..
            } => {}
            other => panic!("the modseq is read where the server has one: {other:?}"),
        }
    }
}
