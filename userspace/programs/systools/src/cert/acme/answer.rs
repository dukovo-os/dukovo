// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `acme-answer`: the responder of HTTP-01 (ADR 0513).
//!
//! An authority that checks a name asks `http://NAME/.well-known/acme-challenge/TOKEN`
//! and expects the key authorization it was told of. `cert get` and certd
//! start this program for the length of an order, in a view of its own that
//! holds only the program, with the two listening sockets of port 80 and the
//! answers on its standard input: a line `TOKEN KEYAUTH` for each, then an
//! empty line. It serves until its standard input closes, then ends. It
//! never sees a key, a file or the network beyond its sockets, so a mistake
//! in reading a stranger's request reaches nothing worth having.
//!
//! The reading is small and strict: a `GET` or `HEAD` of a token it holds
//! is answered 200; a request that is no request, or whose head is longer
//! than 8 KiB, 400; anything else 404. A connection lasts 10 seconds at the
//! most and answers one request; 64 may be open at once.
//!
//! The caller's half, [`Responder`], builds the view and starts the program.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::args::{Arg, Args};
use crate::util::{Fail, fail, usage};

/// The path that is asked for, without the token.
const CHALLENGE_PATH: &str = "/.well-known/acme-challenge/";

/// The longest token, in characters (base64url, RFC 8555 section 8.1).
const TOKEN_MAX: usize = 128;

/// The longest key authorization it takes: a token, a dot and a SHA-256
/// thumbprint in base64url, with room to spare.
const KEYAUTH_MAX: usize = 512;

/// The most answers it takes: one for each of the 100 names an order has.
const ANSWERS_MAX: usize = 100;

/// The most bytes read and dropped after an answer.
const LINGER_MAX: usize = 64 * 1024;

/// What a connection may take.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The most bytes in a request's head: its line and headers.
    pub head: usize,
    /// How long a connection lasts, whatever it does.
    pub connection: Duration,
    /// How many connections are served at once.
    pub connections: usize,
}

/// The limits the program keeps.
pub const LIMITS: Limits =
    Limits { head: 8 * 1024, connection: Duration::from_secs(10), connections: 64 };

/// The answers: each token with the key authorization served for it.
pub type Answers = BTreeMap<String, String>;

/// Whether `token` is one: 1 to 128 base64url characters.
pub fn is_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= TOKEN_MAX
        && token.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// What is answered to a request's head, the bytes of its line and
/// headers up to the empty line.
pub fn reply(head: &[u8], answers: &Answers) -> Vec<u8> {
    let Some((method, target)) = request_line(head) else { return status(400, "Bad Request") };
    if method != "GET" && method != "HEAD" {
        return status(404, "Not Found");
    }
    let Some(token) = target.strip_prefix(CHALLENGE_PATH) else { return status(404, "Not Found") };
    let Some(keyauth) = answers.get(token) else { return status(404, "Not Found") };
    let mut bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        keyauth.len()
    )
    .into_bytes();
    if method == "GET" {
        bytes.extend_from_slice(keyauth.as_bytes());
    }
    bytes
}

/// An answer with no body, for an `code`.
fn status(code: u16, words: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {words}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

/// The method and target of a request's head, if it is a request:
/// an HTTP/1.0 or 1.1 line of three words, an origin-form target, and
/// headers that each have a name and a colon, in printable text.
fn request_line(head: &[u8]) -> Option<(&str, &str)> {
    if head.iter().any(|&byte| {
        byte == 0 || (byte < 0x20 && !matches!(byte, b'\r' | b'\n' | b'\t')) || byte >= 0x7f
    }) {
        return None;
    }
    let text = std::str::from_utf8(head).ok()?;
    // Every line ends with CR LF, and none is a bare CR or LF.
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    if first.contains(['\r', '\n']) {
        return None;
    }
    let mut words = first.split(' ');
    let (method, target, version) = (words.next()?, words.next()?, words.next()?);
    if words.next().is_some() || method.is_empty() || !target.starts_with('/') {
        return None;
    }
    if !method.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return None;
    }
    for line in lines {
        if line.is_empty() {
            break;
        }
        if line.contains(['\r', '\n']) || line.starts_with([' ', '\t']) {
            return None;
        }
        let (name, _) = line.split_once(':')?;
        if name.is_empty() || name.bytes().any(|byte| byte == b' ' || byte == b'\t') {
            return None;
        }
    }
    Some((method, target))
}

/// Serves one connection: reads a head within `limits`, answers it, and
/// closes. A client that does not finish its head in time is cut off
/// without an answer. After the answer what the client still sends is read
/// and dropped, until it closes or time is up, so that closing does not
/// reset the connection under an answer not yet read.
fn serve(mut stream: TcpStream, answers: &Answers, limits: Limits) {
    let end = Instant::now() + limits.connection;
    let _ = stream.set_nonblocking(false);
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    let fits = loop {
        if let Some(at) = head.windows(4).position(|window| window == b"\r\n\r\n") {
            head.truncate(at + 4);
            break head.len() <= limits.head;
        }
        if head.len() >= limits.head {
            break false;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(count) => head.extend_from_slice(&chunk[..count]),
        }
    };
    let answer = if fits { reply(&head, answers) } else { status(400, "Bad Request") };
    let left = end.saturating_duration_since(Instant::now());
    if left.is_zero() || stream.set_write_timeout(Some(left)).is_err() {
        return;
    }
    if stream.write_all(&answer).and_then(|()| stream.flush()).is_err() {
        return;
    }
    let _ = stream.shutdown(Shutdown::Write);
    let mut dropped = 0;
    while dropped < LINGER_MAX {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => dropped += count,
        }
    }
}

/// Serves `listeners` until `stop` can be read to its end (or fails): each
/// connection in a thread of its own, up to `limits.connections`; one
/// beyond is closed at once.
///
/// # Errors
///
/// If waiting for a connection fails.
pub fn run(
    listeners: &[TcpListener],
    stop: RawFd,
    answers: &Arc<Answers>,
    limits: Limits,
) -> io::Result<()> {
    for listener in listeners {
        listener.set_nonblocking(true)?;
    }
    let open = Arc::new(AtomicUsize::new(0));
    loop {
        let mut fds = vec![libc::pollfd { fd: stop, events: libc::POLLIN, revents: 0 }];
        fds.extend(listeners.iter().map(|listener| libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }));
        // SAFETY: poll reads and writes the entries of `fds`, which are as many as it is told.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[0].revents != 0 {
            let mut sink = [0u8; 256];
            // SAFETY: read writes at most the buffer's length.
            let count = unsafe { libc::read(stop, sink.as_mut_ptr().cast(), sink.len()) };
            if count == 0 {
                return Ok(());
            }
            if count < 0 {
                let error = io::Error::last_os_error();
                if !matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) {
                    return Ok(());
                }
            }
        }
        for listener in listeners {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => accept(stream, answers, &open, limits),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        eprintln!("acme-answer: accept: {error}");
                        break;
                    }
                }
            }
        }
    }
}

/// Starts a thread for `stream`, or closes it when [`Limits::connections`]
/// are open already.
fn accept(stream: TcpStream, answers: &Arc<Answers>, open: &Arc<AtomicUsize>, limits: Limits) {
    if open.fetch_add(1, Ordering::SeqCst) >= limits.connections {
        open.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    let (answers, counter) = (Arc::clone(answers), Arc::clone(open));
    let spawned =
        thread::Builder::new().name("answer".into()).stack_size(128 * 1024).spawn(move || {
            serve(stream, &answers, limits);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Reads the answers from `input`: a line `TOKEN KEYAUTH` each, up to an
/// empty line.
///
/// # Errors
///
/// With what is wrong with the first line that is.
pub fn read_answers(input: &mut dyn BufRead) -> Result<Answers, String> {
    let mut answers = Answers::new();
    loop {
        let mut line = String::new();
        let count = (&mut *input)
            .take(KEYAUTH_MAX as u64 + TOKEN_MAX as u64 + 3)
            .read_line(&mut line)
            .map_err(|error| format!("reading the answers: {error}"))?;
        if count == 0 {
            return Err(String::from("the answers end without an empty line"));
        }
        let Some(line) = line.strip_suffix('\n') else {
            return Err(String::from("an answer's line is too long"));
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return Ok(answers);
        }
        let Some((token, keyauth)) = line.split_once(' ') else {
            return Err(String::from("an answer is a token and its key authorization"));
        };
        if !is_token(token) {
            return Err(format!("`{token}` is not a token"));
        }
        if keyauth.is_empty()
            || keyauth.len() > KEYAUTH_MAX
            || !keyauth.bytes().all(|byte| (0x21..0x7f).contains(&byte))
        {
            return Err(format!(
                "the key authorization of {token} is not printable text of up to {KEYAUTH_MAX} characters"
            ));
        }
        if answers.len() >= ANSWERS_MAX {
            return Err(format!("more than {ANSWERS_MAX} answers"));
        }
        answers.insert(token.to_string(), keyauth.to_string());
    }
}

/// `acme-answer FD...`: serves the listening sockets that descriptors
/// `FD...` are, with the answers read from standard input.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut descriptors = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) => {
                let fd = operand
                    .to_str()
                    .and_then(|text| text.parse::<RawFd>().ok())
                    .filter(|fd| *fd > 2)
                    .ok_or_else(|| {
                        usage("acme-answer takes the numbers of its listening sockets".into())
                    })?;
                descriptors.push(fd);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    if descriptors.is_empty() {
        return Err(usage("usage: acme-answer FD...".into()));
    }
    let answers = read_answers(&mut io::stdin().lock()).map_err(fail)?;
    let listeners: Vec<TcpListener> = descriptors
        .into_iter()
        // SAFETY: the caller made these descriptors its listening sockets and gave them to this
        // process alone, so each is open and owned here.
        .map(|fd| unsafe { TcpListener::from_raw_fd(fd) })
        .collect();
    run(&listeners, 0, &Arc::new(answers), LIMITS)
        .map_err(|error| fail(format!("acme-answer: {error}")))?;
    Ok(0)
}

/// The program the responder is, which its view holds alone.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
const PROGRAM: &str = "/system/commands/systools";

/// The responder, running: it serves until this is stopped or dropped.
pub struct Responder {
    number: u64,
    input: Option<File>,
}

impl Responder {
    /// Starts the responder with `listeners` and `answers`, each a token
    /// and its key authorization.
    ///
    /// # Errors
    ///
    /// With what could not be done, in words.
    pub fn start(
        listeners: &[&TcpListener],
        answers: &[(String, String)],
    ) -> Result<Responder, String> {
        let (reader, writer) =
            io::pipe().map_err(|error| format!("cannot make a pipe: {error}"))?;
        let descriptors: Vec<RawFd> =
            listeners.iter().map(|listener| listener.as_raw_fd()).collect();
        let number = launch(&descriptors, reader.as_raw_fd())?;
        drop(reader);
        let mut text = String::new();
        for (token, keyauth) in answers {
            text.push_str(&format!("{token} {keyauth}\n"));
        }
        text.push('\n');
        // Held from here on, so that a failure below ends it.
        let mut responder =
            Responder { number, input: Some(File::from(std::os::fd::OwnedFd::from(writer))) };
        if let Some(input) = responder.input.as_mut() {
            input
                .write_all(text.as_bytes())
                .map_err(|error| format!("cannot give the answers to the responder: {error}"))?;
        }
        Ok(responder)
    }

    /// Ends the responder: closes its input, which it serves until, and
    /// waits for it for a few seconds, then stops it.
    pub fn stop(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if self.input.take().is_none() {
            return;
        }
        let Ok(pid) = i32::try_from(self.number) else { return };
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let mut status = 0;
            // SAFETY: waitpid writes the status.
            let done = unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) };
            if done != 0 {
                return;
            }
            if Instant::now() >= until {
                // SAFETY: kill touches no memory.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                // SAFETY: as above.
                unsafe { libc::waitpid(pid, &raw mut status, 0) };
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(not(target_os = "radix"))]
fn launch(_listeners: &[RawFd], _input: RawFd) -> Result<u64, String> {
    Err(String::from("the responder runs on Dukovo"))
}

/// Makes the responder's view, which holds its program and nothing else,
/// and starts it there as `acme-answer`, with `listeners` as its descriptors
/// from 3 and `input` as its standard input; its number.
#[cfg(target_os = "radix")]
fn launch(listeners: &[RawFd], input: RawFd) -> Result<u64, String> {
    use radix_abi::capability::{self, ObjectKind, Rights};
    use radix_abi::directory::{self, OpenFlags};
    use radix_abi::{Status, process, startup};

    use crate::signin::{self, Spares};
    use crate::sys::{self, SYS};

    // As bundle(1) gives a program its view: to read and change, and to
    // copy and pass on; never the bypass right, and never control.
    const GIVEN: Rights = Rights(
        Rights::READ.bits() | Rights::WRITE.bits() | Rights::COPY.bits() | Rights::TRANSFER.bits(),
    );
    const READ_ONLY: Rights =
        Rights(Rights::READ.bits() | Rights::COPY.bits() | Rights::TRANSFER.bits());

    let failed = |step: &'static str| {
        move |status: Status| {
            format!("cannot start the responder: {step}: {}", sys::status_name(status.0))
        }
    };
    let spares =
        Spares::new(5).map_err(|error| format!("cannot start the responder: no slots: {error}"))?;
    let [base, view, program, given, child] = [0, 1, 2, 3, 4].map(|index| spares.slot(index));
    let rights = capability::info(&SYS, startup::SLOT_ROOT).map_err(failed("the root"))?.rights;
    capability::derive(&SYS, startup::SLOT_ROOT, base, Rights(rights.bits() & GIVEN.bits()), 0)
        .map_err(failed("narrowing the root"))?;
    directory::make_view(&SYS, base, view).map_err(failed("making its view"))?;
    let at = PROGRAM.trim_start_matches('/').as_bytes();
    directory::open(&SYS, base, at, OpenFlags::NO_FOLLOW, READ_ONLY, program, 0)
        .map_err(failed("opening its program"))?;
    directory::attach(&SYS, view, at, program).map_err(failed("attaching its program"))?;
    // Without control: nothing more is attached to it.
    capability::derive(&SYS, view, given, GIVEN, 0).map_err(failed("narrowing its view"))?;
    process::create(&SYS, startup::SLOT_PROCESS, ObjectKind::Process, signin::SLOTS, child)
        .map_err(failed("making its process"))?;
    // Rust's runtime opens /dev/null for a standard descriptor that is
    // closed, and aborts if it cannot, as it cannot in the responder's
    // view: so what it has no use for is the null device.
    let null_device = File::options()
        .write(true)
        .open("/devices/null")
        .map_err(|error| format!("cannot start the responder: /devices/null: {error}"))?;
    let null = sys::slot_of(null_device.as_raw_fd());
    let mut shares = vec![
        (given, startup::SLOT_ROOT),
        (startup::SLOT_RANDOM, startup::SLOT_RANDOM),
        (startup::SLOT_LOG, startup::SLOT_LOG),
        (sys::slot_of(input), startup::SLOT_FD_BASE),
        (null, startup::SLOT_FD_BASE + 1),
    ];
    // Its errors go where the caller's go, unless that is a terminal: a
    // program that answers strangers never holds a person's terminal,
    // which it could read from. It writes nothing else.
    let errors = sys::slot_of(2);
    if !crate::util::is_terminal(2) && capability::info(&SYS, errors).is_ok() {
        shares.push((errors, startup::SLOT_FD_BASE + 2));
    } else {
        shares.push((null, startup::SLOT_FD_BASE + 2));
    }
    for (number, fd) in listeners.iter().enumerate() {
        shares.push((sys::slot_of(*fd), startup::SLOT_FD_BASE + 3 + number as u32));
    }
    for (from, to) in shares {
        process::share(&SYS, child, from, to, Rights::ALL, 0)
            .map_err(failed("giving it its capabilities"))?;
    }
    let numbers: Vec<String> =
        (0..listeners.len()).map(|number| (3 + number).to_string()).collect();
    let mut args: Vec<&[u8]> = vec![b"acme-answer"];
    args.extend(numbers.iter().map(String::as_bytes));
    let block = signin::exec_block_in(b"/", PROGRAM.as_bytes(), &args, &[]);
    process::exec(&SYS, child, program, &block).map_err(failed("starting it"))?;
    let number = process::info(&SYS, child).map_err(failed("its number"))?.number;
    Ok(number)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::net::SocketAddr;

    use super::*;

    fn answers() -> Answers {
        let mut answers = Answers::new();
        answers.insert("tok-1_A".into(), "tok-1_A.thumb".into());
        answers.insert("other".into(), "other.thumb".into());
        answers
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn a_token_is_one_to_128_characters_of_base64url() {
        assert!(is_token("a"));
        assert!(is_token("Az09-_"));
        assert!(is_token(&"x".repeat(128)));
        assert!(!is_token(""));
        assert!(!is_token(&"x".repeat(129)));
        assert!(!is_token("a/b"));
        assert!(!is_token("a.b"));
        assert!(!is_token("a b"));
        assert!(!is_token("a=b"));
        assert!(!is_token("\u{e9}"));
    }

    #[test]
    fn a_get_of_a_token_it_holds_is_answered_with_the_key_authorization() {
        let head = b"GET /.well-known/acme-challenge/tok-1_A HTTP/1.1\r\nHost: a.example\r\n\
                     User-Agent: Mozilla\r\n\r\n";
        assert_eq!(
            text(&reply(head, &answers())),
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 13\r\n\
             Connection: close\r\n\r\ntok-1_A.thumb"
        );
    }

    #[test]
    fn a_head_has_the_same_head_and_no_body() {
        let get =
            text(&reply(b"GET /.well-known/acme-challenge/other HTTP/1.0\r\n\r\n", &answers()));
        let head =
            text(&reply(b"HEAD /.well-known/acme-challenge/other HTTP/1.1\r\n\r\n", &answers()));
        assert!(get.ends_with("\r\n\r\nother.thumb"));
        assert_eq!(head, get.strip_suffix("other.thumb").unwrap());
        assert!(head.contains("Content-Length: 11\r\n"));
    }

    #[test]
    fn anything_else_that_is_a_request_is_not_found() {
        let not_found = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        for head in [
            "GET /.well-known/acme-challenge/unknown HTTP/1.1\r\n\r\n",
            "GET /.well-known/acme-challenge/ HTTP/1.1\r\n\r\n",
            "GET /.well-known/acme-challenge/tok-1_A/ HTTP/1.1\r\n\r\n",
            "GET /.well-known/acme-challenge/tok-1_A?x=1 HTTP/1.1\r\n\r\n",
            "GET /.well-known/acme-challenge/%74ok-1_A HTTP/1.1\r\n\r\n",
            "GET /.well-known/acme-challenge/../acme-challenge/other HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n",
            "GET /index.html HTTP/1.1\r\n\r\n",
            "POST /.well-known/acme-challenge/tok-1_A HTTP/1.1\r\n\r\n",
            "PUT /.well-known/acme-challenge/tok-1_A HTTP/1.1\r\n\r\n",
            "DELETE / HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(text(&reply(head.as_bytes(), &answers())), not_found, "{head}");
        }
    }

    #[test]
    fn what_is_no_request_is_a_bad_request() {
        let bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        for head in [
            "",
            "\r\n\r\n",
            "GET\r\n\r\n",
            "GET /\r\n\r\n",
            "GET / HTTP/2\r\n\r\n",
            "GET / HTTP/1.1 extra\r\n\r\n",
            "get / HTTP/1.1\r\n\r\n",
            "GET  / HTTP/1.1\r\n\r\n",
            "GET http://a.example/ HTTP/1.1\r\n\r\n",
            "GET * HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\nHost: a\r\n\r\n",
            "GET / HTTP/1.1\r\nno colon here\r\n\r\n",
            "GET / HTTP/1.1\r\n folded: header\r\n\r\n",
            "GET / HTTP/1.1\r\nBad Name: x\r\n\r\n",
            "GET / HTTP/1.1\r\n: nameless\r\n\r\n",
            "GET /\u{0}x HTTP/1.1\r\n\r\n",
            "GET /\u{e9} HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: a\rb\r\n\r\n",
        ] {
            assert_eq!(text(&reply(head.as_bytes(), &answers())), bad, "{head:?}");
        }
        assert_eq!(text(&reply(&[0xff, 0xfe], &answers())), bad);
    }

    #[test]
    fn the_answers_are_read_a_line_each_up_to_an_empty_line() {
        let mut input: &[u8] = b"tok-1_A tok-1_A.thumb\nother other.thumb\n\nleft over\n";
        assert_eq!(read_answers(&mut input).unwrap(), answers());
        assert_eq!(read_answers(&mut &b"\n"[..]).unwrap(), Answers::new());
        assert_eq!(read_answers(&mut &b"a b\r\n\r\n"[..]).unwrap().len(), 1);
        let wrong = |text: &str| read_answers(&mut text.as_bytes()).unwrap_err();
        assert_eq!(wrong(""), "the answers end without an empty line");
        assert_eq!(wrong("a b\n"), "the answers end without an empty line");
        assert_eq!(wrong("a\n\n"), "an answer is a token and its key authorization");
        assert_eq!(wrong("a/b c\n\n"), "`a/b` is not a token");
        assert!(wrong("a b c\n\n").starts_with("the key authorization of a is not printable"));
        assert!(
            wrong(&format!("a {}\n\n", "k".repeat(513))).starts_with("the key authorization of a")
        );
        assert_eq!(wrong(&format!("a {}\n\n", "k".repeat(700))), "an answer's line is too long");
        let mut many = String::new();
        for number in 0..101 {
            writeln!(many, "t{number} k").unwrap();
        }
        assert_eq!(wrong(&many), "more than 100 answers");
    }

    /// A server on a port of this machine, serving until the returned
    /// descriptor's other end is closed.
    struct Running {
        address: SocketAddr,
        stop: Option<File>,
        server: Option<thread::JoinHandle<io::Result<()>>>,
    }

    impl Running {
        fn start(limits: Limits) -> Running {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let mut fds = [0; 2];
            // SAFETY: pipe writes the two descriptors.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            // SAFETY: both are open and owned here.
            let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
            let answers = Arc::new(answers());
            let server = thread::spawn(move || {
                let result = run(&[listener], read.as_raw_fd(), &answers, limits);
                drop(read);
                result
            });
            Running { address, stop: Some(write), server: Some(server) }
        }

        fn ask(&self, request: &[u8]) -> String {
            let mut stream = TcpStream::connect(self.address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            let _ = stream.write_all(request);
            let mut answer = Vec::new();
            let _ = stream.read_to_end(&mut answer);
            String::from_utf8_lossy(&answer).into_owned()
        }

        fn end(mut self) -> io::Result<()> {
            self.stop.take();
            self.server.take().unwrap().join().unwrap()
        }
    }

    #[test]
    fn a_connection_is_answered_once_and_closed() {
        let server = Running::start(LIMITS);
        let answer =
            server.ask(b"GET /.well-known/acme-challenge/other HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
        assert!(answer.ends_with("other.thumb"));
        let unknown = server.ask(b"GET /nothing HTTP/1.1\r\n\r\n");
        assert!(unknown.starts_with("HTTP/1.1 404 Not Found\r\n"), "{unknown}");
        let bad = server.ask(b"hello\r\n\r\n");
        assert!(bad.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{bad}");
        // A second request on the connection is not read.
        let twice = server.ask(
            b"GET /.well-known/acme-challenge/other HTTP/1.1\r\n\r\nGET /.well-known/acme-challenge/tok-1_A HTTP/1.1\r\n\r\n",
        );
        assert_eq!(twice.matches("HTTP/1.1").count(), 1, "{twice}");
        server.end().unwrap();
    }

    #[test]
    fn a_head_longer_than_the_limit_is_a_bad_request() {
        let server = Running::start(LIMITS);
        let long = format!(
            "GET /.well-known/acme-challenge/other HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(8 * 1024)
        );
        let answer = server.ask(long.as_bytes());
        assert!(answer.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{answer}");
        // A head that fills the limit exactly, with its end, is read.
        let prefix = "GET /.well-known/acme-challenge/other HTTP/1.1\r\nX-Pad: ";
        let pad = LIMITS.head - prefix.len() - 4;
        let exact = format!("{prefix}{}\r\n\r\n", "a".repeat(pad));
        assert_eq!(exact.len(), LIMITS.head);
        assert!(server.ask(exact.as_bytes()).starts_with("HTTP/1.1 200 OK\r\n"));
        server.end().unwrap();
    }

    #[test]
    fn a_slow_client_is_cut_off_when_its_time_is_up() {
        let limits = Limits { connection: Duration::from_millis(300), ..LIMITS };
        let server = Running::start(limits);
        let started = Instant::now();
        let mut stream = TcpStream::connect(server.address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        stream.write_all(b"GET /.well-known/acme-challenge/other HT").unwrap();
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        assert!(answer.is_empty(), "{}", text(&answer));
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert!(started.elapsed() < Duration::from_secs(10));
        // It still serves.
        assert!(server.ask(b"GET /x HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        server.end().unwrap();
    }

    #[test]
    fn connections_beyond_the_limit_are_closed_at_once() {
        let limits = Limits { connections: 2, connection: Duration::from_secs(5), ..LIMITS };
        let server = Running::start(limits);
        let held: Vec<TcpStream> =
            (0..2).map(|_| TcpStream::connect(server.address).unwrap()).collect();
        thread::sleep(Duration::from_millis(300));
        let beyond = server.ask(b"GET /x HTTP/1.1\r\n\r\n");
        assert!(beyond.is_empty(), "{beyond}");
        drop(held);
        thread::sleep(Duration::from_millis(300));
        let mut after = String::new();
        for _ in 0..20 {
            after = server.ask(b"GET /x HTTP/1.1\r\n\r\n");
            if !after.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(after.starts_with("HTTP/1.1 404"), "{after}");
        server.end().unwrap();
    }

    #[test]
    fn it_ends_when_its_input_closes() {
        let server = Running::start(LIMITS);
        let started = Instant::now();
        server.end().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_responder_does_not_start_where_there_is_no_dukovo() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let started = Responder::start(&[&listener], &[(String::from("t"), String::from("t.k"))]);
        assert_eq!(started.err().unwrap(), "the responder runs on Dukovo");
    }
}
