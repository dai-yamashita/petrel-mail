//! A server refusing the password says so in its own way. RFC 5530 gives the
//! refusal a code, `[AUTHENTICATIONFAILED]`, and two more codes that are also
//! about the credentials themselves, `[AUTHORIZATIONFAILED]` and `[EXPIRED]`.
//! Exchange, Courier and Zimbra say `NO LOGIN failed.` and Cyrus `NO Login
//! failed: authentication failure`, with no code at all.
//!
//! Those, and only those, are a refused password: the error is the typed
//! `ImapError::SignInRefused`, and the shell stands the account down on it
//! (docs/25 #93). Any other code is "not now", an ordinary error retried as
//! one: Gmail's `NO [ALERT] Too many simultaneous connections` while other
//! clients hold its fifteen slots, `[UNAVAILABLE]`, `[INUSE]`, `[LIMIT]`,
//! `[SERVERBUG]`, or a code nobody has seen. One temporary NO used to stand an
//! account down for an hour, telling the person their password was wrong.
#![cfg(feature = "insecure-plaintext")]

use petrel_providers::imap::{Credential, ImapConfig, ImapError, Security};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Greets, then answers LOGIN with `reply` (after the tag).
async fn server(reply: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (rx, mut tx) = sock.into_split();
        let mut reader = BufReader::new(rx);
        tx.write_all(b"* OK [CAPABILITY IMAP4rev1] ready\r\n")
            .await
            .unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            let tag = line.split_whitespace().next().unwrap_or("*").to_string();
            let answer = if line.to_ascii_uppercase().contains(" LOGIN ") {
                format!("{tag} {reply}\r\n")
            } else {
                format!("{tag} OK done\r\n")
            };
            if tx.write_all(answer.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    port
}

async fn sign_in_fails(reply: &'static str) -> ImapError {
    let port = server(reply).await;
    petrel_providers::imap::login_check(&ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "someone".into(),
        credential: Credential::password("wrong"),
        security: Security::InsecurePlaintext,
    })
    .await
    .expect_err("the server said no")
}

#[tokio::test]
async fn a_refused_password_is_told_apart_by_its_code_or_its_words() {
    for reply in [
        "NO [AUTHENTICATIONFAILED] Authentication failed.",
        "NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)",
        "NO [AUTHORIZATIONFAILED] IMAP access is disabled for this account",
        "NO [EXPIRED] Password expired",
        "NO LOGIN failed.",
        "NO Login failed: authentication failure",
    ] {
        let e = sign_in_fails(reply).await;
        assert!(e.is_sign_in_refused(), "{reply} → {e}");
        assert!(matches!(e, ImapError::SignInRefused(_)), "{reply} → {e:?}");
        assert!(e.to_string().starts_with("sign-in refused: "), "{e}");
    }
}

#[tokio::test]
async fn a_server_saying_not_now_is_not_a_refused_password() {
    for reply in [
        // Gmail, over its fifteen-connection cap and past its bandwidth.
        "NO [ALERT] Too many simultaneous connections. (Failure)",
        "NO [ALERT] Account exceeded command or bandwidth limits. (Failure)",
        "NO [UNAVAILABLE] Temporary authentication failure. [host:2026-10-01 10:00:00]",
        "NO Temporary authentication failure.",
        "NO [INUSE] Mailbox in use, try again",
        "NO [LIMIT] Too many connections",
        "NO [SERVERBUG] Internal error",
        "NO [OVERQUOTA] Something nobody has seen at sign-in",
        "NO Server busy, try again later",
        "BAD Command syntax error",
    ] {
        let e = sign_in_fails(reply).await;
        assert!(!e.is_sign_in_refused(), "{reply} → {e}");
        assert!(!e.to_string().contains("sign-in refused"), "{reply} → {e}");
    }
}
