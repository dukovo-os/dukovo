// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! admind's server (ADR 0514): what listens, whom it lets in, and what it
//! keeps up while it runs.
//!
//! A connection is looked at before anything is said to it. It counts
//! against the 512 connections at once and the 32 from one address; then
//! the address must be one admind answers (`reach`), and if it is not the
//! connection is closed without a byte and the log says so once a minute
//! for the address, never once per try. What passes gets its TLS handshake
//! and then its first request's head within 20 seconds, so that a client
//! that says nothing holds a place for no longer than that; hyper reads
//! the heads (16 KiB and 64 headers at most) and closes a connection that
//! sends none for 60 seconds, which is the keep-alive.
//!
//! A browser that offers HTTP/2 gets it (ADR 0519): every tab's requests
//! and event streams then share one connection, 64 streams at most, where
//! HTTP/1.1 gives a browser six connections to a site and a seventh tab's
//! stream waits for one. A stream's head is held to the same 16 KiB; the
//! first request's time is the same; a connection with no stream open for
//! 60 seconds is told to go away, which is HTTP/2's way to close. The
//! Terminal's WebSocket stays on HTTP/1.1: a browser opens a connection of
//! its own for it, offering HTTP/1.1 alone, since admind takes no
//! WebSocket over HTTP/2.
//!
//! One thread does all of it, on tokio's current-thread runtime, so that
//! the sessions and the counts are plain data (see [`Shared`]). Every 30
//! seconds the server ends the sessions whose time is up, reads the sign-in
//! rules and `reach` again, and looks at the machine's name and networks.
//! SIGHUP, which Spark sends when the certificate changed, reads the
//! certificate again for the connections that come after it; one that does
//! not read leaves the old in use, and the log says why. SIGTERM ends the
//! sessions, which tells authd, and stops.

use std::cell::Cell;
use std::fs;
use std::future::{Future, poll_fn};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use sysapi::signal::{self, Caught};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;

use super::files::{self, Files};
use super::limits::{Held, Slot};
use super::log;
use super::mcp;
use super::reach;
use super::routes::{self, Connection};
use super::shared::{Identity, Shared, unix_now};
use super::store::{self, Store};
use super::tasks;
use super::tls::{self, Certificate};
use super::trials;
use crate::admin_settings::{Reach, Settings};
use crate::logd;
use crate::util::{self, Fail, fail};

/// How long a connection has for its TLS handshake and the head of its
/// first request together. Browsers open connections ahead of need and
/// keep one they have not used for a while (Chromium for 10 seconds);
/// this is longer, so that the browser closes such a connection first and
/// never sends a request on one admind is closing, which it would retry
/// on another and, a few times over, give up as a failed page.
const HEAD_TIME: Duration = Duration::from_secs(20);

/// How long a connection may wait for the head of its next request, and
/// how long a quiet one waits before the network's first probe.
const KEEP_ALIVE: Duration = Duration::from_secs(60);

/// How long apart the network's probes of a quiet connection go when
/// none is answered: a browser that went away without a word is found
/// out by them, which also frees the place of an event stream no one
/// reads.
const PROBE_EVERY: Duration = Duration::from_secs(15);

/// The most bytes a request's head may have, and the most headers.
/// A head over either is answered 431, with no body (`openapi::head_too_large`).
pub(super) const HEAD_MAX: usize = 16 << 10;
pub(super) const HEADERS_MAX: usize = 64;

/// The most streams one HTTP/2 connection has open at once: a browser's
/// tabs share it, each with its event stream and a few requests.
const STREAMS_MAX: u32 = 64;

/// How often the server looks after itself.
const UPKEEP_EVERY: Duration = Duration::from_secs(30);

/// How often it looks at the signals that came.
const SIGNALS_EVERY: Duration = Duration::from_millis(500);

/// The file consoled shows at its sign-in prompt: where to open the console.
/// `admin fingerprint` reads it too.
pub(crate) const ISSUE: &str = "/system/live/admind/issue";

/// Set by SIGTERM: admind stops. Set by SIGHUP: it reads its certificate
/// again.
static STOP: Caught = Caught::new();
static RELOAD: Caught = Caught::new();

/// Has the signals admind answers set the flags above, and a browser that
/// goes away not end it: the write to it fails instead.
fn install_signals() {
    let _ = signal::ignore(libc::SIGPIPE);
    let _ = signal::catch(libc::SIGTERM, &STOP, 0);
    let _ = signal::catch(libc::SIGHUP, &RELOAD, 0);
}

/// The certificate that `admin.toml` names.
///
/// # Errors
///
/// In plain words, if Spark gave admind no folder for it or it does not read.
fn certificate_named(name: &str) -> Result<Certificate, String> {
    tls::load(&tls::folder(name)?)
}

/// The settings to use once `new` has been read while `old` ran: the
/// sign-in rules and `reach` take effect as they are, and the port never
/// does, since a port is a restart's. The certificate's name does only when
/// `certificate_read`, which says the new one was read.
fn merged(old: &Settings, new: Settings, certificate_read: bool) -> Settings {
    let mut settings = new;
    settings.console.listen = old.console.listen;
    if !certificate_read {
        settings.console.certificate.clone_from(&old.console.certificate);
    }
    settings
}

/// The address the issue file gives: the first name of the certificate
/// that is a name with dots, such as `duke-nas.local` or an authority's, and
/// otherwise the machine's.
fn address_for(host: &str, certificate: &[String], port: u16) -> String {
    let named =
        certificate.iter().find(|name| name.contains('.') && name.parse::<IpAddr>().is_err());
    let name = named.map_or(host, String::as_str);
    if port == 443 { format!("https://{name}") } else { format!("https://{name}:{port}") }
}

/// What the issue file holds: the address, and the certificate's
/// fingerprint to compare with the browser's warning.
fn issue_text(address: &str, fingerprint: &str) -> String {
    format!("{address}\nSHA-256 fingerprint {fingerprint}\n")
}

/// Writes `text` to the issue file, so that a reader never sees half.
fn write_issue(text: &str) -> io::Result<()> {
    let place = Path::new(ISSUE);
    if let Some(folder) = place.parent() {
        fs::create_dir_all(folder)?;
    }
    let temporary = format!("{ISSUE}.new");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, place)
}

/// What the server keeps up with, and has said already.
#[derive(Default)]
struct Upkeep {
    /// Whether a round of the journal is under way: it waits for logd, and
    /// the next is not started until it has ended.
    round: Rc<Cell<bool>>,
    /// The issue file as last written.
    issue: String,
    /// The last words the settings gave when they did not read, so that
    /// the log says them once and not every 30 seconds.
    complained: Option<String>,
    /// The Unix time old tasks were last forgotten, which is done once a
    /// day.
    forgot: i64,
}

impl Upkeep {
    /// Writes the issue file if what it should hold has changed.
    fn issue(&mut self, shared: &Shared) {
        let certificate = shared.certificate.borrow();
        let identity = shared.identity.borrow();
        let address = address_for(&identity.host, &certificate.names, shared.port);
        let text = issue_text(&address, &certificate.fingerprint);
        if text == self.issue {
            return;
        }
        match write_issue(&text) {
            Ok(()) => self.issue = text,
            Err(err) => log(4, &format!("cannot write {ISSUE}: {}", util::strip_os_error(&err))),
        }
    }

    /// What is done every 30 seconds.
    fn tick(&mut self, shared: &Shared) {
        match Settings::read() {
            Ok(new) => {
                self.complained = None;
                let old = shared.settings.borrow().clone();
                if new.console.listen != old.console.listen {
                    log(4, "a new port in admin.toml takes a restart of admind");
                }
                *shared.settings.borrow_mut() = merged(&old, new, false);
            }
            Err(why) => {
                if self.complained.as_ref() != Some(&why) {
                    log(4, &format!("{why}: admind goes on with the settings it has"));
                    self.complained = Some(why);
                }
            }
        }
        shared.read_identity();
        shared.maintain(std::time::Instant::now());
        shared.tasks.prune(tokio::time::Instant::now());
        if let Some(store) = &shared.store
            && unix_now() - self.forgot >= 24 * 3600
        {
            self.forgot = unix_now();
            tasks::forget_old(store);
        }
        self.issue(shared);
    }

    /// Starts the journal's round, which reads Spark's trials, then looks
    /// for files changed by hand and, if there are any, asks logd who
    /// changed them. It runs as a task, so that Spark and logd are not
    /// waited for here.
    fn hand_edits(&self, shared: &Rc<Shared>) {
        if self.round.replace(true) {
            return;
        }
        let (shared, round) = (Rc::clone(shared), Rc::clone(&self.round));
        tokio::task::spawn_local(async move {
            journal_round(&shared).await;
            round.set(false);
        });
    }

    /// What SIGHUP does: reads the settings and the certificate again, and
    /// keeps what was in use for any that does not read.
    fn reload(&mut self, shared: &Shared) {
        let old = shared.settings.borrow().clone();
        let read = Settings::read().unwrap_or_else(|why| {
            log(4, &format!("{why}: admind goes on with the settings it has"));
            old.clone()
        });
        match certificate_named(&read.console.certificate) {
            Ok(certificate) => {
                log(6, &format!("the certificate {} is read again", read.console.certificate));
                *shared.settings.borrow_mut() = merged(&old, read, true);
                *shared.certificate.borrow_mut() = certificate;
                shared.read_identity();
            }
            Err(why) => {
                log(4, &format!("{why}: admind goes on with the certificate it has"));
                *shared.settings.borrow_mut() = merged(&old, read, false);
            }
        }
        // The trial `console` takes effect now, not when its action
        // answered (C5 9.2).
        shared.trials.borrow_mut().reloaded(std::time::Instant::now());
        self.issue(shared);
    }
}

/// One round of the journal: Spark's trials and the undos it made by
/// itself, then the files changed by hand since the last, with who
/// changed them, as rows. Said in the log when it made any, and when the
/// store failed. The first, when admind starts, gives every trial Spark
/// lists admind's start as its beginning, since every connection is newer.
async fn journal_round(shared: &Shared) {
    // Spark's undos first: the files an undo put back are its row's, and
    // not a hand edit.
    look_at_trials(shared).await;
    let Some(store) = &shared.store else { return };
    let now = unix_now();
    let rooted = shared.terminals.borrow().rooted(now);
    match shared.journal.round(store, logd::QUERY, &rooted, now).await {
        Ok(0) => {}
        Ok(rows) => log(6, &format!("{rows} changes made by hand are in the journal")),
        Err(why) => log(4, &format!("the journal could not look at the files: {why}")),
    }
}

/// Reads Spark's trials, for the pages that ask whether their browser
/// reaches the console over a change being tried, tells the open tabs of
/// the trials that run when they are due to be told
/// ([`trials::Trials::round_tells`]), and gives the journal a row for each
/// trial Spark undid by itself (ADR 0518). A change that begins a trial
/// calls it too, since how long the trial has left is Spark's to say.
///
/// The same read says whether a restart or a shut down is set: the tabs
/// are told when that changes (`event: power`, ADR 0565), so a restart set
/// or cancelled at a terminal is told at the next round, and one from the
/// console at once, since each `power` change calls this when it answers.
pub(super) async fn look_at_trials(shared: &Shared) {
    let document = match trials::document(shared.spark.get()).await {
        Ok(document) => document,
        Err(why) => {
            if shared.trials.borrow_mut().complain(Some(&why)) {
                log(4, &format!("{why}: the console cannot tell what is being tried"));
            }
            return;
        }
    };
    if shared.trials.borrow_mut().complain(None) {
        log(6, "Spark answers again: the console reads what is being tried");
    }
    let set = crate::power::pending(&document, crate::util::now_ns() / 1_000_000);
    if shared.power.borrow_mut().read(set) {
        shared.changes.send_replace(());
    }
    let now = unix_now();
    let read_at = Instant::now().into_std();
    let (undos, tells) = {
        let mut trials = shared.trials.borrow_mut();
        trials.read(&document, read_at);
        (trials.undos(&document, now), trials.round_tells(read_at))
    };
    if tells {
        shared.tell_trials(read_at);
    }
    let Some(store) = &shared.store else { return };
    for undo in undos {
        match shared.journal.undone(store, &undo, now) {
            Ok(_) => shared.trials.borrow_mut().noted(&undo),
            Err(why) => log(4, &format!("the journal could not keep an undo of Spark's: {why}")),
        }
    }
}

/// Opens the store. One that does not open is said, and the console goes
/// on without a journal and tasks: signing in and the actions do not
/// depend on it.
fn open_store() -> Option<Store> {
    match Store::open(Path::new(store::PATH)) {
        Ok(store) => {
            // The commands of the tasks an admind left running were hung up
            // with it.
            tasks::restarted(&store);
            Some(store)
        }
        Err(why) => {
            log(3, &format!("{why}: the console keeps no journal and runs no tasks"));
            None
        }
    }
}

/// A socket that listens on `address`, and for IPv6 only on IPv6, since
/// the IPv4 address has its own.
///
/// # Errors
///
/// If the address cannot be listened on.
///
/// # Panics
///
/// Outside a tokio runtime.
fn listener(address: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    TcpListener::from_std(socket.into())
}

/// The sockets to listen on at `port`: IPv6 and IPv4, as many as the
/// machine has.
///
/// # Errors
///
/// In plain words, if neither can be listened on.
fn listen(port: u16) -> Result<Vec<TcpListener>, String> {
    let wanted = [
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
    ];
    let mut listening = Vec::new();
    let mut failed = Vec::new();
    for address in wanted {
        match listener(address) {
            Ok(socket) => listening.push(socket),
            Err(err) => failed.push(format!("{address}: {}", util::strip_os_error(&err))),
        }
    }
    if listening.is_empty() {
        return Err(format!("cannot listen: {}", failed.join("; ")));
    }
    for why in failed {
        log(4, &format!("does not listen on {why}"));
    }
    Ok(listening)
}

/// Polls `work` to its end, unless `seen` is still false once `deadline`
/// comes, which is `None`: the head of a connection's first request has
/// not come in time. After it has, hyper's own timer watches the heads.
async fn head_in_time<F: Future>(
    work: F,
    seen: &Cell<bool>,
    deadline: Instant,
) -> Option<F::Output> {
    let mut work = pin!(work);
    let mut late = pin!(tokio::time::sleep_until(deadline));
    poll_fn(|cx| {
        if let Poll::Ready(done) = work.as_mut().poll(cx) {
            return Poll::Ready(Some(done));
        }
        if !seen.get() && late.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// Sets the network's probes on `stream`: the first after [`KEEP_ALIVE`]
/// of quiet, and then every [`PROBE_EVERY`].
fn set_probes(stream: &TcpStream) -> io::Result<()> {
    let probes = TcpKeepalive::new().with_time(KEEP_ALIVE);
    #[cfg(not(target_os = "radix"))]
    let probes = probes.with_interval(PROBE_EVERY);
    SockRef::from(stream).set_tcp_keepalive(&probes)?;
    #[cfg(target_os = "radix")]
    probe_every(stream)?;
    Ok(())
}

/// Sets the time between probes on Radix, whose socket2 leaves
/// TCP_KEEPINTVL out although the kernel takes it.
#[cfg(target_os = "radix")]
fn probe_every(stream: &TcpStream) -> io::Result<()> {
    let seconds = libc::c_int::try_from(PROBE_EVERY.as_secs()).unwrap_or(libc::c_int::MAX);
    sysapi::net::set_int_option(stream, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, seconds)
}

/// Runs the futures of an HTTP/2 connection's streams on this thread,
/// beside the rest of admind, whose state is not shared between threads.
#[derive(Clone, Copy)]
struct Local;

impl<F: Future + 'static> hyper::rt::Executor<F> for Local {
    fn execute(&self, future: F) {
        tokio::task::spawn_local(future);
    }
}

/// How many of a connection's requests are being answered: from the
/// request's head to the end of its answer's body, or until the browser
/// lets the stream go. An HTTP/2 connection with none for [`KEEP_ALIVE`]
/// is told to go away.
#[derive(Default)]
struct Open {
    count: Cell<usize>,
    /// When the count last fell to none.
    quiet_since: Cell<Option<Instant>>,
    /// Told when the count falls to none.
    quiet: tokio::sync::Notify,
}

/// One request being answered, until it is dropped.
struct Answering(Rc<Open>);

impl Answering {
    fn new(open: &Rc<Open>) -> Answering {
        open.count.set(open.count.get() + 1);
        open.quiet_since.set(None);
        Answering(Rc::clone(open))
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        let open = &self.0;
        open.count.set(open.count.get() - 1);
        if open.count.get() == 0 {
            open.quiet_since.set(Some(Instant::now()));
            open.quiet.notify_one();
        }
    }
}

/// An answer's body that keeps its request counted until it ends.
struct Counted {
    body: routes::Body,
    _answering: Answering,
}

impl http_body::Body for Counted {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        std::pin::Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }
}

/// How long a connection has: for its handshake and first head, and, on
/// HTTP/2, with no request open before it is told to go away.
#[derive(Clone, Copy, Debug)]
struct Times {
    head: Duration,
    quiet: Duration,
}

/// A browser's connection's times.
const TIMES: Times = Times { head: HEAD_TIME, quiet: KEEP_ALIVE };

/// Ends when `open` has had no request for `quiet`: a connection counts
/// as quiet from its start.
async fn quiet_for_long(open: &Open, quiet: Duration) {
    if open.count.get() == 0 && open.quiet_since.get().is_none() {
        open.quiet_since.set(Some(Instant::now()));
    }
    loop {
        match open.quiet_since.get() {
            Some(since) if open.count.get() == 0 => {
                let until = since + quiet;
                if Instant::now() >= until {
                    return;
                }
                let mut late = pin!(tokio::time::sleep_until(until));
                let mut told = pin!(open.quiet.notified());
                poll_fn(|cx| {
                    let woken =
                        late.as_mut().poll(cx).is_ready() || told.as_mut().poll(cx).is_ready();
                    if woken { Poll::Ready(()) } else { Poll::Pending }
                })
                .await;
            }
            _ => open.quiet.notified().await,
        }
    }
}

/// Ends when either `a` or `b` has.
async fn either(a: impl Future<Output = ()>, b: impl Future<Output = ()>) {
    let (mut a, mut b) = (pin!(a), pin!(b));
    poll_fn(|cx| {
        if a.as_mut().poll(cx).is_ready() || b.as_mut().poll(cx).is_ready() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

/// Serves `served`, a connection, until it ends; once `told` ends first,
/// asks it with `end` to end once the answers under way are sent, and
/// waits for that.
async fn served_until<F: Future>(
    served: F,
    end: impl FnOnce(Pin<&mut F>),
    told: impl Future<Output = ()>,
) {
    let mut served = pin!(served);
    let mut told = pin!(told);
    let ended = poll_fn(|cx| {
        if served.as_mut().poll(cx).is_ready() {
            return Poll::Ready(true);
        }
        told.as_mut().poll(cx).map(|()| false)
    })
    .await;
    if !ended {
        end(served.as_mut());
        // A connection that ends badly is a browser that went away.
        let _ = served.await;
    }
}

/// Serves an HTTP/2 connection with `service` until it ends, telling it
/// to go away once it has been quiet for `quiet`, or an answer on it asks
/// that it end (`fresh`): the streams still open end, and no new one
/// begins.
async fn served_h2<S>(
    io: TokioIo<tokio_rustls::server::TlsStream<TcpStream>>,
    service: S,
    open: &Open,
    quiet: Duration,
    connection: &Connection,
) where
    S: hyper::service::HttpService<hyper::body::Incoming, ResBody = Counted>,
    S::Future: 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let mut builder = http2::Builder::new(Local);
    builder
        .timer(TokioTimer::new())
        .max_concurrent_streams(STREAMS_MAX)
        .max_header_list_size(u32::try_from(HEAD_MAX).unwrap_or(u32::MAX));
    let told = either(quiet_for_long(open, quiet), connection.ending.notified());
    served_until(builder.serve_connection(io, service), http2::Connection::graceful_shutdown, told)
        .await;
}

/// Serves one connection, which holds `slot` until it ends, or hands it
/// to the Terminal's socket that it becomes, as `times` says.
async fn connection(shared: Rc<Shared>, stream: TcpStream, peer: IpAddr, times: Times, slot: Slot) {
    let deadline = Instant::now() + times.head;
    let to = stream.local_addr().ok();
    // A socket whose probes cannot be set is still a socket.
    if set_probes(&stream).is_err() {
        log(7, "a connection's keep-alive probes could not be set");
    }
    let acceptor = TlsAcceptor::from(Arc::clone(&shared.certificate.borrow().config));
    let tls = match tokio::time::timeout_at(deadline, acceptor.accept(stream)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(err)) => {
            shared.refused_connection(peer, "its TLS handshake failed", &err.to_string());
            return;
        }
        Err(_) => {
            shared.refused_connection(peer, "its TLS handshake did not finish in time", "");
            return;
        }
    };
    let seen = Rc::new(Cell::new(false));
    if tls.get_ref().1.alpn_protocol() == Some(b"h2") {
        let open = Rc::new(Open::default());
        let connection = Rc::new(Connection::new(to, None));
        let service = {
            let (shared, seen, open) = (Rc::clone(&shared), Rc::clone(&seen), Rc::clone(&open));
            let connection = Rc::clone(&connection);
            service_fn(move |request| {
                seen.set(true);
                let answering = Answering::new(&open);
                let connection = Rc::clone(&connection);
                let answer = routes::handle_on(Rc::clone(&shared), peer, connection, request);
                async move {
                    let Ok(response) = answer.await;
                    Ok::<_, std::convert::Infallible>(
                        response.map(|body| Counted { body, _answering: answering }),
                    )
                }
            })
        };
        let served = served_h2(TokioIo::new(tls), service, &open, times.quiet, &connection);
        if head_in_time(served, &seen, deadline).await.is_none() {
            shared.refused_connection(peer, "it sent no request in time", "");
        }
        // The connection's place is given back as it ends.
        drop(slot);
        return;
    }
    // The Terminal's socket takes the connection's place from here.
    let held: Held = Rc::new(Cell::new(Some(slot)));
    let connection = Rc::new(Connection::new(to, Some(Rc::clone(&held))));
    let service = {
        let (shared, seen, connection) =
            (Rc::clone(&shared), Rc::clone(&seen), Rc::clone(&connection));
        service_fn(move |request| {
            seen.set(true);
            routes::handle_on(Rc::clone(&shared), peer, Rc::clone(&connection), request)
        })
    };
    let served = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(KEEP_ALIVE)
        .max_buf_size(HEAD_MAX)
        .max_headers(HEADERS_MAX)
        .keep_alive(true)
        .serve_connection(TokioIo::new(tls), service)
        // The Terminal's WebSocket takes the connection over once its
        // answer, 101, is sent (`websocket`).
        .with_upgrades();
    // An answer that asks the connection to end (`fresh`) closes it once
    // it is sent.
    let served = served_until(
        served,
        http1::UpgradeableConnection::graceful_shutdown,
        connection.ending.notified(),
    );
    if head_in_time(served, &seen, deadline).await.is_none() {
        shared.refused_connection(peer, "it sent no request in time", "");
    }
    drop(connection);
    drop(held);
}

/// The place a connection from `peer` holds if it is served, or why it is
/// not: there are as many connections as it takes, or the address is not
/// one admind answers.
fn admission(shared: &Shared, peer: IpAddr) -> Result<Slot, &'static str> {
    let slot = Slot::take(&shared.connections, reach::counted(peer))
        .ok_or("there are as many connections as it takes")?;
    let kind: Reach = shared.settings.borrow().console.reach;
    if reach::answers(kind, peer, &shared.identity.borrow().networks) {
        Ok(slot)
    } else {
        Err("it is not on a network admind answers")
    }
}

/// Serves a connection from `peer`, or closes it with no word and notes
/// why.
fn admit(shared: &Rc<Shared>, stream: TcpStream, peer: SocketAddr) {
    let peer = reach::plain(peer.ip());
    match admission(shared, peer) {
        Ok(slot) => {
            tokio::task::spawn_local(connection(Rc::clone(shared), stream, peer, TIMES, slot));
        }
        Err(why) => shared.refused_connection(peer, why, ""),
    }
}

/// Accepts connections on `socket` as long as admind runs.
async fn accepting(shared: Rc<Shared>, socket: TcpListener) {
    loop {
        match socket.accept().await {
            Ok((stream, peer)) => admit(&shared, stream, peer),
            Err(err) => {
                // Out of descriptors, most likely; a moment's wait lets
                // some connections end.
                log(4, &format!("cannot accept a connection: {}", util::strip_os_error(&err)));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// The text of the log line that says admind is up.
fn start_line(address: &str, fingerprint: &str, kind: Reach) -> String {
    let reach = match kind {
        Reach::Private => "private networks",
        Reach::Any => "any network",
    };
    format!(
        "answers at {address} to {reach}; the certificate's SHA-256 fingerprint is {fingerprint}"
    )
}

/// Serves until SIGTERM.
async fn serve(shared: Rc<Shared>) -> Result<(), String> {
    let sockets = listen(shared.port)?;
    let mut upkeep = Upkeep::default();
    // The files as they are now are what later changes are compared with;
    // a file changed while admind was away is a change.
    journal_round(&shared).await;
    upkeep.issue(&shared);
    {
        let (certificate, identity) = (shared.certificate.borrow(), shared.identity.borrow());
        let address = address_for(&identity.host, &certificate.names, shared.port);
        let kind = shared.settings.borrow().console.reach;
        log(6, &start_line(&address, &certificate.fingerprint, kind));
    }
    for socket in sockets {
        tokio::task::spawn_local(accepting(Rc::clone(&shared), socket));
    }
    mcp::start(&shared);
    let mut last = Instant::now();
    while !STOP.is_set() {
        tokio::time::sleep(SIGNALS_EVERY).await;
        if RELOAD.take() {
            upkeep.reload(&shared);
        }
        shared.look_at_terminals();
        if last.elapsed() >= UPKEEP_EVERY {
            last = Instant::now();
            upkeep.tick(&shared);
            upkeep.hand_edits(&shared);
        }
    }
    log(6, &format!("stops: it ends {} sessions", shared.sessions.borrow().len()));
    shared.end_all().await;
    mcp::stop(&shared);
    // A console that is no more has no address to show.
    if let Err(err) = fs::remove_file(ISSUE)
        && err.kind() != io::ErrorKind::NotFound
    {
        log(4, &format!("cannot remove {ISSUE}: {}", util::strip_os_error(&err)));
    }
    Ok(())
}

/// `admind`: reads what it needs, listens, and serves.
///
/// # Errors
///
/// If `admin.toml` or the certificate does not read, or admind cannot
/// listen.
pub fn run() -> Result<i32, Fail> {
    install_signals();
    let settings = Settings::read().map_err(fail)?;
    let certificate = certificate_named(&settings.console.certificate).map_err(fail)?;
    let mut said = Vec::new();
    // A console with no files still answers its API, and says why the
    // pages are missing.
    let files = Files::read(Path::new(files::FOLDER), &mut said).unwrap_or_else(|why| {
        log(3, &format!("the console's files: {why}"));
        Files::default()
    });
    for line in said {
        log(4, &format!("the console's files: {line}"));
    }
    log(6, &format!("serves {} files of the console", files.len()));
    let identity = Identity::read(&certificate.names);
    let shared =
        Shared::new(settings, identity, certificate, files, accounts::SOCKET, open_store());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| util::failed(&"the runtime", err))?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, serve(shared)).map_err(fail)?;
    Ok(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::admin_settings::SecondStep;
    use crate::admind::fixture::{Console, run};
    use crate::admind::limits::CONNECTIONS_MAX;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn the_rules_and_reach_take_effect_and_a_port_waits_for_a_restart() {
        let old =
            Settings::parse("[console]\nlisten = 9090\ncertificate = \"this-machine\"\n").unwrap();
        let new = Settings::parse(
            "[sign-in]\nsecond-step = \"everyone\"\nsession-hours = 2\n\
             [console]\nlisten = 8443\nreach = \"any\"\ncertificate = \"admin.example.org\"\n",
        )
        .unwrap();
        let kept = merged(&old, new.clone(), false);
        assert_eq!(kept.console.listen, 9090);
        assert_eq!(kept.console.reach, Reach::Any);
        assert_eq!(kept.console.certificate, "this-machine");
        assert_eq!(kept.sign_in.second_step, SecondStep::Everyone);
        assert_eq!(kept.sign_in.session_hours, 2);
        let read = merged(&old, new, true);
        assert_eq!(
            (read.console.listen, read.console.certificate.as_str()),
            (9090, "admin.example.org")
        );
    }

    #[test]
    fn the_console_is_opened_at_the_first_name_with_dots_the_certificate_has() {
        let names = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        let this = names(&["duke-nas", "duke-nas.local", "localhost"]);
        assert_eq!(address_for("duke-nas", &this, 9090), "https://duke-nas.local:9090");
        assert_eq!(
            address_for("duke-nas", &names(&["admin.example.org", "10.0.0.5"]), 443),
            "https://admin.example.org"
        );
        // A certificate with no such name leaves the machine's own, and an
        // address is no name.
        assert_eq!(
            address_for("duke-nas", &names(&["10.0.0.5", "localhost"]), 9090),
            "https://duke-nas:9090"
        );
        assert_eq!(address_for("duke-nas", &[], 8443), "https://duke-nas:8443");
        assert_eq!(
            issue_text("https://duke-nas.local:9090", "AB:CD"),
            "https://duke-nas.local:9090\nSHA-256 fingerprint AB:CD\n"
        );
    }

    #[test]
    fn the_start_line_says_where_whom_and_which_certificate() {
        let line = start_line("https://duke-nas.local:9090", "AB:CD", Reach::Private);
        assert_eq!(
            line,
            "answers at https://duke-nas.local:9090 to private networks; \
             the certificate's SHA-256 fingerprint is AB:CD"
        );
        assert!(start_line("x", "y", Reach::Any).contains("to any network"));
    }

    #[test]
    fn a_head_that_has_not_come_by_the_deadline_ends_the_connection_and_one_that_has_does_not() {
        run(async {
            let never = std::future::pending::<()>();
            let soon = Instant::now() + Duration::from_millis(30);
            assert_eq!(head_in_time(never, &Cell::new(false), soon).await, None);
            // Once a request came, the deadline no longer matters.
            let seen = Cell::new(true);
            let later = async {
                tokio::time::sleep(Duration::from_millis(80)).await;
                7
            };
            assert_eq!(head_in_time(later, &seen, Instant::now()).await, Some(7));
            // Work that ends first is its own end.
            let quick = async { 1 };
            assert_eq!(
                head_in_time(quick, &Cell::new(false), Instant::now() + HEAD_TIME).await,
                Some(1)
            );
        });
    }

    #[test]
    fn a_crowd_from_many_addresses_holds_512_places_at_most_and_leaves_the_console_answering() {
        // The crowd and admind's side of it take about 1,200 descriptors,
        // more than some systems let a process have unless it asks.
        enough_descriptors(4096);
        run(async {
            let console = console("serve-crowd");
            let shared = &console.shared;
            let socket = listener(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
            let port = socket.local_addr().unwrap().port();
            tokio::task::spawn_local(accepting(Rc::clone(shared), socket));
            let console_address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

            // 600 connections that say nothing, 30 from each of 20
            // addresses of the loopback network: fewer than one address
            // may hold, more than there are places in all.
            let mut crowd = Vec::new();
            for n in 0..600u16 {
                let from = Ipv4Addr::new(127, 0, 0, 2 + u8::try_from(n / 30).unwrap());
                let socket = tokio::net::TcpSocket::new_v4().unwrap();
                socket.bind(SocketAddr::from((from, 0))).unwrap();
                crowd.push(socket.connect(console_address).await.unwrap());
            }
            let full = Instant::now() + Duration::from_secs(10);
            while shared.connections.borrow().total() < CONNECTIONS_MAX && Instant::now() < full {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            // admind has taken them all by now, and holds no more than its
            // places: the rest it closed at once.
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(shared.connections.borrow().total(), CONNECTIONS_MAX);
            let closed = crowd
                .into_iter()
                .map(|stream| {
                    let mut stream = stream.into_std().unwrap();
                    stream.set_nonblocking(true).unwrap();
                    (matches!(std::io::Read::read(&mut stream, &mut [0u8; 8]), Ok(0)), stream)
                })
                .collect::<Vec<_>>();
            assert_eq!(closed.iter().filter(|(closed, _)| *closed).count(), 600 - CONNECTIONS_MAX);

            // Meanwhile another address, which holds nothing, is closed at
            // once too: there is no place for it.
            let mut late = TcpStream::connect(console_address).await.unwrap();
            let read = tokio::time::timeout(Duration::from_secs(5), late.read(&mut [0u8; 8])).await;
            assert!(matches!(read, Ok(Ok(0) | Err(_))), "{read:?}");

            // Once the crowd goes, every place is given back, and the
            // console answers.
            drop(closed);
            let empty = Instant::now() + Duration::from_secs(10);
            while shared.connections.borrow().total() > 0 && Instant::now() < empty {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(shared.connections.borrow().total(), 0);
            let health =
                "GET /api/1/health HTTP/1.1\r\nHost: nas.example:9090\r\nConnection: close\r\n\r\n";
            assert!(exchange(&console, port, health).await.starts_with("HTTP/1.1 200"));
        });
    }

    /// Raises the limit of open descriptors to `wanted`, or as far as the
    /// hard limit lets.
    fn enough_descriptors(wanted: u64) {
        let files = libc::RLIMIT_NOFILE as libc::c_int;
        let (soft, hard) = sysapi::process::limit(files).unwrap();
        if soft < wanted {
            sysapi::process::set_limit(files, wanted.min(hard), hard).unwrap();
        }
    }

    #[test]
    fn a_port_is_listened_on_by_the_ipv4_address_and_by_the_ipv6_one_apart() {
        run(async {
            let first = listener(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
            let port = first.local_addr().unwrap().port();
            // The same port, again, is refused.
            assert!(listener(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_err());
            // A machine with no IPv6 has nothing to try.
            if let Ok(six) = listener(SocketAddr::from((Ipv6Addr::LOCALHOST, 0))) {
                let six_port = six.local_addr().unwrap().port();
                // The IPv6 socket takes no IPv4 connection, which the
                // IPv4 one does.
                assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, six_port)).await.is_err());
            }
            assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.is_ok());
        });
    }

    /// Serves each connection to a loopback port as the server does,
    /// with `head_time` for its first head, and gives the port.
    pub(crate) fn serve_on_loopback(shared: &Rc<Shared>, head_time: Duration) -> u16 {
        serve_quiet_on_loopback(shared, Times { head: head_time, quiet: KEEP_ALIVE })
    }

    /// As [`serve_on_loopback`], with `times`.
    fn serve_quiet_on_loopback(shared: &Rc<Shared>, times: Times) -> u16 {
        let socket = listener(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let port = socket.local_addr().unwrap().port();
        let shared = Rc::clone(shared);
        tokio::task::spawn_local(async move {
            loop {
                let (stream, peer) = socket.accept().await.unwrap();
                let slot = admission(&shared, peer.ip()).unwrap();
                tokio::task::spawn_local(connection(
                    Rc::clone(&shared),
                    stream,
                    peer.ip(),
                    times,
                    slot,
                ));
            }
        });
        port
    }

    /// A TLS connection to the console on `port`, as a browser that trusts
    /// its authority makes it, offering no protocol: HTTP/1.1.
    pub(crate) async fn connect(
        console: &Console,
        port: u16,
    ) -> tokio_rustls::client::TlsStream<TcpStream> {
        connect_offering(console, port, &[]).await
    }

    /// As [`connect`], offering the protocols `offer` by ALPN.
    async fn connect_offering(
        console: &Console,
        port: u16,
        offer: &[&[u8]],
    ) -> tokio_rustls::client::TlsStream<TcpStream> {
        let mut config = rustls::ClientConfig::builder_with_provider(crate::tls::provider())
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_root_certificates(console.roots.clone())
            .with_no_client_auth();
        config.alpn_protocols = offer.iter().map(|protocol| protocol.to_vec()).collect();
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("nas.example").unwrap();
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, stream).await.unwrap()
    }

    /// What the console answers to `request`, written to a connection
    /// that it then reads to its end.
    async fn exchange(console: &Console, port: u16, request: &str) -> String {
        let mut stream = connect(console, port).await;
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        let read =
            tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer)).await;
        // A connection that was closed without the alert, as a server that
        // hangs up on a request it refuses does, is an end too.
        let _ = read.expect("an answer in 5 seconds");
        String::from_utf8_lossy(&answer).into_owned()
    }

    fn console(what: &str) -> Console {
        Console::new(what, Arc::new(|_, _| vec!["no the test's authd answers nothing".into()]))
    }

    /// An HTTP/2 connection to the console on `port`, as a browser makes
    /// it, and the task that drives it, which ends when the connection
    /// does.
    async fn connect_h2(
        console: &Console,
        port: u16,
    ) -> (
        hyper::client::conn::http2::SendRequest<http_body_util::Empty<bytes::Bytes>>,
        tokio::task::JoinHandle<()>,
    ) {
        let tls = connect_offering(console, port, &[b"h2", b"http/1.1"]).await;
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
        let (sender, connection) =
            hyper::client::conn::http2::handshake(Local, TokioIo::new(tls)).await.unwrap();
        let driven = tokio::task::spawn_local(async move {
            // A connection the server tells to go away ends either way.
            let _ = connection.await;
        });
        (sender, driven)
    }

    /// `GET /api/1/health` over HTTP/2, to `authority`, with a `Host`
    /// header when `host` is given.
    fn health_h2(
        authority: &str,
        host: Option<&str>,
    ) -> http::Request<http_body_util::Empty<bytes::Bytes>> {
        let mut request = http::Request::get(format!("https://{authority}/api/1/health"))
            .version(http::Version::HTTP_2);
        if let Some(host) = host {
            request = request.header(http::header::HOST, host);
        }
        request.body(http_body_util::Empty::new()).unwrap()
    }

    /// The status and the body of the answer to `request`.
    async fn asked_h2(
        sender: &mut hyper::client::conn::http2::SendRequest<http_body_util::Empty<bytes::Bytes>>,
        request: http::Request<http_body_util::Empty<bytes::Bytes>>,
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        let answer = sender.send_request(request).await.unwrap();
        assert_eq!(answer.version(), http::Version::HTTP_2);
        let status = answer.status().as_u16();
        let body = answer.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[test]
    fn a_browser_that_offers_http_2_has_its_requests_answered_together_on_one_connection() {
        run(async {
            let console = console("serve-h2");
            let port = serve_on_loopback(&console.shared, Duration::from_secs(10));
            let (sender, driven) = connect_h2(&console, port).await;
            // Twenty at once, as a browser's tabs ask.
            let asking: Vec<_> = (0..20)
                .map(|_| {
                    let mut sender = sender.clone();
                    tokio::task::spawn_local(async move {
                        asked_h2(&mut sender, health_h2("nas.example:9090", None)).await
                    })
                })
                .collect();
            for asked in asking {
                assert_eq!(asked.await.unwrap(), (200, "{\"ok\":true}".to_string()));
            }
            assert_eq!(console.shared.connections.borrow().total(), 1);
            // The name is the `:authority`'s; a `Host` beside it must say
            // the same, in any case.
            let mut sender = sender;
            for (authority, host, status) in [
                ("nas.example:9090", Some("NAS.example:9090"), 200),
                ("evil.example", None, 421),
                ("nas.example:9090", Some("evil.example"), 421),
                ("evil.example", Some("nas.example:9090"), 421),
            ] {
                let (got, _) = asked_h2(&mut sender, health_h2(authority, host)).await;
                assert_eq!(got, status, "{authority} {host:?}");
            }
            drop(sender);
            tokio::time::timeout(Duration::from_secs(5), driven).await.unwrap().unwrap();
        });
    }

    #[test]
    fn an_http_2_connection_with_no_request_open_goes_away_once_quiet() {
        run(async {
            let console = console("serve-h2-quiet");
            let quiet = Duration::from_millis(400);
            let port = serve_quiet_on_loopback(
                &console.shared,
                Times { head: Duration::from_secs(10), quiet },
            );
            let (mut sender, driven) = connect_h2(&console, port).await;
            assert_eq!(asked_h2(&mut sender, health_h2("nas.example:9090", None)).await.0, 200);
            let answered = Instant::now();
            // The server tells it to go away, and its place is free.
            tokio::time::timeout(Duration::from_secs(5), driven).await.unwrap().unwrap();
            assert!(answered.elapsed() >= quiet - Duration::from_millis(50));
            for _ in 0..100 {
                if console.shared.connections.borrow().total() == 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("the quiet connection still holds its place");
        });
    }

    /// Serves one connection on a port of its own, as `connection` does,
    /// with a service that answers every request 200 and asks the
    /// connection to end, as `fresh` does, when its path is `/end`. Gives
    /// the port, and the task that ends when the connection does.
    fn serve_ending_once(console: &Console) -> (u16, tokio::task::JoinHandle<()>) {
        let socket = listener(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let port = socket.local_addr().unwrap().port();
        let config = Arc::clone(&console.shared.certificate.borrow().config);
        let served = tokio::task::spawn_local(async move {
            let (stream, _) = socket.accept().await.unwrap();
            let tls = TlsAcceptor::from(config).accept(stream).await.unwrap();
            let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
            let connection = Rc::new(Connection::new(None, None));
            let open = Rc::new(Open::default());
            let answer = {
                let (connection, open) = (Rc::clone(&connection), Rc::clone(&open));
                move |request: http::Request<hyper::body::Incoming>| {
                    if request.uri().path() == "/end" {
                        connection.ending.notify_one();
                    }
                    let body = http_body_util::BodyExt::boxed_unsync(http_body_util::Full::new(
                        bytes::Bytes::from_static(b"ok"),
                    ));
                    let counted = Counted { body, _answering: Answering::new(&open) };
                    async move { Ok::<_, std::convert::Infallible>(http::Response::new(counted)) }
                }
            };
            let io = TokioIo::new(tls);
            if h2 {
                served_h2(io, service_fn(answer), &open, KEEP_ALIVE, &connection).await;
            } else {
                let served = http1::Builder::new()
                    .timer(TokioTimer::new())
                    .keep_alive(true)
                    .serve_connection(io, service_fn(answer))
                    .with_upgrades();
                let told = connection.ending.notified();
                served_until(served, http1::UpgradeableConnection::graceful_shutdown, told).await;
            }
        });
        (port, served)
    }

    #[test]
    fn an_answer_that_asks_its_connection_to_end_ends_it_on_http_2_and_http_1_1() {
        run(async {
            let console = console("serve-ending");
            // HTTP/2: the connection is told to go away once the answer
            // is sent, and the client's next request needs a new one.
            let (port, served) = serve_ending_once(&console);
            let (mut sender, driven) = connect_h2(&console, port).await;
            let ask = |path: &str| {
                http::Request::get(format!("https://nas.example:{port}{path}"))
                    .version(http::Version::HTTP_2)
                    .body(http_body_util::Empty::new())
                    .unwrap()
            };
            assert_eq!(asked_h2(&mut sender, ask("/a")).await, (200, "ok".into()));
            assert_eq!(asked_h2(&mut sender, ask("/end")).await, (200, "ok".into()));
            tokio::time::timeout(Duration::from_secs(5), driven).await.unwrap().unwrap();
            tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
            // HTTP/1.1: the connection kept alive is closed after the answer.
            let (port, served) = serve_ending_once(&console);
            let mut stream = connect(&console, port).await;
            let head = "GET /a HTTP/1.1\r\nHost: nas.example\r\n\r\nGET /end HTTP/1.1\r\nHost: nas.example\r\n\r\n";
            stream.write_all(head.as_bytes()).await.unwrap();
            let mut answer = Vec::new();
            let read =
                tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer)).await;
            assert!(read.is_ok(), "the connection was not closed");
            let answer = String::from_utf8_lossy(&answer);
            assert_eq!(answer.matches("HTTP/1.1 200 OK").count(), 2, "{answer}");
            tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
        });
    }

    #[test]
    fn a_connection_is_quiet_only_while_no_request_is_open_and_an_answer_counts_to_its_end() {
        run(async {
            let open = Rc::new(Open::default());
            let quiet = Duration::from_millis(300);
            let asked = Answering::new(&open);
            let body = Counted {
                body: http_body_util::BodyExt::boxed_unsync(http_body_util::Empty::new()),
                _answering: Answering::new(&open),
            };
            drop(asked);
            assert_eq!(open.count.get(), 1);
            // An answer still being sent keeps it from being quiet.
            let mut watching = pin!(quiet_for_long(&open, quiet));
            let waited = tokio::time::timeout(quiet * 3, watching.as_mut()).await;
            assert!(waited.is_err());
            drop(body);
            assert_eq!(open.count.get(), 0);
            let ended = Instant::now();
            watching.await;
            let took = ended.elapsed();
            assert!(took >= quiet - Duration::from_millis(20) && took < quiet * 4, "{took:?}");
        });
    }

    #[test]
    fn a_browser_gets_the_health_answer_over_tls_and_requests_may_follow_one_another() {
        run(async {
            let console = console("serve-health");
            let port = serve_on_loopback(&console.shared, Duration::from_secs(10));
            let one = "GET /api/1/health HTTP/1.1\r\nHost: nas.example:9090\r\n\r\n";
            let two =
                "GET /api/1/health HTTP/1.1\r\nHost: nas.example:9090\r\nConnection: close\r\n\r\n";
            let answer = exchange(&console, port, &format!("{one}{two}")).await;
            assert_eq!(answer.matches("HTTP/1.1 200 OK").count(), 2, "{answer}");
            assert_eq!(answer.matches("{\"ok\":true}").count(), 2);
            let lower = answer.to_ascii_lowercase();
            assert!(lower.contains("content-security-policy: default-src 'self'"));
            assert!(!lower.contains("\r\nserver:"), "{answer}");
            // Another name is another machine's.
            let rebound =
                "GET /api/1/health HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n";
            assert!(exchange(&console, port, rebound).await.starts_with("HTTP/1.1 421"));
        });
    }

    #[test]
    fn a_head_of_more_than_16_kib_or_64_headers_is_refused_with_431() {
        run(async {
            let console = console("serve-heads");
            let port = serve_on_loopback(&console.shared, Duration::from_secs(10));
            let head = |extra: &str| {
                format!("GET /api/1/health HTTP/1.1\r\nHost: nas.example:9090\r\n{extra}\r\n")
            };
            let large = head(&format!("X-Pad: {}\r\n", "a".repeat(20 << 10)));
            assert!(exchange(&console, port, &large).await.starts_with("HTTP/1.1 431"));
            let headers = |count: u32| "X-1: 1\r\n".repeat(count as usize);
            let many = headers(70);
            assert!(exchange(&console, port, &head(&many)).await.starts_with("HTTP/1.1 431"));
            let few = headers(40);
            let closing = head(&format!("{few}Connection: close\r\n"));
            assert!(exchange(&console, port, &closing).await.starts_with("HTTP/1.1 200"));
        });
    }

    #[test]
    fn a_client_that_sends_no_head_in_time_is_cut_off_with_or_without_a_handshake() {
        run(async {
            let console = console("serve-slow");
            let port = serve_on_loopback(&console.shared, Duration::from_millis(300));
            let started = std::time::Instant::now();
            // No handshake at all.
            let mut quiet = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
            let mut buffer = [0u8; 64];
            let read = tokio::time::timeout(Duration::from_secs(5), quiet.read(&mut buffer)).await;
            assert!(matches!(read, Ok(Ok(0) | Err(_))), "{read:?}");
            assert!(started.elapsed() >= Duration::from_millis(250));
            // A handshake and then a head that never ends.
            let mut slow = connect(&console, port).await;
            slow.write_all(b"GET /api/1/health HTTP/1.1\r\nHost: nas.exa").await.unwrap();
            let mut rest = Vec::new();
            let read =
                tokio::time::timeout(Duration::from_secs(5), slow.read_to_end(&mut rest)).await;
            assert!(read.is_ok(), "not closed in time");
            assert!(!String::from_utf8_lossy(&rest).contains("200 OK"));
            // Nothing holds a place once they are gone.
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(console.shared.connections.borrow().total(), 0);
        });
    }

    #[test]
    fn a_connection_is_served_only_within_the_limits_and_from_an_address_that_is_answered() {
        run(async {
            let console = console("serve-admission");
            let shared = &console.shared;
            let ip = |text: &str| text.parse::<IpAddr>().unwrap();
            // 32 from one address; the 33rd waits for one to end.
            let held: Vec<Slot> =
                (0..32).map(|_| admission(shared, ip("192.168.1.5")).unwrap()).collect();
            let refused = admission(shared, ip("192.168.1.5")).err();
            assert_eq!(refused, Some("there are as many connections as it takes"));
            assert!(admission(shared, ip("192.168.1.6")).is_ok());
            drop(held);
            assert!(admission(shared, ip("192.168.1.5")).is_ok());
            // Private networks only: this one's, and the ranges no one
            // reaches from the internet.
            for private in [
                "127.0.0.1",
                "10.1.2.3",
                "172.16.0.9",
                "192.168.9.9",
                "100.64.1.1",
                "fd12::1",
                "fe80::1",
                "::ffff:192.168.1.9",
            ] {
                assert!(admission(shared, ip(private)).is_ok(), "{private}");
            }
            for public in ["203.0.113.9", "8.8.8.8", "2001:db8::1", "::ffff:8.8.8.8", "100.128.0.1"]
            {
                assert_eq!(
                    admission(shared, ip(public)).err(),
                    Some("it is not on a network admind answers"),
                    "{public}"
                );
            }
            // The count is of the /64 for IPv6, and a refused one holds no place.
            shared.settings.borrow_mut().console.reach = Reach::Any;
            let six: Vec<Slot> = (0..32)
                .map(|n| admission(shared, ip(&format!("2001:db8::{n:x}"))).unwrap())
                .collect();
            assert!(admission(shared, ip("2001:db8::ffff")).is_err());
            assert!(admission(shared, ip("2001:db8:1::1")).is_ok());
            drop(six);
            assert_eq!(shared.connections.borrow().total(), 0);
        });
    }
}
