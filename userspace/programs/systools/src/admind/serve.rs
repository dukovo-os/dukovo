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
//! and then its first request's head within 10 seconds, so that a client
//! that says nothing holds a place for no longer than that; hyper reads
//! the heads (16 KiB and 64 headers at most) and closes a connection that
//! sends none for 60 seconds, which is the keep-alive.
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
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;

use super::files::{self, Files};
use super::limits::Slot;
use super::log;
use super::reach;
use super::routes;
use super::shared::{Identity, Shared};
use super::tls::{self, Certificate};
use crate::admin_settings::{Reach, Settings};
use crate::util::{self, Fail, fail};

/// How long a connection has for its TLS handshake and the head of its
/// first request together.
const HEAD_TIME: Duration = Duration::from_secs(10);

/// How long a connection may wait for the head of its next request.
const KEEP_ALIVE: Duration = Duration::from_secs(60);

/// The most bytes a request's head may have, and the most headers.
const HEAD_MAX: usize = 16 << 10;
const HEADERS_MAX: usize = 64;

/// How often the server looks after itself.
const UPKEEP_EVERY: Duration = Duration::from_secs(30);

/// How often it looks at the signals that came.
const SIGNALS_EVERY: Duration = Duration::from_millis(500);

/// The file consoled shows at its sign-in prompt: where to open the console.
const ISSUE: &str = "/system/live/admind/issue";

/// Set by SIGTERM: admind stops. Set by SIGHUP: it reads its certificate
/// again.
static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_hangup(_: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
}

/// Has the signals admind answers set the flags above, and a browser that
/// goes away not end it: the write to it fails instead.
fn install_signals() {
    // SAFETY: setting a signal's disposition touches no memory.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    for (signal, handler) in [
        (libc::SIGTERM, on_term as extern "C" fn(libc::c_int)),
        (libc::SIGHUP, on_hangup as extern "C" fn(libc::c_int)),
    ] {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers only store to atomics.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
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
    let named = certificate
        .iter()
        .find(|name| name.contains('.') && name.parse::<IpAddr>().is_err());
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
    /// The issue file as last written.
    issue: String,
    /// The last words the settings gave when they did not read, so that
    /// the log says them once and not every 30 seconds.
    complained: Option<String>,
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
        let names = shared.certificate.borrow().names.clone();
        *shared.identity.borrow_mut() = Identity::read(&names);
        shared.maintain(std::time::Instant::now());
        self.issue(shared);
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
                let names = certificate.names.clone();
                *shared.certificate.borrow_mut() = certificate;
                *shared.identity.borrow_mut() = Identity::read(&names);
            }
            Err(why) => {
                log(4, &format!("{why}: admind goes on with the certificate it has"));
                *shared.settings.borrow_mut() = merged(&old, read, false);
            }
        }
        self.issue(shared);
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
async fn head_in_time<F: Future>(work: F, seen: &Cell<bool>, deadline: Instant) -> Option<F::Output> {
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

/// Serves one connection, which holds `slot` until it ends. `head_time`
/// is how long its handshake and first head have.
async fn connection(
    shared: Rc<Shared>,
    stream: TcpStream,
    peer: IpAddr,
    head_time: Duration,
    slot: Slot,
) {
    let deadline = Instant::now() + head_time;
    // A browser that went away without a word is found out by the
    // network's own probes, which also frees the place of an event stream
    // that no one reads. A socket that cannot be set so is still a socket.
    let probes = TcpKeepalive::new().with_time(KEEP_ALIVE).with_interval(Duration::from_secs(15));
    if SockRef::from(&stream).set_tcp_keepalive(&probes).is_err() {
        log(7, "a connection's keep-alive probes could not be set");
    }
    let acceptor = TlsAcceptor::from(Arc::clone(&shared.certificate.borrow().config));
    let tls = match tokio::time::timeout_at(deadline, acceptor.accept(stream)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(err)) => {
            shared.refused_connection(peer, &format!("its TLS handshake failed: {err}"));
            return;
        }
        Err(_) => {
            shared.refused_connection(peer, "its TLS handshake did not finish in time");
            return;
        }
    };
    let seen = Rc::new(Cell::new(false));
    let service = {
        let (shared, seen) = (Rc::clone(&shared), Rc::clone(&seen));
        service_fn(move |request| {
            seen.set(true);
            routes::handle(Rc::clone(&shared), peer, request)
        })
    };
    let served = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(KEEP_ALIVE)
        .max_buf_size(HEAD_MAX)
        .max_headers(HEADERS_MAX)
        .keep_alive(true)
        .serve_connection(TokioIo::new(tls), service);
    // A connection that ends badly is a browser that went away: nothing
    // to say about it.
    if head_in_time(served, &seen, deadline).await.is_none() {
        shared.refused_connection(peer, "it sent no request in time");
    }
    drop(slot);
}

/// What to do with a connection from `peer`: serve it, or close it with no
/// word and note why.
fn admit(shared: &Rc<Shared>, stream: TcpStream, peer: SocketAddr) {
    let peer = reach::plain(peer.ip());
    let Some(slot) = Slot::take(&shared.connections, reach::counted(peer)) else {
        shared.refused_connection(peer, "there are as many connections as it takes");
        return;
    };
    let kind: Reach = shared.settings.borrow().console.reach;
    if !reach::answers(kind, peer, &shared.identity.borrow().networks) {
        shared.refused_connection(peer, "it is not on a network admind answers");
        return;
    }
    tokio::task::spawn_local(connection(Rc::clone(shared), stream, peer, HEAD_TIME, slot));
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
    format!("answers at {address} to {reach}; the certificate's SHA-256 fingerprint is {fingerprint}")
}

/// Serves until SIGTERM.
async fn serve(shared: Rc<Shared>) -> Result<(), String> {
    let sockets = listen(shared.port)?;
    let mut upkeep = Upkeep::default();
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
    let mut last = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        tokio::time::sleep(SIGNALS_EVERY).await;
        if RELOAD.swap(false, Ordering::Relaxed) {
            upkeep.reload(&shared);
        }
        if last.elapsed() >= UPKEEP_EVERY {
            last = Instant::now();
            upkeep.tick(&shared);
        }
    }
    log(6, "stops: it ends its sessions");
    shared.end_all().await;
    // A console that is no more has no address to show.
    if let Err(err) = fs::remove_file(ISSUE) {
        if err.kind() != io::ErrorKind::NotFound {
            log(4, &format!("cannot remove {ISSUE}: {}", util::strip_os_error(&err)));
        }
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
        log(3, &why);
        Files::default()
    });
    for line in said {
        log(4, &line);
    }
    let identity = Identity::read(&certificate.names);
    let shared = Shared::new(settings, identity, certificate, files, accounts::SOCKET);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| util::failed(&"the runtime", err))?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, serve(shared)).map_err(fail)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_settings::SecondStep;

    #[test]
    fn the_rules_and_reach_take_effect_and_a_port_waits_for_a_restart() {
        let old = Settings::parse("[console]\nlisten = 9090\ncertificate = \"this-machine\"\n").unwrap();
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
        assert_eq!((read.console.listen, read.console.certificate.as_str()), (9090, "admin.example.org"));
    }

    #[test]
    fn the_console_is_opened_at_the_first_name_with_dots_the_certificate_has() {
        let names = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        let this = names(&["duke-nas", "duke-nas.local", "localhost"]);
        assert_eq!(address_for("duke-nas", &this, 9090), "https://duke-nas.local:9090");
        assert_eq!(address_for("duke-nas", &names(&["admin.example.org", "10.0.0.5"]), 443), "https://admin.example.org");
        // A certificate with no such name leaves the machine's own, and an
        // address is no name.
        assert_eq!(address_for("duke-nas", &names(&["10.0.0.5", "localhost"]), 9090), "https://duke-nas:9090");
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

    fn run<F: Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(future)
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
            assert_eq!(head_in_time(quick, &Cell::new(false), Instant::now() + HEAD_TIME).await, Some(1));
        });
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
}
