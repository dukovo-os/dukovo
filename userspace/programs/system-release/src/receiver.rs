// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The canaries' receiver on the update server (ADR 0533):
//! `system-release canary-receive`, behind the web server on a unix
//! socket, as a user of its own that holds no key and has no network.
//! It answers one request, `POST /canary/v1/report`, and keeps a report
//! only once [`canary::verify`] has taken it: so it writes nothing for a
//! key that is not on the list, and at most one file for each canary
//! that is.
//!
//! It reads at most [`HEAD_MAX`] bytes of head and
//! [`canary::REPORT_MAX`] of body, each connection within
//! [`DEADLINE`], with at most [`CONNECTIONS`] at once; it asks for a
//! length and takes no chunked body; and it closes each connection after
//! its answer. It reads the list for each request, so a canary added or
//! removed counts at once.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::canary::{self, Canaries, KeepError};

/// The most bytes of a request's line and headers.
pub const HEAD_MAX: usize = 8 * 1024;

/// How long a connection may take, from accept to answer.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// The most connections served at once; more are answered 503.
pub const CONNECTIONS: usize = 8;

/// The most headers a request may have.
const HEADERS_MAX: usize = 32;

/// Where the receiver reads the list and keeps the reports.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The canary list, root's.
    pub canaries: PathBuf,
    /// The folder of reports, the receiver's.
    pub reports: PathBuf,
}

/// Listens on `socket`, which it makes, readable and writable by its
/// owner and group alone, and answers each connection on a thread of
/// its own. It returns only if the socket fails.
///
/// # Errors
///
/// If the socket cannot be made, or accepting fails.
pub fn serve(socket: &Path, paths: &Paths) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    if let Ok(meta) = std::fs::symlink_metadata(socket) {
        if !meta.file_type().is_socket() {
            return Err(format!("{} is there already and is not a socket", socket.display()));
        }
        std::fs::remove_file(socket).map_err(|err| format!("{}: {err}", socket.display()))?;
    }
    let listener =
        UnixListener::bind(socket).map_err(|err| format!("{}: {err}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))
        .map_err(|err| format!("{}: {err}", socket.display()))?;
    eprintln!("canary-receive: listening on {}", socket.display());
    let busy = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(format!("accepting: {err}")),
        };
        if busy.fetch_add(1, Ordering::SeqCst) >= CONNECTIONS {
            busy.fetch_sub(1, Ordering::SeqCst);
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            let _ = answer(&mut stream, 503, "busy, try again later");
            continue;
        }
        let counter = Arc::clone(&busy);
        let paths = paths.clone();
        let spawned = std::thread::Builder::new().name("canary-receive".into()).spawn(move || {
            serve_one(stream, &paths);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            // The thread was not made, so it never counts itself out; the
            // connection closed with it.
            busy.fetch_sub(1, Ordering::SeqCst);
            eprintln!("canary-receive: no thread for a connection");
        }
    }
    Ok(())
}

/// Answers one connection, and says what became of it in the log.
fn serve_one(mut stream: UnixStream, paths: &Paths) {
    let (status, said) = match receive(&mut stream, paths, tuf::time::now()) {
        Ok(name) => (200, format!("kept {name}'s report")),
        Err((status, why)) => (status, why),
    };
    eprintln!("canary-receive: {status}: {said}");
    let body = if status == 200 { String::new() } else { said };
    let _ = answer(&mut stream, status, &body);
}

/// Reads one request from `stream` and keeps its report: the canary's
/// name, or the status to answer and why.
fn receive(stream: &mut UnixStream, paths: &Paths, now: i64) -> Result<String, (u16, String)> {
    let started = Instant::now();
    let (head, mut body) = read_head(stream, started)?;
    let length = parse_head(&head)?;
    if length > canary::REPORT_MAX {
        return Err((413, canary::Refusal::TooLarge.to_string()));
    }
    if body.len() > length {
        return Err((400, "more bytes than the length said".into()));
    }
    read_rest(stream, &mut body, length, started)?;
    let canaries =
        Canaries::load(&paths.canaries).map_err(|why| (500, format!("the canary list: {why}")))?;
    let verified = canary::verify(&body, &canaries, now)
        .map_err(|refusal| (refusal.status(), refusal.to_string()))?;
    match canary::keep(&paths.reports, &body, &verified, &canaries) {
        Ok(()) => Ok(verified.name),
        Err(KeepError::Refused(refusal)) => Err((refusal.status(), refusal.to_string())),
        Err(KeepError::Failed(why)) => Err((500, why)),
    }
}

/// The time left before `started` + [`DEADLINE`], or 408 if none is.
fn left(started: Instant) -> Result<Duration, (u16, String)> {
    DEADLINE
        .checked_sub(started.elapsed())
        .filter(|left| !left.is_zero())
        .ok_or((408, "too slow".to_string()))
}

/// Reads until the end of the head: the head, and what of the body came
/// with it.
fn read_head(
    stream: &mut UnixStream,
    started: Instant,
) -> Result<(Vec<u8>, Vec<u8>), (u16, String)> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let body = buffer.split_off(end + 4);
            return Ok((buffer, body));
        }
        if buffer.len() >= HEAD_MAX {
            return Err((431, "the head is too large".into()));
        }
        let _ = stream.set_read_timeout(Some(left(started)?));
        let want = chunk.len().min(HEAD_MAX + canary::REPORT_MAX - buffer.len());
        match stream.read(&mut chunk[..want]) {
            Ok(0) => return Err((400, "the connection closed before the head ended".into())),
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err((408, "too slow".into())),
        }
    }
}

/// Checks the head: `POST` to [`canary::REPORT_PATH`], with a length
/// and no chunked body. The length.
fn parse_head(head: &[u8]) -> Result<usize, (u16, String)> {
    let mut headers = [httparse::EMPTY_HEADER; HEADERS_MAX];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) | Err(_) => return Err((400, "not an HTTP request".into())),
    }
    if request.path != Some(canary::REPORT_PATH) {
        return Err((404, "nothing here but the canaries' reports".into()));
    }
    if request.method != Some("POST") {
        return Err((405, "reports are sent with POST".into()));
    }
    let mut length = None;
    for header in request.headers.iter() {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err((411, "send the report with a length".into()));
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            let value = std::str::from_utf8(header.value)
                .ok()
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|v| v.parse::<usize>().ok())
                .ok_or((400, "the length is not a number".to_string()))?;
            if length.is_some_and(|seen| seen != value) {
                return Err((400, "two lengths".into()));
            }
            length = Some(value);
        }
    }
    length.ok_or((411, "send the report with a length".into()))
}

/// Reads the body until it is `length` bytes.
fn read_rest(
    stream: &mut UnixStream,
    body: &mut Vec<u8>,
    length: usize,
    started: Instant,
) -> Result<(), (u16, String)> {
    let mut chunk = [0u8; 4096];
    while body.len() < length {
        let _ = stream.set_read_timeout(Some(left(started)?));
        let want = chunk.len().min(length - body.len());
        match stream.read(&mut chunk[..want]) {
            Ok(0) => return Err((400, "the connection closed before the report ended".into())),
            Ok(read) => body.extend_from_slice(&chunk[..read]),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err((408, "too slow".into())),
        }
    }
    Ok(())
}

/// Writes the answer: `status`, and `words` as JSON's `error` when it is
/// not 200, then closes.
fn answer(stream: &mut UnixStream, status: u16, words: &str) -> std::io::Result<()> {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let body = if status == 200 {
        "{\"kept\":true}\n".to_string()
    } else {
        let mut body = serde_json::json!({ "error": words }).to_string();
        body.push('\n');
        body
    };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    );
    if status == 405 {
        head.push_str("Allow: POST\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)?;
    // A lingering close: what the client still sends after an early
    // answer is read and dropped, briefly and up to a limit, so that the
    // close does not reset the connection before the answer is read.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut sink = [0u8; 4096];
    let mut drained = 0;
    while drained < HEAD_MAX + canary::REPORT_MAX {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(read) => drained += read,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canary::{Key, Report};
    use crate::list::tests::scratch;

    /// Sends `request` to the receiver at `socket`: the status and body.
    fn send(socket: &Path, request: &[u8]) -> (u16, String) {
        let mut stream = UnixStream::connect(socket).unwrap();
        stream.write_all(request).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let status = answer[9..12].parse().unwrap();
        let body = answer.split_once("\r\n\r\n").unwrap().1.to_string();
        (status, body)
    }

    fn post(body: &[u8]) -> Vec<u8> {
        let mut request = format!(
            "POST /canary/v1/report HTTP/1.1\r\nHost: updates\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        request
    }

    #[test]
    fn the_receiver_keeps_a_listed_canarys_newer_report_and_nothing_else() {
        let dir = scratch("receiver");
        let paths = Paths { canaries: dir.join("canaries.json"), reports: dir.join("reports") };
        std::fs::create_dir(&paths.reports).unwrap();
        let ref_box = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
        let mut canaries = Canaries::default();
        canaries.add("ref-box", &ref_box.public_hex(), 0).unwrap();
        canaries.save(&paths.canaries).unwrap();
        let socket = dir.join("receive.sock");
        let serving = (socket.clone(), paths.clone());
        std::thread::spawn(move || serve(&serving.0, &serving.1));
        while !socket.exists() {
            std::thread::yield_now();
        }

        let now = tuf::time::now();
        let report = |time| {
            let mut report = Report::new("ref-box", time, "0.16.0", "beta");
            report.services_failed = vec!["none".into()];
            ref_box.seal(&report)
        };
        assert_eq!(send(&socket, &post(&report(now - 60))), (200, "{\"kept\":true}\n".into()));
        let kept = std::fs::read(paths.reports.join("ref-box.json")).unwrap();
        // Ed25519 signs the same bytes the same way: the exact envelope.
        assert_eq!(kept, report(now - 60));
        assert_eq!(send(&socket, &post(&report(now - 60))).0, 409);
        assert_eq!(send(&socket, &post(&report(now))).0, 200);

        let stranger = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
        let theirs = stranger.seal(&Report::new("ref-box", now, "0.16.0", "beta"));
        let (status, body) = send(&socket, &post(&theirs));
        assert_eq!(status, 403);
        assert!(body.contains("no canary on the list has that key"), "{body}");
        let files: Vec<_> = std::fs::read_dir(&paths.reports).unwrap().collect();
        assert_eq!(files.len(), 1);

        let large = format!(
            "POST /canary/v1/report HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            canary::REPORT_MAX + 1
        );
        assert_eq!(send(&socket, large.as_bytes()).0, 413);
        assert_eq!(send(&socket, b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").0, 404);
        let huge_head =
            format!("POST /canary/v1/report HTTP/1.1\r\nX: {}\r\n", "a".repeat(HEAD_MAX));
        assert_eq!(send(&socket, huge_head.as_bytes()).0, 431);

        // A canary taken off the list is refused at once.
        Canaries::default().save(&paths.canaries).unwrap();
        assert_eq!(send(&socket, &post(&report(now + 1))).0, 403);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_head_must_be_a_post_to_the_one_path_with_a_length() {
        let ok = b"POST /canary/v1/report HTTP/1.1\r\nHost: x\r\nContent-Length: 12\r\n\r\n";
        assert_eq!(parse_head(ok), Ok(12));
        let get = b"GET /canary/v1/report HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(parse_head(get).unwrap_err().0, 405);
        let other = b"POST /canary/v1/report?x=1 HTTP/1.1\r\nContent-Length: 1\r\n\r\n";
        assert_eq!(parse_head(other).unwrap_err().0, 404);
        let chunked = b"POST /canary/v1/report HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert_eq!(parse_head(chunked).unwrap_err().0, 411);
        let none = b"POST /canary/v1/report HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(parse_head(none).unwrap_err().0, 411);
        let two =
            b"POST /canary/v1/report HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n";
        assert_eq!(parse_head(two).unwrap_err().0, 400);
        let signed = b"POST /canary/v1/report HTTP/1.1\r\nContent-Length: +1\r\n\r\n";
        assert_eq!(parse_head(signed).unwrap_err().0, 400);
        assert_eq!(parse_head(b"\x00\x01garbage\r\n\r\n").unwrap_err().0, 400);
    }
}
