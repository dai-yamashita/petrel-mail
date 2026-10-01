//! A scripted IMAP server for the shell's tests: plaintext, in memory,
//! passwords accepted or refused by name, and every LOGIN recorded with its
//! password, its verdict and when it came. What the sign-in stand-down is
//! measured with (signin.rs): how many times a refused password was tried.
//!
//! Knobs for the cases the stand-down has to tell apart: refusals answered
//! late (`refuse_delay`, Dovecot's auth_failure_delay); the next sign-ins
//! answered with other words whatever the password (`refuse_next` and
//! `transient`, Gmail's "Too many simultaneous connections"); a scripted
//! reply to UID MOVE (`move_reply`); `* 1 EXISTS` pushed to a connection
//! idling in a named mailbox (`push`, `push_to`); and a mailbox that is
//! listed but cannot be opened (`locked`), which fails every cycle.
//!
//! A TLS client opens with a ClientHello rather than a command; such a
//! connection is counted (`tls_hellos`) and closed, so a configuration built
//! from the store, which is always TLS, can still be counted connection by
//! connection.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub struct Mailbox {
    pub attrs: &'static str,
    pub msgs: Vec<(u32, Vec<u8>)>,
    pub next: u32,
}

pub struct Srv {
    pub caps: &'static str,
    pub boxes: Mutex<BTreeMap<String, Mailbox>>,
    /// Passwords the server takes. Anything else is refused.
    pub accept: Mutex<HashSet<String>>,
    /// Delay before answering a LOGIN, accepted or refused (Dovecot's
    /// auth_failure_delay is 2 s for refusals).
    pub login_delay: Mutex<Duration>,
    /// Delay before answering a refused LOGIN only.
    pub refuse_delay: Mutex<Duration>,
    /// Refuse this many more LOGINs with `transient`, whatever the password.
    pub refuse_next: AtomicUsize,
    pub transient: Mutex<String>,
    /// The words after the tag for every UID MOVE, when set.
    pub move_reply: Mutex<Option<String>>,
    /// Mailboxes LIST shows that SELECT, EXAMINE and STATUS refuse.
    pub locked: Mutex<HashSet<String>>,
    /// Bumped to say "* 1 EXISTS" on every connection idling in `push_to`.
    pub push: tokio::sync::watch::Sender<u64>,
    pub push_to: Mutex<String>,
    /// The words of a refusal, after the tag.
    pub refusal: Mutex<String>,
    /// (password, accepted, when)
    pub logins: Mutex<Vec<(String, bool, Instant)>>,
    pub conns: AtomicUsize,
    /// Connections that opened with a TLS ClientHello.
    pub tls_hellos: AtomicUsize,
    /// Connections to drop on arrival, as a network that is not up yet.
    pub drop_first: AtomicUsize,
    /// Commands seen, in order.
    pub seen: Mutex<Vec<String>>,
    /// Bumped to drop every open connection (a provider ending sessions
    /// when the password changes).
    pub epoch: tokio::sync::watch::Sender<u64>,
    pub started: Instant,
}

impl Srv {
    pub fn new(caps: &'static str, boxes: &[(&str, &'static str)], accept: &[&str]) -> Arc<Srv> {
        Arc::new(Srv {
            caps,
            boxes: Mutex::new(
                boxes
                    .iter()
                    .map(|(n, a)| {
                        (
                            n.to_string(),
                            Mailbox {
                                attrs: a,
                                msgs: Vec::new(),
                                next: 1,
                            },
                        )
                    })
                    .collect(),
            ),
            accept: Mutex::new(accept.iter().map(|s| s.to_string()).collect()),
            login_delay: Mutex::new(Duration::ZERO),
            refuse_delay: Mutex::new(Duration::ZERO),
            refuse_next: AtomicUsize::new(0),
            transient: Mutex::new(String::new()),
            move_reply: Mutex::new(None),
            locked: Mutex::new(HashSet::new()),
            push: tokio::sync::watch::channel(0).0,
            push_to: Mutex::new(String::new()),
            refusal: Mutex::new("NO [AUTHENTICATIONFAILED] Authentication failed.".into()),
            logins: Mutex::new(Vec::new()),
            conns: AtomicUsize::new(0),
            tls_hellos: AtomicUsize::new(0),
            drop_first: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
            epoch: tokio::sync::watch::channel(0).0,
            started: Instant::now(),
        })
    }

    pub fn put(&self, mailbox: &str, uid: u32, raw: Vec<u8>) {
        let mut b = self.boxes.lock().unwrap();
        let m = b.get_mut(mailbox).unwrap();
        m.msgs.push((uid, raw));
        m.next = m.next.max(uid + 1);
    }

    pub fn refused_since(&self, since: Instant) -> usize {
        self.logins
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, ok, at)| !ok && *at >= since)
            .count()
    }

    pub fn logins_with(&self, pass: &str, since: Instant) -> usize {
        self.logins
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _, at)| p == pass && *at >= since)
            .count()
    }

    pub fn timeline(&self) -> String {
        self.logins
            .lock()
            .unwrap()
            .iter()
            .map(|(p, ok, at)| {
                format!(
                    "{:.1}s {p} {}",
                    at.duration_since(self.started).as_secs_f32(),
                    if *ok { "ok" } else { "REFUSED" }
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

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

/// The reply to one command line, and whether to close afterwards.
fn answer(srv: &Srv, selected: &mut Option<String>, line: &str) -> (String, bool) {
    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
    let up = line.to_ascii_uppercase();
    if up.contains(" LOGOUT") {
        return (format!("* BYE\r\n{tag} OK bye\r\n"), true);
    }
    if up.contains(" CAPABILITY") {
        return (
            format!("* CAPABILITY IMAP4rev1 {}\r\n{tag} OK done\r\n", srv.caps),
            false,
        );
    }
    let boxes = srv.boxes.lock().unwrap();
    if up.contains(" LIST ") {
        let mut out = String::new();
        for (name, b) in boxes.iter() {
            out.push_str(&format!(
                "* LIST (\\HasNoChildren{}{}) \"/\" \"{name}\"\r\n",
                if b.attrs.is_empty() { "" } else { " " },
                b.attrs
            ));
        }
        return (format!("{out}{tag} OK done\r\n"), false);
    }
    if up.contains(" SELECT ") || up.contains(" EXAMINE ") || up.contains(" STATUS ") {
        let name = quoted(line).into_iter().next().unwrap_or_default();
        if srv.locked.lock().unwrap().contains(&name) {
            return (format!("{tag} NO Mailbox is locked\r\n"), false);
        }
        if up.contains(" STATUS ") {
            return (format!("{tag} OK done\r\n"), false);
        }
        return match boxes.get(&name) {
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
                format!("{tag} NO [NONEXISTENT] Unknown Mailbox: {name}\r\n"),
                false,
            ),
        };
    }
    let Some(sel) = selected.clone() else {
        return (format!("{tag} OK done\r\n"), false);
    };
    if up.contains(" SEARCH ") {
        let uids: Vec<String> = boxes[&sel]
            .msgs
            .iter()
            .map(|(u, _)| u.to_string())
            .collect();
        return (
            format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" ")),
            false,
        );
    }
    (format!("{tag} OK done\r\n"), false)
}

pub async fn serve(srv: Arc<Srv>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            srv.conns.fetch_add(1, Ordering::SeqCst);
            if srv
                .drop_first
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                drop(sock);
                continue;
            }
            let srv = Arc::clone(&srv);
            tokio::spawn(async move {
                let mut epoch = srv.epoch.subscribe();
                let mut push = srv.push.subscribe();
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                // Read as bytes: a ClientHello is not UTF-8, and read as a
                // line it was an error, which looked like a closed socket.
                let mut bytes = Vec::new();
                let mut line = String::new();
                let mut selected = None;
                let mut idle_tag: Option<String> = None;
                loop {
                    let got = tokio::select! {
                        r = reader.read_until(b'\n', &mut bytes) => r.unwrap_or(0),
                        _ = epoch.changed() => 0,
                        _ = push.changed() => {
                            let here = selected.clone().unwrap_or_default();
                            let idling_here =
                                idle_tag.is_some() && here == *srv.push_to.lock().unwrap();
                            if idling_here && tx.write_all(b"* 1 EXISTS\r\n").await.is_err() {
                                return;
                            }
                            continue;
                        }
                    };
                    if got == 0 {
                        let _ = tx.shutdown().await;
                        return;
                    }
                    // A TLS client opens with a ClientHello, not a command:
                    // the connection is counted and closed.
                    if bytes.first() == Some(&0x16) {
                        srv.tls_hellos.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                    line.clear();
                    line.push_str(&String::from_utf8_lossy(&bytes));
                    bytes.clear();
                    if let Some(t) = idle_tag.take()
                        && line.trim().eq_ignore_ascii_case("DONE")
                    {
                        if tx
                            .write_all(format!("{t} OK idle done\r\n").as_bytes())
                            .await
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    let up = line.to_ascii_uppercase();
                    srv.seen.lock().unwrap().push(line.trim_end().to_string());
                    if up.contains(" LOGIN ") {
                        let pass = quoted(&line).get(1).cloned().unwrap_or_default();
                        let transient = srv
                            .refuse_next
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                            .is_ok();
                        let ok = !transient && srv.accept.lock().unwrap().contains(&pass);
                        srv.logins.lock().unwrap().push((pass, ok, Instant::now()));
                        let mut delay = *srv.login_delay.lock().unwrap();
                        if !ok {
                            delay += *srv.refuse_delay.lock().unwrap();
                        }
                        tokio::time::sleep(delay).await;
                        let reply = if ok {
                            format!("{tag} OK Logged in\r\n")
                        } else if transient {
                            format!("{tag} {}\r\n", srv.transient.lock().unwrap())
                        } else {
                            format!("{tag} {}\r\n", srv.refusal.lock().unwrap())
                        };
                        if tx.write_all(reply.as_bytes()).await.is_err() {
                            return;
                        }
                        continue;
                    }
                    if up.contains(" IDLE") {
                        idle_tag = Some(tag);
                        if tx.write_all(b"+ idling\r\n").await.is_err() {
                            return;
                        }
                        continue;
                    }
                    let scripted = if up.contains(" UID MOVE ") {
                        srv.move_reply.lock().unwrap().clone()
                    } else {
                        None
                    };
                    if let Some(words) = scripted {
                        if tx
                            .write_all(format!("{tag} {words}\r\n").as_bytes())
                            .await
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    let (reply, close) = answer(&srv, &mut selected, &line);
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

pub fn raw(n: u32) -> Vec<u8> {
    format!(
        "From: a@example.com\r\nTo: b@example.com\r\nSubject: m{n}\r\n\
         Message-ID: <m{n}@probe.example>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain\r\n\r\nbody {n}\r\n"
    )
    .into_bytes()
}
