// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! reportd's socket (ADR 0531): a connection sends one request, a line
//! of JSON, and reads the answer ([`crate::reports::protocol`]), after
//! which reportd closes it.
//!
//! Any user may ask: the kernel says who they are (`socket_state`, which
//! is POSIX's SO_PEERCRED), never the request, and each sees only their
//! own crashes and reports, an administrator all. Each connection has a
//! thread of its own, at most [`super::CONNECTIONS_MAX`] at a time; a
//! request has 5 seconds to arrive. A `send` or a `retry` waits for the
//! server, up to half a minute for each report.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde::Serialize;

use super::parts::Asker;
use super::{CONNECTIONS_MAX, Reportd};
use crate::deadline::Deadline;
use crate::reports::protocol::{Done, End, REQUEST_MAX, Refusal, Request, SOCKET};
use crate::sys;
use crate::util::{self, Fail};

/// How long a request has to arrive.
const REQUEST_TIME: Duration = Duration::from_secs(5);
/// How long the answer has to be read, which a send makes wait for the
/// server.
const ANSWER_TIME: Duration = Duration::from_secs(120);

const BUSY: &str = "reportd is busy with other requests; try again in a moment";

/// The socket, open to every program: reportd checks who asks.
///
/// # Errors
///
/// If it cannot be made.
pub fn listen() -> Result<UnixListener, Fail> {
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(SOCKET).parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(dir).map_err(|err| util::failed(&dir.display(), err))?;
    let _ = std::fs::remove_file(SOCKET);
    let listener = UnixListener::bind(SOCKET).map_err(|err| util::failed(&SOCKET, err))?;
    let open = |path: &Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|err| util::failed(&path.display(), err))
    };
    open(dir, 0o755)?;
    open(Path::new(SOCKET), 0o666)?;
    listener.set_nonblocking(true).map_err(|err| util::failed(&SOCKET, err))?;
    Ok(listener)
}

/// Accepts connections until `stop`.
///
/// # Errors
///
/// If waiting for a connection fails.
pub fn serve(reportd: &Arc<Reportd>, listener: &UnixListener, stop: &AtomicBool) -> io::Result<()> {
    while !stop.load(Ordering::Relaxed) {
        let mut fds = [libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: `fds` holds one valid pollfd.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, 500) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        while let Ok((stream, _)) = listener.accept() {
            admit(reportd, stream);
        }
    }
    Ok(())
}

/// Serves `stream` on a thread of its own, or says reportd is busy.
fn admit(reportd: &Arc<Reportd>, stream: UnixStream) {
    let _ = stream.set_nonblocking(false);
    if reportd.serving.fetch_add(1, Ordering::AcqRel) >= CONNECTIONS_MAX {
        reportd.serving.fetch_sub(1, Ordering::AcqRel);
        return refuse(&stream, BUSY);
    }
    let asker = asker_of(&stream);
    let this = Arc::clone(reportd);
    let own = stream.try_clone();
    let started = thread::Builder::new().name("reportd-request".into()).spawn(move || {
        serve_one(&this, &stream, asker);
        this.serving.fetch_sub(1, Ordering::AcqRel);
    });
    if started.is_err() {
        reportd.serving.fetch_sub(1, Ordering::AcqRel);
        if let Ok(stream) = own {
            refuse(&stream, BUSY);
        }
    }
}

/// Says `why` to a connection that is turned away.
fn refuse(stream: &UnixStream, why: &str) {
    if let Ok(mut out) = Deadline::new(stream, Duration::from_secs(1)) {
        let _ = line(&mut out, &Refusal { error: why.into() });
    }
}

/// Who is at the other end of `stream`, as the kernel says.
fn asker_of(stream: &UnixStream) -> Result<Asker, String> {
    let slot = sys::slot_of(stream.as_raw_fd());
    let state = radix_abi::file::socket_state(&sys::SYS, slot)
        .map_err(|_| "reportd cannot tell who is asking".to_string())?;
    Ok(Asker { user: state.user, admin: is_admin(state.user) })
}

/// Whether `user` is root or an administrator, by the account files.
fn is_admin(user: u32) -> bool {
    if user == 0 {
        return true;
    }
    let (Ok(users), Ok(groups)) =
        (std::fs::read_to_string(accounts::PASSWD), std::fs::read_to_string(accounts::GROUP))
    else {
        return false;
    };
    let users = accounts::users(&users);
    let groups = accounts::groups(&groups);
    users.entries().find(|u| u.uid == user).is_some_and(|u| accounts::is_admin(u, &groups))
}

/// Reads the request on `stream` and answers it.
fn serve_one(reportd: &Reportd, stream: &UnixStream, asker: Result<Asker, String>) {
    let (Ok(request_in), Ok(answer_out)) =
        (Deadline::new(stream, REQUEST_TIME), Deadline::each(stream, ANSWER_TIME))
    else {
        return;
    };
    let mut out = io::BufWriter::new(answer_out);
    let answered = read_request(request_in)
        .and_then(|request| asker.map(|asker| (asker, request)))
        .and_then(|(asker, request)| answer(reportd, asker, &request, &mut out));
    if let Err(why) = answered {
        let _ = line(&mut out, &Refusal { error: why });
    }
    let _ = out.flush();
}

/// The request `stream` sends before its deadline.
fn read_request(stream: Deadline<'_>) -> Result<Request, String> {
    let mut buffered = BufReader::new(stream);
    let mut text = Vec::new();
    let limit = (REQUEST_MAX + 1) as u64;
    match (&mut buffered).take(limit).read_until(b'\n', &mut text) {
        Ok(0) => return Err("the request is empty".into()),
        Ok(_) if text.len() > REQUEST_MAX => {
            return Err(format!("the request is longer than {REQUEST_MAX} bytes"));
        }
        Ok(_) if !text.ends_with(b"\n") => return Err("the request has no end of line".into()),
        Ok(_) => {}
        Err(err) if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
            return Err(format!("no request within {} seconds", REQUEST_TIME.as_secs()));
        }
        Err(err) => return Err(format!("cannot read the request: {err}")),
    }
    serde_json::from_slice(&text).map_err(|err| format!("not a request: {err}"))
}

/// Writes `item` as a line of JSON.
fn line<W: Write + ?Sized>(out: &mut W, item: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *out, item).map_err(io::Error::other)?;
    out.write_all(b"\n")
}

/// Writes `item` as the whole answer.
fn one(out: &mut impl Write, item: &impl Serialize) -> Result<(), String> {
    line(out, item).map_err(|err| format!("cannot answer: {err}"))
}

/// Writes each of `items` as a line, then the end.
fn lines(out: &mut impl Write, items: &[impl Serialize]) -> Result<(), String> {
    items
        .iter()
        .try_for_each(|item| line(out, item))
        .and_then(|()| line(out, &End { end: true }))
        .map_err(|err| format!("cannot answer: {err}"))
}

/// Does what `request` asks, for `asker`, and writes the answer.
fn answer(
    reportd: &Reportd,
    asker: Asker,
    request: &Request,
    out: &mut impl Write,
) -> Result<(), String> {
    match request.ask.as_str() {
        "crashes" => lines(out, &reportd.crashes(asker)),
        "draft" => {
            let about = request.about.as_ref().ok_or("draft needs what it is about")?;
            let answers = request.answers.clone().unwrap_or_default();
            one(out, &reportd.draft(asker, about, &answers, request.without_logs)?)
        }
        "redact" => {
            let text = request.text.as_deref().ok_or("redact needs a text")?;
            one(out, &Reportd::redact(text))
        }
        "send" => {
            let text = request.text.as_deref().ok_or("send needs the report's text")?;
            let title = request.title.as_deref().unwrap_or_default();
            let crash = request.crash.as_deref();
            one(out, &reportd.send(asker, text, title, request.security, crash)?)
        }
        "queue" => lines(out, &reportd.waiting(asker)),
        "retry" => lines(out, &reportd.retry(asker)),
        "status" => lines(out, &reportd.status(asker, request.number)?),
        "delete" => {
            let number = request.number.ok_or("delete needs a report's number")?;
            reportd.delete(asker, number)?;
            one(out, &Done { ok: true })
        }
        "forget" => {
            let id = request.crash.as_deref().ok_or("forget needs a crash's ID")?;
            reportd.forget(asker, id)?;
            one(out, &Done { ok: true })
        }
        other => Err(format!("reportd does not know '{other}'")),
    }
}
