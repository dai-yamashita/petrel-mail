//! A server that takes the connection and never answers the TLS handshake
//! must not hold the client for good.
//!
//! The handshake had no deadline: the read deadline is wrapped round the
//! socket only once TLS is up, and the STARTTLS upgrade on the submission
//! port went through the same unbounded call. A wedged proxy, a TLS front end
//! with no back end, or a captive portal that accepts every connection was
//! enough to stop that account's sync, every account's queued triage (the
//! drain's guard is shared), and a send, which sat at "Sending…" until the
//! app was quit.
//!
//! Own file, so the deadlines it shortens cannot race another test's.

use std::time::Duration;

use petrel_providers::imap::{Credential, ImapConfig, Security, sync_pass};
use petrel_providers::smtp::{Outgoing, SendResult, SmtpConfig, send_tls};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Long enough not to fire on a loaded machine; the point is that it fires.
fn short_connect_deadlines() {
    // Safety: every test in this binary sets the same values.
    unsafe {
        std::env::set_var("PETREL_IMAP_CONNECT_SECONDS", "2");
        std::env::set_var("PETREL_SMTP_CONNECT_SECONDS", "2");
    }
}

/// What the test waits before calling it a hang: far beyond the deadline,
/// far short of the shipping read and reply deadlines, which are not what
/// should end it.
const HANG: Duration = Duration::from_secs(20);

/// Accepts, reads whatever comes, and never writes a byte: a ClientHello
/// gets no ServerHello.
async fn silent_peer() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = sock.read(&mut buf).await {
                    if n == 0 {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// Greets, offers STARTTLS, agrees to it, and then says nothing at all.
async fn mute_after_starttls() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = sock.write_all(b"220 mail.example ESMTP\r\n").await;
                let mut buf = [0u8; 4096];
                let mut agreed = false;
                while let Ok(n) = sock.read(&mut buf).await {
                    if n == 0 {
                        return;
                    }
                    if agreed {
                        // The ClientHello: read, and never answered.
                        continue;
                    }
                    let said = String::from_utf8_lossy(&buf[..n]).to_ascii_uppercase();
                    if said.starts_with("EHLO") {
                        let _ = sock
                            .write_all(b"250-mail.example\r\n250 STARTTLS\r\n")
                            .await;
                    } else if said.starts_with("STARTTLS") {
                        let _ = sock.write_all(b"220 2.0.0 Ready to start TLS\r\n").await;
                        agreed = true;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn an_imap_server_that_never_answers_the_handshake_is_given_up_on() {
    short_connect_deadlines();
    let port = silent_peer().await;
    let cfg = ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p"),
        security: Security::Tls,
    };
    let pass = tokio::time::timeout(HANG, sync_pass(&cfg, &[], false, |_, _, _, _| {}))
        .await
        .expect("the handshake has a deadline of its own");
    let err = pass.expect_err("a server that never answered is a failure");
    assert!(
        err.to_string().contains("TLS handshake timed out"),
        "and says what it waited for: {err}"
    );
}

#[tokio::test]
async fn a_starttls_handshake_that_is_never_answered_fails_the_send_before_it_starts() {
    short_connect_deadlines();
    let port = mute_after_starttls().await;
    let cfg = SmtpConfig {
        host: "127.0.0.1".into(),
        port,
        user: "someone@example.com".into(),
        credential: Credential::password("not-a-real-password"),
    };
    let msg = Outgoing {
        from_addr: "someone@example.com".into(),
        from_name: String::new(),
        to: vec!["dana@example.com".into()],
        cc: vec![],
        bcc: vec![],
        subject: "s".into(),
        body_text: "b".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };
    let (_, raw) = msg.render("example.com");
    let sent = tokio::time::timeout(HANG, send_tls(&cfg, &msg, &raw))
        .await
        .expect("the STARTTLS handshake has a deadline of its own");
    match sent {
        // Nothing was transmitted, so the retry ladder may try again.
        SendResult::FailedBeforeCommit { stage, detail } => {
            assert_eq!(stage, "starttls");
            assert!(detail.contains("TLS handshake timed out"), "{detail}");
        }
        other => panic!("a send that never got past STARTTLS failed before commit: {other:?}"),
    }
}
