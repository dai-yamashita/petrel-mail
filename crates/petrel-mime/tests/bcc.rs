//! A Bcc header, where one is present: the sender's own copy of what they
//! sent, a draft written in another client, or a delivery that left the
//! blind copy's own name on it.

fn raw(headers: &str) -> Vec<u8> {
    format!(
        "From: Sam Ortiz <sam@example.com>\r\n{headers}Subject: Numbers\r\n\
         Message-ID: <n1@example.com>\r\nDate: Mon, 7 Sep 2026 09:00:00 +0000\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nIn.\r\n"
    )
    .into_bytes()
}

#[test]
fn a_bcc_header_is_read_as_blind_copies() {
    let m = petrel_mime::parse_message(&raw(
        "To: dana@example.com\r\nBcc: Priya Nair <priya@example.net>, board@example.org\r\n",
    ))
    .unwrap();
    assert_eq!(
        m.bcc,
        vec![
            (
                Some("Priya Nair".to_string()),
                "priya@example.net".to_string()
            ),
            (None, "board@example.org".to_string()),
        ]
    );
    // Under their own role, so nothing that reads To or Cc ever takes them
    // for people the message was openly sent to.
    let roles: Vec<(&str, String)> = m
        .addresses()
        .into_iter()
        .map(|(role, addr, _)| (role, addr))
        .collect();
    assert_eq!(
        roles,
        vec![
            ("from", "sam@example.com".to_string()),
            ("to", "dana@example.com".to_string()),
            ("bcc", "priya@example.net".to_string()),
            ("bcc", "board@example.org".to_string()),
        ]
    );
}

#[test]
fn stacked_bcc_headers_are_all_read() {
    let m = petrel_mime::parse_message(&raw(
        "To: dana@example.com\r\nBcc: one@example.net\r\nBcc: two@example.net\r\n",
    ))
    .unwrap();
    let addrs: Vec<&str> = m.bcc.iter().map(|(_, a)| a.as_str()).collect();
    assert_eq!(addrs, vec!["one@example.net", "two@example.net"]);
}

#[test]
fn a_message_without_one_has_no_blind_copies() {
    let m = petrel_mime::parse_message(&raw("To: dana@example.com\r\n")).unwrap();
    assert!(m.bcc.is_empty());
    assert!(!m.addresses().iter().any(|(role, _, _)| *role == "bcc"));
}
