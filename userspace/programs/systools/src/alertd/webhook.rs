// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Webhooks (ADR 0485): the text of an alert posted to ntfy, Matrix,
//! Slack or Discord, or its JSON to a program of the owner's.
//!
//! Each kind gets the request its service documents. The text is the
//! message's subject, then its body, but for ntfy, which has the subject
//! as the notification's title. A 2xx answer is delivered; any other
//! names the status and the first of the server's words, so that `alerts
//! test` shows what a service refused ("ntfy answered 403 Forbidden:
//! ...").
//!
//! An address is often a secret itself (a Slack or Discord URL carries
//! its token), and so is a token: neither is ever in an error or the log.
//! The system's root certificates are the ones trusted, as for every
//! other client of tuf's HTTP.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use tuf::http::{self, Error};
use tuf::url::Url;

use super::email;
use super::settings::{Kind, Webhook};
use super::trouble;

/// The most characters of a server's answer in an error.
const SAID_MAX: usize = 200;

/// How much of an answer is read to find those.
const READ_MAX: u64 = 1000;

/// Discord refuses a message of more than this many characters.
const DISCORD_MAX: usize = 2000;

/// How urgent a message is, which ntfy shows as a priority and an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Critical,
    Warning,
    /// A clearing, or a test.
    Calm,
}

/// What a webhook is told.
#[derive(Clone, Debug)]
pub struct Post<'a> {
    /// A name for this message, the same on each retry of it: Matrix
    /// takes it as the transaction, and drops a repeat.
    pub id: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    pub tone: Tone,
    /// The batch as the json kind's text.
    pub json: &'a str,
}

/// A request, before it is sent.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: &'static str,
    url: String,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

/// Counts the messages of this run that are not given an id, so that
/// their ids differ.
static MADE: AtomicU64 = AtomicU64::new(0);

/// An id for a message made at `now_ms`, milliseconds since the epoch:
/// that and a count, so that two in one millisecond differ.
pub fn new_id(now_ms: u64) -> String {
    format!("{now_ms}-{}", MADE.fetch_add(1, Ordering::Relaxed))
}

/// Posts `post` to `hook`.
///
/// # Errors
///
/// Why it was not delivered: that the address cannot be reached, with the
/// reason, or the status the service answered and its first words.
pub fn send(hook: &Webhook, post: &Post) -> Result<(), String> {
    let request = request(hook, post)?;
    let url = Url::parse(&request.url).map_err(|_| not_a_url())?;
    let headers: Vec<(&str, &str)> =
        request.headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let answer = http::exchange(request.method, &url, &headers, &request.body)
        .map_err(|err| unreachable(&err, url.host()))?;
    if (200..300).contains(&answer.status) {
        return Ok(());
    }
    let mut said = Vec::new();
    // What was read is what there is to say; a failed read says nothing
    // more.
    let _ = answer.body.take(READ_MAX).read_to_end(&mut said);
    Err(refusal(hook.kind, answer.status, &String::from_utf8_lossy(&said)))
}

/// The request that posts `post` to `hook`.
fn request(hook: &Webhook, post: &Post) -> Result<Request, String> {
    let text = format!("{}\n{}", post.subject, post.body);
    let bearer = hook.token.as_ref().map(|token| format!("Bearer {}", token.expose()));
    let mut headers = Vec::new();
    let (method, url, content_type, body) = match hook.kind {
        Kind::Ntfy => {
            headers.push(("Title", email::mime_words(post.subject)));
            headers.push(("Priority", priority(post.tone).to_string()));
            headers.push(("Tags", tag(post.tone).to_string()));
            ("POST", hook.url.clone(), "text/plain; charset=utf-8", post.body.to_string())
        }
        Kind::Matrix => {
            let Some(room) = &hook.room else {
                return Err("the Matrix room is missing in alerts.toml".into());
            };
            let path = format!(
                "/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
                percent_encoded(room),
                percent_encoded(post.id)
            );
            let url = format!("{}{path}", hook.url.trim_end_matches('/'));
            let body = json!({"msgtype": "m.text", "body": text}).to_string();
            ("PUT", url, "application/json", body)
        }
        Kind::Slack => {
            let body = json!({"text": text}).to_string();
            ("POST", hook.url.clone(), "application/json", body)
        }
        Kind::Discord => {
            let body = json!({"content": shortened(&text, DISCORD_MAX)}).to_string();
            ("POST", hook.url.clone(), "application/json", body)
        }
        Kind::Json => ("POST", hook.url.clone(), "application/json", post.json.to_string()),
    };
    headers.push(("Content-Type", content_type.to_string()));
    // Matrix and json take a token as a bearer; ntfy does too. Slack and
    // Discord have theirs in the address.
    if let (Some(bearer), true) = (bearer, takes_token(hook.kind)) {
        headers.push(("Authorization", bearer));
    }
    Ok(Request { method, url, headers, body: body.into_bytes() })
}

/// Whether `kind` signs in with a token in the Authorization header.
fn takes_token(kind: Kind) -> bool {
    matches!(kind, Kind::Ntfy | Kind::Matrix | Kind::Json)
}

/// ntfy's priority for `tone`: 5 is its highest, 3 the default.
fn priority(tone: Tone) -> u8 {
    match tone {
        Tone::Critical => 5,
        Tone::Warning => 4,
        Tone::Calm => 3,
    }
}

/// ntfy's tag for `tone`, which it shows as an emoji.
fn tag(tone: Tone) -> &'static str {
    match tone {
        Tone::Critical => "rotating_light",
        Tone::Warning => "warning",
        Tone::Calm => "white_check_mark",
    }
}

/// `text` cut to `max` characters, the last an ellipsis, when it is
/// longer.
fn shortened(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// `text` for a part of a path: each byte but a letter, digit, `-`, `.`,
/// `_` and `~` as `%` and two capital hex digits (RFC 3986).
fn percent_encoded(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The words for a connection that failed: the host, which is not secret
/// (the address's path and query may be).
fn unreachable(err: &Error, host: &str) -> String {
    match err {
        Error::Io(err) => {
            let why = trouble::connection(err, host, "the system trusts");
            format!("cannot reach {host}: {why}")
        }
        other => format!("cannot reach {host}: {other}"),
    }
}

/// The words for an address that is not a URL, without the address.
fn not_a_url() -> String {
    "the address in alerts.toml is not a URL this takes".into()
}

/// The words for a service that answered `status` with `body`: `ntfy
/// answered 403 Forbidden: ...`.
fn refusal(kind: Kind, status: u16, body: &str) -> String {
    let said = trouble::cut(body, SAID_MAX);
    let status = match reason(status) {
        "" => status.to_string(),
        reason => format!("{status} {reason}"),
    };
    if said.is_empty() {
        format!("{} answered {status}", kind.name())
    } else {
        format!("{} answered {status}: {said}", kind.name())
    }
}

/// The usual reason phrase of `status`, or none.
fn reason(status: u16) -> &'static str {
    match status {
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        410 => "Gone",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use super::super::settings::Secret;

    /// What the server in a test was sent.
    #[derive(Debug)]
    struct Received {
        request_line: String,
        /// Header names in lower case, and their values.
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Received {
        fn header(&self, name: &str) -> Option<&str> {
            let found = self.headers.iter().find(|(have, _)| have == name);
            found.map(|(_, value)| value.as_str())
        }
    }

    /// A server on lo for one request, which it answers with `status` and
    /// `body`: its port, and what it was sent.
    fn server(status: &str, body: &'static str) -> (u16, JoinHandle<Received>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_string();
        let thread = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(tcp);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request_line = line.trim_end().to_string();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
            }
            let length = headers
                .iter()
                .find(|(name, _)| name == "content-length")
                .map_or(0, |(_, value)| value.parse().unwrap());
            let mut sent = vec![0; length];
            reader.read_exact(&mut sent).unwrap();
            let answer = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(answer.as_bytes()).unwrap();
            Received { request_line, headers, body: String::from_utf8(sent).unwrap() }
        });
        (port, thread)
    }

    fn hook(kind: Kind, port: u16, path: &str, token: Option<&str>) -> Webhook {
        Webhook {
            kind,
            url: format!("http://127.0.0.1:{port}{path}"),
            token: token.map(Secret::new),
            room: (kind == Kind::Matrix).then(|| "!abc:example.org".to_string()),
        }
    }

    fn post(tone: Tone) -> Post<'static> {
        Post {
            id: "1763030531000-0",
            subject: "[server] CRITICAL: /data is 97% full",
            body: "Raised, critical: /data is 97% full\n  Free space.\n",
            tone,
            json: r#"{"version":1,"host":"server","changes":[]}"#,
        }
    }

    #[test]
    fn ntfy_gets_the_body_with_a_title_a_priority_and_a_tag() {
        let (port, thread) = server("200 OK", "{}");
        let hook = hook(Kind::Ntfy, port, "/server-alerts-7f3a", Some("tk_secret"));
        send(&hook, &post(Tone::Critical)).unwrap();
        let got = thread.join().unwrap();
        assert_eq!(got.request_line, "POST /server-alerts-7f3a HTTP/1.1");
        assert_eq!(got.header("title"), Some("[server] CRITICAL: /data is 97% full"));
        assert_eq!(got.header("priority"), Some("5"));
        assert_eq!(got.header("tags"), Some("rotating_light"));
        assert_eq!(got.header("authorization"), Some("Bearer tk_secret"));
        assert_eq!(got.header("content-type"), Some("text/plain; charset=utf-8"));
        assert_eq!(got.body, "Raised, critical: /data is 97% full\n  Free space.\n");
    }

    #[test]
    fn ntfy_priority_and_tag_follow_the_tone_and_no_token_sends_no_authorization() {
        for (tone, priority, tag) in
            [(Tone::Warning, "4", "warning"), (Tone::Calm, "3", "white_check_mark")]
        {
            let (port, thread) = server("200 OK", "");
            send(&hook(Kind::Ntfy, port, "/t", None), &post(tone)).unwrap();
            let got = thread.join().unwrap();
            assert_eq!(got.header("priority"), Some(priority));
            assert_eq!(got.header("tags"), Some(tag));
            assert_eq!(got.header("authorization"), None);
        }
    }

    #[test]
    fn a_title_that_is_not_ascii_goes_as_an_rfc_2047_word() {
        let (port, thread) = server("200 OK", "");
        let mut message = post(Tone::Warning);
        message.subject = "[serveur] Avertissement : le disque est plein \u{e9}";
        send(&hook(Kind::Ntfy, port, "/t", None), &message).unwrap();
        let title = thread.join().unwrap().header("title").unwrap().to_string();
        assert!(title.starts_with("=?utf-8?B?") && title.ends_with("?="), "{title}");
    }

    #[test]
    fn slack_gets_json_with_its_text_key() {
        let (port, thread) = server("200 OK", "ok");
        let hook = hook(Kind::Slack, port, "/services/T000/B000/XXXX", Some("unused"));
        send(&hook, &post(Tone::Warning)).unwrap();
        let got = thread.join().unwrap();
        assert_eq!(got.request_line, "POST /services/T000/B000/XXXX HTTP/1.1");
        assert_eq!(got.header("content-type"), Some("application/json"));
        // Slack has its token in the address.
        assert_eq!(got.header("authorization"), None);
        let sent: serde_json::Value = serde_json::from_str(&got.body).unwrap();
        assert_eq!(
            sent,
            json!({"text": "[server] CRITICAL: /data is 97% full\nRaised, critical: /data is 97% full\n  Free space.\n"})
        );
    }

    #[test]
    fn discord_gets_json_with_its_content_key() {
        let (port, thread) = server("204 No Content", "");
        send(&hook(Kind::Discord, port, "/api/webhooks/1/abc", None), &post(Tone::Calm)).unwrap();
        let got = thread.join().unwrap();
        assert_eq!(got.request_line, "POST /api/webhooks/1/abc HTTP/1.1");
        let sent: serde_json::Value = serde_json::from_str(&got.body).unwrap();
        assert!(sent["content"].as_str().unwrap().starts_with("[server] CRITICAL: /data"));
    }

    #[test]
    fn a_discord_message_is_cut_to_2000_characters_with_an_ellipsis() {
        let (port, thread) = server("204 No Content", "");
        let long = "\u{e9}".repeat(3000);
        let mut message = post(Tone::Warning);
        message.body = &long;
        send(&hook(Kind::Discord, port, "/w", None), &message).unwrap();
        let sent: serde_json::Value = serde_json::from_str(&thread.join().unwrap().body).unwrap();
        let content = sent["content"].as_str().unwrap();
        assert_eq!(content.chars().count(), 2000);
        assert!(content.ends_with('\u{2026}'));
        // One that fits is not cut.
        assert_eq!(shortened("short", 2000), "short");
        assert_eq!(shortened(&"x".repeat(2000), 2000).len(), 2000);
        assert_eq!(shortened(&"x".repeat(2001), 2000).chars().count(), 2000);
    }

    #[test]
    fn matrix_puts_a_message_in_the_room_with_the_room_percent_encoded() {
        let (port, thread) = server("200 OK", r#"{"event_id":"$x"}"#);
        send(&hook(Kind::Matrix, port, "", Some("syt_secret")), &post(Tone::Warning)).unwrap();
        let got = thread.join().unwrap();
        assert_eq!(
            got.request_line,
            "PUT /_matrix/client/v3/rooms/%21abc%3Aexample.org/send/m.room.message/1763030531000-0 \
             HTTP/1.1"
        );
        assert_eq!(got.header("authorization"), Some("Bearer syt_secret"));
        assert_eq!(got.header("content-type"), Some("application/json"));
        let sent: serde_json::Value = serde_json::from_str(&got.body).unwrap();
        assert_eq!(sent["msgtype"], "m.text");
        assert!(sent["body"].as_str().unwrap().starts_with("[server] CRITICAL: /data"));
    }

    #[test]
    fn a_homeserver_with_a_path_and_a_closing_slash_keeps_its_path() {
        let (port, thread) = server("200 OK", "{}");
        send(&hook(Kind::Matrix, port, "/chat/", Some("t")), &post(Tone::Warning)).unwrap();
        let line = thread.join().unwrap().request_line;
        assert!(line.starts_with("PUT /chat/_matrix/client/v3/rooms/"), "{line}");
    }

    #[test]
    fn each_message_to_matrix_has_its_own_transaction_and_a_retry_has_the_same() {
        assert_ne!(new_id(5), new_id(5));
        let hook = hook(Kind::Matrix, 1, "", Some("t"));
        let first = request(&hook, &post(Tone::Warning)).unwrap();
        let again = request(&hook, &post(Tone::Warning)).unwrap();
        assert_eq!(first, again);
    }

    #[test]
    fn percent_encoding_keeps_only_unreserved_characters() {
        assert_eq!(percent_encoded("!abc:example.org"), "%21abc%3Aexample.org");
        assert_eq!(percent_encoded("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encoded("a/b c"), "a%2Fb%20c");
        assert_eq!(percent_encoded("\u{e9}"), "%C3%A9");
    }

    #[test]
    fn json_gets_the_batch_as_it_is_with_a_bearer_token() {
        let (port, thread) = server("200 OK", "");
        send(&hook(Kind::Json, port, "/alerts", Some("tk")), &post(Tone::Warning)).unwrap();
        let got = thread.join().unwrap();
        assert_eq!(got.request_line, "POST /alerts HTTP/1.1");
        assert_eq!(got.header("content-type"), Some("application/json"));
        assert_eq!(got.header("authorization"), Some("Bearer tk"));
        assert_eq!(got.body, post(Tone::Warning).json);
    }

    #[test]
    fn any_2xx_is_delivered() {
        for status in ["200 OK", "201 Created", "202 Accepted", "204 No Content"] {
            let (port, thread) = server(status, "");
            send(&hook(Kind::Json, port, "/", None), &post(Tone::Warning)).unwrap();
            thread.join().unwrap();
        }
    }

    #[test]
    fn a_403_is_reported_with_its_status_and_the_services_words() {
        let (port, thread) = server("403 Forbidden", r#"{"code":40301,"error":"forbidden"}"#);
        let hook = hook(Kind::Ntfy, port, "/topic", Some("tk_secret"));
        let refused = send(&hook, &post(Tone::Warning)).unwrap_err();
        assert_eq!(refused, r#"ntfy answered 403 Forbidden: {"code":40301,"error":"forbidden"}"#);
        thread.join().unwrap();
        assert!(!refused.contains("tk_secret") && !refused.contains("/topic"));
    }

    #[test]
    fn a_refusal_without_words_gives_the_status_alone_and_long_words_are_cut() {
        assert_eq!(refusal(Kind::Slack, 404, ""), "slack answered 404 Not Found");
        assert_eq!(refusal(Kind::Json, 599, "  "), "json answered 599");
        let long = refusal(Kind::Discord, 400, &"x".repeat(900));
        assert_eq!(long, format!("discord answered 400 Bad Request: {}...", "x".repeat(200)));
        // HTML on a line.
        assert_eq!(
            refusal(Kind::Json, 502, "<html>\n<body>Bad gateway</body>\n</html>"),
            "json answered 502 Bad Gateway: <html> <body>Bad gateway</body> </html>"
        );
    }

    #[test]
    fn a_server_that_is_not_there_is_named_by_its_host_alone() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let refused =
            send(&hook(Kind::Ntfy, port, "/secret-topic", Some("tk_secret")), &post(Tone::Warning))
                .unwrap_err();
        assert_eq!(refused, "cannot reach 127.0.0.1: Connection refused");
    }

    #[test]
    fn an_address_that_is_not_a_url_is_refused_without_being_shown() {
        let hook = Webhook {
            kind: Kind::Slack,
            url: "not a url with a token".into(),
            token: None,
            room: None,
        };
        assert_eq!(
            send(&hook, &post(Tone::Warning)).unwrap_err(),
            "the address in alerts.toml is not a URL this takes"
        );
    }

    #[test]
    fn a_matrix_hook_without_a_room_is_refused() {
        let mut hook = hook(Kind::Matrix, 1, "", Some("t"));
        hook.room = None;
        assert_eq!(
            send(&hook, &post(Tone::Warning)).unwrap_err(),
            "the Matrix room is missing in alerts.toml"
        );
    }
}
