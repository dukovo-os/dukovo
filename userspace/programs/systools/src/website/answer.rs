// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The check that a website answers (ADR 0569): a file `website` put in
//! the site's folder, asked for over this machine's own address, port 80
//! with each of the site's names as the `Host:`, and for a site served
//! over HTTPS port 443 with each name as the TLS server name, the
//! certificate and the name checked as a visitor's browser checks them.
//! An HTTPS site's port 80 must send visitors on to the same name over
//! HTTPS. A PHP file that prints a word proves PHP-FPM answers too.
//!
//! The request is HTTP/1.0 with a `Host:`, so that nginx answers with the
//! length of the page or by closing the connection, never in chunks, and
//! the answer is read to its end. What a name answers is tried again for
//! as long as the host's patience says, as the new workers of a reload
//! take a moment to take over.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::host::Host;
use crate::tls;

/// How long one connection gets for each of its steps.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// The most bytes of an answer that are read.
const ANSWER_MAX: u64 = 64 * 1024;

/// One request to this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch {
    /// The port: 80, or 443 for TLS.
    pub port: u16,
    /// The name asked for: the `Host:`, and for TLS the server name.
    pub host: String,
    /// The path asked for.
    pub path: String,
    /// The certificate the server's must be chained to and named for,
    /// when the request is over TLS.
    pub certificate: Option<String>,
}

/// What came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The status code.
    pub status: u16,
    /// Where it sends the visitor on, if it does.
    pub location: Option<String>,
    /// The page, at most 64 KiB of it.
    pub body: Vec<u8>,
}

/// The text of the request for `fetch`.
pub fn request(fetch: &Fetch) -> String {
    format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: website\r\nConnection: close\r\n\r\n",
        fetch.path, fetch.host
    )
}

/// What the bytes `bytes` of an answer say.
///
/// # Errors
///
/// When they are not an HTTP answer.
pub fn parse_reply(bytes: &[u8]) -> Result<Reply, String> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the answer has no end of its headers")?;
    let head = String::from_utf8_lossy(&bytes[..split]);
    let mut lines = head.lines();
    let status = lines.next().unwrap_or_default();
    let code = status
        .strip_prefix("HTTP/")
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("{status:?} is not an HTTP status line"))?;
    let location = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("location").then(|| value.trim().to_string())
    });
    Ok(Reply { status: code, location, body: bytes[split + 4..].to_vec() })
}

/// Reads `stream` to its end, at most [`ANSWER_MAX`] bytes. A peer that
/// closes without TLS's goodbye ends the answer as any close does.
fn read_all(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match stream.take(ANSWER_MAX).read_to_end(&mut bytes) {
        Ok(_) => Ok(bytes),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof && !bytes.is_empty() => {
            Ok(bytes)
        }
        Err(error) => Err(error),
    }
}

/// Asks this machine's own port as `fetch` says. `extra` are PEM files of
/// certificates to trust besides the system's roots.
///
/// # Errors
///
/// Why there was no answer: the connection, the certificate, the name or
/// the answer itself.
pub fn fetch(fetch: &Fetch, extra: &[String]) -> Result<Reply, String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, fetch.port));
    let text = request(fetch);
    let failed = |error: io::Error| crate::util::strip_os_error(&error);
    let bytes = if fetch.certificate.is_some() {
        let name = tls::server_name(Some(&fetch.host), address).map_err(failed)?;
        let config = tls::verified(tls::roots(extra, "website"), &[]);
        let mut stream = tls::connect(address, name, &config, CONNECT_WAIT).map_err(|error| {
            // A certificate that is refused says why in rustls's words.
            let inner = error.get_ref().map(ToString::to_string);
            inner.unwrap_or_else(|| failed(error))
        })?;
        tls::send(&mut stream, text.as_bytes()).map_err(failed)?;
        read_all(&mut stream).map_err(failed)?
    } else {
        let mut stream = TcpStream::connect_timeout(&address, CONNECT_WAIT).map_err(failed)?;
        stream.set_read_timeout(Some(CONNECT_WAIT)).map_err(failed)?;
        stream.set_write_timeout(Some(CONNECT_WAIT)).map_err(failed)?;
        stream.write_all(text.as_bytes()).map_err(failed)?;
        read_all(&mut stream).map_err(failed)?
    };
    parse_reply(&bytes)
}

/// What a check file is asked for and must answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The site's names.
    pub names: Vec<String>,
    /// The certificate it is served with over HTTPS, if it is.
    pub certificate: Option<String>,
    /// The file of text in the folder: its path as asked for, and what
    /// it holds.
    pub text: (String, String),
    /// The PHP file that prints a word, for a site of PHP.
    pub php: Option<(String, String)>,
}

/// What a request must come back as.
enum Expect<'a> {
    /// A page that holds `content`.
    Page(&'a str),
    /// A redirect to HTTPS, to the same name.
    Redirect,
}

/// Why `reply` is not what `expect` says for `name`, or none when it is.
fn unmet(reply: &Reply, expect: &Expect<'_>, name: &str) -> Option<String> {
    match expect {
        Expect::Page(content) => {
            if reply.status != 200 {
                return Some(format!("answered {}", reply.status));
            }
            (reply.body.as_slice() != content.as_bytes())
                .then(|| "answered a page that is not the check's file".to_string())
        }
        Expect::Redirect => {
            let to = format!("https://{name}/");
            if reply.status != 301 {
                return Some(format!("answered {}, not a redirect to {to}", reply.status));
            }
            (!reply.location.as_deref().is_some_and(|at| at.starts_with(&to)))
                .then(|| format!("answered 301, but not to {to}"))
        }
    }
}

/// Asks until `fetch` is answered as `expect` says, or the host's patience
/// is spent; the words say which port and name and what came back.
fn until(host: &dyn Host, fetch: &Fetch, expect: &Expect<'_>) -> Result<(), String> {
    let deadline = Instant::now() + host.patience();
    loop {
        let why = match host.fetch(fetch) {
            Ok(reply) => match unmet(&reply, expect, &fetch.host) {
                None => return Ok(()),
                Some(why) => why,
            },
            Err(why) => format!("did not answer: {why}"),
        };
        if Instant::now() >= deadline {
            return Err(format!("port {} for {} {why}", fetch.port, fetch.host));
        }
        host.pause();
    }
}

/// Checks that the site answers as `probe` says.
///
/// # Errors
///
/// The first thing that does not, as `port 80 for NAME answered 404`.
pub fn check(host: &dyn Host, probe: &Probe) -> Result<(), String> {
    let (path, content) = &probe.text;
    let ask = |port: u16, name: &str, path: &str, certificate: Option<&String>| Fetch {
        port,
        host: name.to_string(),
        path: path.to_string(),
        certificate: certificate.cloned(),
    };
    for name in &probe.names {
        match &probe.certificate {
            Some(certificate) => {
                until(host, &ask(80, name, path, None), &Expect::Redirect)?;
                until(host, &ask(443, name, path, Some(certificate)), &Expect::Page(content))?;
            }
            None => until(host, &ask(80, name, path, None), &Expect::Page(content))?,
        }
    }
    if let (Some((path, content)), Some(first)) = (&probe.php, probe.names.first()) {
        let (port, certificate) = match &probe.certificate {
            Some(certificate) => (443, Some(certificate)),
            None => (80, None),
        };
        until(host, &ask(port, first, path, certificate), &Expect::Page(content))
            .map_err(|why| format!("{why} (PHP-FPM may not be answering)"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn the_request_is_http_one_oh_with_the_name_as_its_host() {
        let ask = Fetch {
            port: 80,
            host: "shop.example.org".into(),
            path: "/a.txt".into(),
            certificate: None,
        };
        assert_eq!(
            request(&ask),
            "GET /a.txt HTTP/1.0\r\nHost: shop.example.org\r\nUser-Agent: website\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn an_answer_gives_its_status_its_redirect_and_its_page() {
        let reply = parse_reply(
            b"HTTP/1.1 301 Moved Permanently\r\nServer: nginx\r\nlocation: https://a/x\r\n\r\n",
        )
        .unwrap();
        assert_eq!((reply.status, reply.location.as_deref()), (301, Some("https://a/x")));
        assert!(reply.body.is_empty());
        let reply = parse_reply(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").unwrap();
        assert_eq!((reply.status, reply.body.as_slice(), reply.location), (200, &b"abc"[..], None));
        for bad in [
            &b""[..],
            b"HTTP/1.1 200 OK\r\n",
            b"nothing\r\n\r\n",
            b"HTTP/1.1 abc\r\n\r\n",
            b"HTTP/1.1\r\n\r\n",
        ] {
            assert!(parse_reply(bad).is_err(), "{bad:?}");
        }
    }

    /// Serves `answers` one after another, each to one connection, and
    /// gives what each connection asked.
    fn serve(answers: Vec<&'static str>) -> (u16, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let mut asked = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut seen = Vec::new();
                let mut buffer = [0u8; 512];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    seen.extend_from_slice(&buffer[..n]);
                }
                asked.push(String::from_utf8_lossy(&seen).into_owned());
                stream.write_all(answer.as_bytes()).unwrap();
            }
            asked
        });
        (port, handle)
    }

    #[test]
    fn a_page_is_fetched_over_plain_http_with_the_host_it_names() {
        let (port, server) = serve(vec!["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"]);
        let ask = Fetch {
            port,
            host: "blog.example.org".into(),
            path: "/check.txt".into(),
            certificate: None,
        };
        let reply = fetch(&ask, &[]).unwrap();
        assert_eq!((reply.status, reply.body.as_slice()), (200, &b"hello"[..]));
        let asked = server.join().unwrap();
        assert!(
            asked[0].starts_with("GET /check.txt HTTP/1.0\r\nHost: blog.example.org\r\n"),
            "{asked:?}"
        );
    }

    #[test]
    fn nothing_listening_is_an_answer_that_says_so() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let ask = Fetch { port, host: "a".into(), path: "/".into(), certificate: None };
        assert!(fetch(&ask, &[]).is_err());
    }

    use std::cell::RefCell;

    /// What answers a request, given the request and how many came before.
    type Answering = Box<dyn Fn(&Fetch, usize) -> Result<Reply, String>>;

    /// A host that answers each request from a function.
    struct Script {
        answer: Answering,
        seen: RefCell<Vec<Fetch>>,
        patience: Duration,
    }

    impl Script {
        fn new(
            patience: Duration,
            answer: impl Fn(&Fetch, usize) -> Result<Reply, String> + 'static,
        ) -> Script {
            Script { answer: Box::new(answer), seen: RefCell::new(Vec::new()), patience }
        }
    }

    impl Host for Script {
        fn accounts(&self) -> Vec<accounts::User> {
            Vec::new()
        }
        fn installed(&self, _: &str, _: &str) -> bool {
            true
        }
        fn state(&self, _: &str) -> Result<Option<super::super::host::Svc>, String> {
            Ok(None)
        }
        fn service_check(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn restart(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn reload(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn turn(&self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn nginx_check(&self, _: &str) -> Result<super::super::host::Checked, String> {
            Err("not asked".into())
        }
        fn certificate_names(&self, _: &str) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn firewall(&self, _: u16) -> super::super::host::Rule {
            super::super::host::Rule { allowed: true, rule: None, text: String::new() }
        }
        fn fetch(&self, request: &Fetch) -> Result<Reply, String> {
            let tries = self.seen.borrow().iter().filter(|seen| *seen == request).count();
            self.seen.borrow_mut().push(request.clone());
            (self.answer)(request, tries)
        }
        fn patience(&self) -> Duration {
            self.patience
        }
        fn pause(&self) {}
        fn log(&self, _: &str) {}
        fn now(&self) -> i64 {
            0
        }
        fn measure(
            &self,
            _: &[crate::bundles::sizes::Request],
        ) -> Result<Vec<crate::bundles::sizes::Line>, String> {
            Err("not asked".into())
        }
    }

    fn page(content: &str) -> Reply {
        Reply { status: 200, location: None, body: content.as_bytes().to_vec() }
    }

    fn probe(certificate: Option<&str>, php: bool) -> Probe {
        Probe {
            names: vec!["a.example.org".into(), "www.a.example.org".into()],
            certificate: certificate.map(Into::into),
            text: ("/website-check-1.txt".into(), "content".into()),
            php: php.then(|| ("/website-check-1.php".into(), "printed".into())),
        }
    }

    #[test]
    fn an_http_site_must_answer_the_check_file_under_each_of_its_names() {
        let host = Script::new(Duration::ZERO, |ask, _| {
            Ok(page(if ask.path.ends_with("txt") { "content" } else { "printed" }))
        });
        assert_eq!(check(&host, &probe(None, true)), Ok(()));
        let seen = host.seen.borrow();
        let asked: Vec<(u16, &str, &str)> =
            seen.iter().map(|f| (f.port, f.host.as_str(), f.path.as_str())).collect();
        assert_eq!(
            asked,
            [
                (80, "a.example.org", "/website-check-1.txt"),
                (80, "www.a.example.org", "/website-check-1.txt"),
                (80, "a.example.org", "/website-check-1.php"),
            ]
        );
    }

    #[test]
    fn an_https_site_redirects_on_port_80_and_answers_on_443_with_its_certificate() {
        let host = Script::new(Duration::ZERO, |ask, _| match (ask.port, &ask.certificate) {
            (80, None) => Ok(Reply {
                status: 301,
                location: Some(format!("https://{}{}", ask.host, ask.path)),
                body: Vec::new(),
            }),
            (443, Some(certificate)) if certificate == "shop-cert" => Ok(page("content")),
            other => Err(format!("unexpected {other:?}")),
        });
        assert_eq!(check(&host, &probe(Some("shop-cert"), false)), Ok(()));
        assert_eq!(host.seen.borrow().len(), 4);
    }

    #[test]
    fn what_does_not_answer_as_it_should_is_told_with_its_port_name_and_answer() {
        let not_found = Script::new(Duration::ZERO, |_, _| {
            Ok(Reply { status: 404, location: None, body: Vec::new() })
        });
        assert_eq!(
            check(&not_found, &probe(None, false)),
            Err("port 80 for a.example.org answered 404".into())
        );
        let other = Script::new(Duration::ZERO, |_, _| Ok(page("some other site")));
        assert_eq!(
            check(&other, &probe(None, false)),
            Err("port 80 for a.example.org answered a page that is not the check's file".into())
        );
        let down = Script::new(Duration::ZERO, |_, _| Err("Connection refused".into()));
        assert_eq!(
            check(&down, &probe(None, false)),
            Err("port 80 for a.example.org did not answer: Connection refused".into())
        );
        let no_redirect = Script::new(Duration::ZERO, |_, _| Ok(page("content")));
        assert_eq!(
            check(&no_redirect, &probe(Some("c"), false)),
            Err("port 80 for a.example.org answered 200, not a redirect to https://a.example.org/"
                .into())
        );
        let elsewhere = Script::new(Duration::ZERO, |_, _| {
            Ok(Reply {
                status: 301,
                location: Some("https://other.example.org/x".into()),
                body: Vec::new(),
            })
        });
        assert_eq!(
            check(&elsewhere, &probe(Some("c"), false)),
            Err("port 80 for a.example.org answered 301, but not to https://a.example.org/".into())
        );
        let no_php = Script::new(Duration::ZERO, |ask, _| {
            Ok(if ask.path.ends_with("php") { page("<?php echo") } else { page("content") })
        });
        let why = check(&no_php, &probe(None, true)).unwrap_err();
        assert!(why.starts_with("port 80 for a.example.org answered a page that is not"), "{why}");
        assert!(why.ends_with("(PHP-FPM may not be answering)"), "{why}");
    }

    #[test]
    fn a_name_that_answers_late_is_asked_again_within_the_patience() {
        let host = Script::new(Duration::from_secs(5), |_, tries| {
            if tries < 3 {
                Ok(Reply { status: 404, location: None, body: Vec::new() })
            } else {
                Ok(page("content"))
            }
        });
        assert_eq!(check(&host, &probe(None, false)), Ok(()));
        // Each name is asked four times: three misses, then the answer.
        assert_eq!(host.seen.borrow().len(), 8);
    }
}
