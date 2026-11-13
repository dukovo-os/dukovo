// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The webhook receiver of the boot test: it reads one HTTP/1.1 request
//! on each connection, writes it to [`HOOKS`](crate::records::HOOKS) with
//! its method, path, the kind of webhook the path is for, its headers and
//! body, and answers `200` and `{}` as
//! Slack, ntfy and Matrix do, or `403` and a JSON error to a path that
//! begins with [`DENIED`], as a service does that refuses a token.

use std::io::{self, BufRead, BufReader, Read, Write};

use crate::config::{DENIED, kind_of};
use crate::records::{HOOKS, Records};

/// The longest a request's head may be, and its body.
const HEAD_MAX: u64 = 16 * 1024;
const BODY_MAX: usize = 1 << 20;

/// A request, as the file records it.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    path: String,
    /// Each header, its name in lower case.
    headers: Vec<(String, String)>,
    body: String,
}

/// What a request that cannot be taken is answered with.
struct Bad {
    status: &'static str,
    why: &'static str,
}

/// Reads the request on `stream`, records it, and answers it. `tls` says
/// whether `stream` is encrypted, which the record says too.
///
/// # Errors
///
/// Why the request could not be read or the answer written.
pub fn serve<S: Read + Write>(stream: &mut S, tls: bool, records: &Records) -> io::Result<()> {
    let request = read_request(&mut BufReader::new(&mut *stream));
    let answer = match request {
        Ok(request) => {
            records.add(HOOKS, &block(&request, tls))?;
            if request.path.starts_with(DENIED) {
                response("403 Forbidden", "{\"error\":\"forbidden\"}")
            } else {
                response("200 OK", "{}")
            }
        }
        Err(Bad { status, why }) => response(status, &format!("{{\"error\":\"{why}\"}}")),
    };
    stream.write_all(answer.as_bytes())?;
    stream.flush()
}

/// The answer with `status` and the JSON `body`, after which the
/// connection ends.
fn response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The request in `reader`: its head, and as much body as it says.
fn read_request<R: BufRead>(reader: &mut R) -> Result<Request, Bad> {
    let bad = |why| Bad { status: "400 Bad Request", why };
    let mut head = reader.take(HEAD_MAX);
    let mut line = String::new();
    head.read_line(&mut line).map_err(|_| bad("the request line cannot be read"))?;
    let mut words = line.split_whitespace();
    let (Some(method), Some(path), Some(version)) = (words.next(), words.next(), words.next())
    else {
        return Err(bad("not a request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(bad("not HTTP/1"));
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let got = head.read_line(&mut line).map_err(|_| bad("a header cannot be read"))?;
        if got == 0 {
            return Err(bad("the request ends before its head does"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| bad("not a header"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let length = match headers.iter().find(|(name, _)| name == "content-length") {
        None => 0,
        Some((_, value)) => {
            value.parse::<usize>().map_err(|_| bad("Content-Length is no number"))?
        }
    };
    if length > BODY_MAX {
        return Err(Bad { status: "413 Payload Too Large", why: "the body is over 1 MiB" });
    }
    let mut body = vec![0; length];
    // What the head's reader buffered is the start of the body.
    let reader = head.into_inner();
    reader.read_exact(&mut body).map_err(|_| bad("the body is shorter than Content-Length"))?;
    Ok(Request {
        method: method.to_string(),
        path: path.to_string(),
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// What goes in the file for `request`.
fn block(request: &Request, tls: bool) -> String {
    let mut block = String::from("== hook\n");
    block.push_str(&format!("request: {} {}\n", request.method, request.path));
    block.push_str(&format!("kind: {}\n", kind_of(&request.path)));
    block.push_str(&format!("tls: {}\n", if tls { "yes" } else { "no" }));
    for (name, value) in &request.headers {
        block.push_str(&format!("{name}: {value}\n"));
    }
    block.push_str("-- body\n");
    block.push_str(&request.body);
    if !request.body.ends_with('\n') {
        block.push('\n');
    }
    block.push_str("== end\n");
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::io::Cursor;

    /// A connection of bytes: what the client sent, and what was
    /// answered.
    struct Pipe {
        sent: Cursor<Vec<u8>>,
        answered: Vec<u8>,
    }

    impl Read for Pipe {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.sent.read(buffer)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.answered.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Serves `request`; gives the answer and what the file `hooks.txt`
    /// then holds.
    fn serve_one(name: &str, request: &str, tls: bool) -> (String, String) {
        let dir =
            std::env::temp_dir().join(format!("monitoring-http-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let records = Records::start(&dir).unwrap();
        let mut pipe =
            Pipe { sent: Cursor::new(request.as_bytes().to_vec()), answered: Vec::new() };
        serve(&mut pipe, tls, &records).unwrap();
        let recorded = fs::read_to_string(dir.join(HOOKS)).unwrap();
        fs::remove_dir_all(dir).unwrap();
        (String::from_utf8(pipe.answered).unwrap(), recorded)
    }

    #[test]
    fn a_request_is_recorded_with_its_headers_and_body_and_answered_200() {
        let (answer, recorded) = serve_one(
            "ok",
            "POST /slack-hook HTTP/1.1\r\nHost: 127.0.0.1:8443\r\nContent-Type: application/json\r\n\
             Content-Length: 15\r\n\r\n{\"text\":\"hi\"}  ",
            true,
        );
        assert_eq!(
            answer,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\
             Connection: close\r\n\r\n{}"
        );
        assert_eq!(
            recorded,
            "== hook\nrequest: POST /slack-hook\nkind: slack\ntls: yes\nhost: 127.0.0.1:8443\n\
             content-type: application/json\ncontent-length: 15\n-- body\n{\"text\":\"hi\"}  \n== end\n"
        );
    }

    #[test]
    fn plain_http_is_recorded_as_not_tls_and_a_bearer_token_is_kept() {
        let (_, recorded) = serve_one(
            "plain",
            "PUT /json-hook HTTP/1.1\r\nAuthorization: Bearer json-token\r\nContent-Length: 0\r\n\r\n",
            false,
        );
        assert!(recorded.contains("tls: no\n"), "{recorded}");
        assert!(recorded.contains("authorization: Bearer json-token\n"), "{recorded}");
    }

    #[test]
    fn the_denied_path_is_answered_403_with_a_json_error() {
        let (answer, recorded) = serve_one(
            "denied",
            &format!("POST {DENIED}-hook HTTP/1.1\r\nContent-Length: 0\r\n\r\n"),
            true,
        );
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{answer}");
        assert!(answer.ends_with("{\"error\":\"forbidden\"}"), "{answer}");
        // It was heard all the same.
        assert!(recorded.contains("request: POST /denied-hook\n"), "{recorded}");
    }

    #[test]
    fn what_is_not_a_request_is_answered_400_and_not_recorded() {
        for (name, request) in [
            ("line", "hello\r\n\r\n"),
            ("header", "POST / HTTP/1.1\r\nno colon\r\n\r\n"),
            ("short", "POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc"),
            ("ended", "POST / HTTP/1.1\r\nHost: x\r\n"),
        ] {
            let (answer, recorded) = serve_one(name, request, true);
            assert!(answer.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{name}: {answer}");
            assert_eq!(recorded, "", "{name}");
        }
    }

    #[test]
    fn a_body_of_more_than_a_mebibyte_is_refused() {
        let (answer, recorded) =
            serve_one("big", "POST / HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n", true);
        assert!(answer.starts_with("HTTP/1.1 413 "), "{answer}");
        assert_eq!(recorded, "");
    }
}
