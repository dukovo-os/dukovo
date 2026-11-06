// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `nfsd`: listens on port 2049 for NFS 4.1 and 4.2 over TCP, a thread
//! for each connection, and keeps the workers authd starts, one for each
//! person calls come as.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use accounts::request::{Answer as AuthdAnswer, Request as AuthdRequest};

use crate::contexts::Refused;
use crate::gss::{self, Flavor, RPCSEC_GSS};
use crate::ipc::{self, Answer, Request};
use crate::rpc::{self, Call, GssCall, NFS_PROGRAM, NFS_VERSION, Refusal};
use crate::server::{Backend, Context, Server};
use crate::sys;
use crate::xdr::Reader;

/// NFS's port.
pub const PORT: u16 = 2049;
/// The most connections at once: Linux's client makes one to a server,
/// or up to 16 with `nconnect`, and each may hold a record of up to a
/// megabyte while it waits its turn.
const CONNECTIONS_MAX: usize = 64;
/// How long a connection may say nothing: Linux's client closes one it
/// left idle for five minutes, and calls at least once a lease.
const IDLE: Duration = Duration::from_mins(15);
/// The most workers kept: past it, the one called longest ago ends.
const WORKERS_MAX: usize = 64;

/// Writes `line` to the log, up to ten times a minute. Why a call under
/// Kerberos was refused is where to start when a mount fails, but a
/// client can send refused calls without end, and the log is shared.
fn log_refusal(line: &str) {
    static LOGGED: Mutex<(u64, u32)> = Mutex::new((0, 0));
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let minute = since.map_or(0, |since| since.as_secs() / 60);
    let mut logged = LOGGED.lock().unwrap_or_else(PoisonError::into_inner);
    if logged.0 != minute {
        *logged = (minute, 0);
    }
    logged.1 += 1;
    if logged.1 <= 10 {
        eprintln!("{line}");
    }
}
/// How long a worker may take to answer: a search for a handle through a
/// large share is the longest.
const WORKER_TIME: Duration = Duration::from_secs(120);
/// How long authd may take to start one.
const AUTHD_TIME: Duration = Duration::from_secs(10);

/// The workers, each by the person it serves, and when it was last
/// called.
#[derive(Default)]
pub struct Pool {
    workers: HashMap<u32, (Link, Instant)>,
}

/// The two pipes to a worker and back. Not a socket pair: Radix sends no
/// socket with another's bytes, so that none keeps itself open (ADR
/// 0091), and authd is sent the worker's ends.
struct Link {
    /// Where requests go.
    to: File,
    /// Where answers come from.
    from: File,
}

impl Pool {
    /// The worker of the person numbered `uid`, started if there is none.
    fn worker(&mut self, uid: u32) -> io::Result<&mut Link> {
        if !self.workers.contains_key(&uid) {
            if self.workers.len() >= WORKERS_MAX {
                let oldest =
                    self.workers.iter().min_by_key(|(_, (_, at))| *at).map(|(uid, _)| *uid);
                if let Some(oldest) = oldest {
                    self.workers.remove(&oldest);
                }
            }
            let stream = start_worker(uid)?;
            self.workers.insert(uid, (stream, Instant::now()));
        }
        let (link, at) = self.workers.get_mut(&uid).ok_or_else(|| io::Error::other("no worker"))?;
        *at = Instant::now();
        Ok(link)
    }
}

impl Backend for Pool {
    fn call(&mut self, uid: u32, request: Request) -> Answer {
        let message = request.encode();
        // A worker that ended is started again, once.
        for _ in 0..2 {
            let worker = match self.worker(uid) {
                Ok(worker) => worker,
                Err(err) => {
                    eprintln!("nfsd: cannot start a worker for number {uid}: {err}");
                    return Err(libc::EIO);
                }
            };
            match exchange(worker, &message) {
                Ok(answer) => return answer,
                Err(err) => {
                    eprintln!("nfsd: the worker for number {uid} failed: {err}");
                    self.workers.remove(&uid);
                }
            }
        }
        Err(libc::EIO)
    }

    fn restart(&mut self) {
        self.workers.clear();
    }
}

/// Sends a worker `message` and reads its answer, which it has
/// [`WORKER_TIME`] to give.
fn exchange(link: &mut Link, message: &[u8]) -> io::Result<Answer> {
    ipc::send(&mut link.to, message)?;
    if !sys::readable(&link.from, WORKER_TIME)? {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer in time"));
    }
    let answer = ipc::receive(&mut link.from)?.ok_or(io::ErrorKind::UnexpectedEof)?;
    ipc::decode_answer(&answer)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a broken answer"))
}

/// Has authd start a worker as the person numbered `uid`: nfsd's ends of
/// the pipes to it and back.
fn start_worker(uid: u32) -> io::Result<Link> {
    let (requests, to) = sys::pipe()?;
    let (from, answers) = sys::pipe()?;
    let stream = UnixStream::connect(accounts::SOCKET)?;
    let request = AuthdRequest::NfsWorker { uid };
    sys::send_with_fds(
        &stream,
        &request.encode(),
        &[requests.as_raw_fd(), answers.as_raw_fd(), io::stderr().as_raw_fd()],
    )?;
    stream.shutdown(Shutdown::Write)?;
    // The worker holds them now; nfsd's copies would keep each pipe open
    // after the worker ended.
    drop((requests, answers));
    if !sys::readable(&stream, AUTHD_TIME)? {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "authd did not answer in time"));
    }
    let mut line = String::new();
    (&stream).take(4096).read_to_string(&mut line)?;
    match AuthdAnswer::decode(&line) {
        AuthdAnswer::Done(_) => Ok(Link { to: File::from(to), from: File::from(from) }),
        AuthdAnswer::Refused(why) => Err(io::Error::other(why)),
    }
}

/// The answer to one RPC record from `peer`: none for one that is no
/// call.
pub fn answer<B: Backend>(
    server: &Mutex<Server<B>>,
    peer: IpAddr,
    record: &[u8],
) -> Option<Vec<u8>> {
    let mut r = Reader::new(record);
    let call = match rpc::read_call(&mut r) {
        Ok(call) => call,
        Err((Some(xid), refusal)) => return rpc::refused(xid, &refusal),
        Err((None, _)) => return None,
    };
    if call.program != NFS_PROGRAM {
        return Some(rpc::accepted(call.xid, rpc::PROG_UNAVAIL).bytes);
    }
    if call.version != NFS_VERSION {
        let mut w = rpc::accepted(call.xid, rpc::PROG_MISMATCH);
        w.u32(NFS_VERSION).u32(NFS_VERSION);
        return Some(w.bytes);
    }
    let args = &record[r.position()..];
    if let Some(gss) = &call.gss {
        return kerberos(server, peer, &call, gss, record, args);
    }
    match call.procedure {
        0 => Some(rpc::accepted(call.xid, rpc::SUCCESS).bytes),
        1 => {
            // Root is served as no one, as is a call with no credential.
            let uid = if call.flavor == rpc::AUTH_SYS && call.caller.uid != 0 {
                call.caller.uid
            } else {
                rpc::NOBODY
            };
            let cx = Context { peer, uid, flavor: Flavor::Sys };
            let mut server = server.lock().unwrap_or_else(PoisonError::into_inner);
            Some(match server.compound(&cx, args) {
                Some(results) => {
                    let mut w = rpc::accepted(call.xid, rpc::SUCCESS);
                    w.bytes.extend_from_slice(&results);
                    w.bytes
                }
                None => rpc::accepted(call.xid, rpc::GARBAGE_ARGS).bytes,
            })
        }
        _ => Some(rpc::accepted(call.xid, rpc::PROC_UNAVAIL).bytes),
    }
}

/// The answer to an RPCSEC_GSS call (RFC 2203 5.3.3): a context set up
/// (INIT), a call under one (DATA) or its end (DESTROY). `args` are the
/// call's arguments in `record`, protected as its credential says.
fn kerberos<B: Backend>(
    server: &Mutex<Server<B>>,
    peer: IpAddr,
    call: &Call,
    gss: &GssCall,
    record: &[u8],
    args: &[u8],
) -> Option<Vec<u8>> {
    let (xid, cred) = (call.xid, &gss.cred);
    let refuse = |stat| rpc::refused(xid, &Refusal::Auth(stat));
    let mut server = server.lock().unwrap_or_else(PoisonError::into_inner);
    if cred.proc == gss::proc::INIT {
        // A context is set up with NULL; Kerberos takes one round.
        if call.procedure != 0 {
            return refuse(rpc::AUTH_BADCRED);
        }
        let Ok(token) = Reader::new(args).opaque(64 * 1024) else {
            return Some(rpc::accepted(xid, rpc::GARBAGE_ARGS).bytes);
        };
        let (result, verifier) = server.contexts.init(token, &peer.to_string());
        let mut w = match verifier {
            Some(mic) => rpc::accepted_with(xid, RPCSEC_GSS, &mic, rpc::SUCCESS),
            None => rpc::accepted(xid, rpc::SUCCESS),
        };
        w.bytes.extend_from_slice(&result.encode());
        return Some(w.bytes);
    }
    if !matches!(cred.proc, gss::proc::DATA | gss::proc::DESTROY) || gss.verifier.0 != RPCSEC_GSS {
        return refuse(rpc::AUTH_BADCRED);
    }
    let header = &record[gss.header.clone()];
    let (verifier, uid, body) =
        match server.contexts.check(&cred.handle, cred.seq, header, &gss.verifier.1) {
            Ok(held) => {
                let verifier = gss::reply_verifier(&mut held.context, cred.seq);
                let body = (call.procedure == 1)
                    .then(|| gss::unprotect(&held.context, cred.service, cred.seq, args));
                (verifier, held.uid, body)
            }
            Err(Refused::Credential) => {
                let why = if server.contexts.holds(&cred.handle) {
                    "its verifier does not check"
                } else {
                    "no context has its handle, or its ticket ran out"
                };
                log_refusal(&format!("nfsd: Kerberos: refused a call from {peer}: {why}"));
                return refuse(gss::CREDPROBLEM);
            }
            Err(Refused::Context) => return refuse(gss::CTXPROBLEM),
            // A replay, or a call the client gave up on: no answer.
            Err(Refused::Dropped) => return None,
        };
    let reply = |stat| rpc::accepted_with(xid, RPCSEC_GSS, &verifier, stat);
    if cred.proc == gss::proc::DESTROY {
        server.contexts.destroy(&cred.handle);
        return Some(reply(rpc::SUCCESS).bytes);
    }
    let results = match (call.procedure, body) {
        (0, _) => Vec::new(),
        (1, Some(Ok(body))) => {
            let cx = Context { peer, uid, flavor: Flavor::Krb5(cred.service) };
            match server.compound(&cx, &body) {
                Some(results) => results,
                None => return Some(reply(rpc::GARBAGE_ARGS).bytes),
            }
        }
        // Arguments whose seal or signature does not check are garbage
        // too (RFC 2203 5.3.3.4.2).
        (1, Some(Err(err))) => {
            let service = cred.service.name();
            log_refusal(&format!("nfsd: Kerberos: a call from {peer} under {service}: {err:?}"));
            return Some(reply(rpc::GARBAGE_ARGS).bytes);
        }
        (1, _) => return Some(reply(rpc::GARBAGE_ARGS).bytes),
        _ => return Some(reply(rpc::PROC_UNAVAIL).bytes),
    };
    let Some(context) = server.contexts.context(&cred.handle) else {
        return refuse(gss::CREDPROBLEM);
    };
    match gss::protect(context, cred.service, cred.seq, &results) {
        Ok(protected) => {
            let mut w = reply(rpc::SUCCESS);
            w.bytes.extend_from_slice(&protected);
            Some(w.bytes)
        }
        Err(err) => {
            eprintln!("nfsd: Kerberos: cannot seal a reply to {peer}: {err}");
            Some(reply(rpc::SYSTEM_ERR).bytes)
        }
    }
}

/// Serves one connection until it ends, or its client's address may
/// reach no share.
pub(crate) fn serve<B: Backend>(
    stream: &TcpStream,
    from: SocketAddr,
    server: &Mutex<Server<B>>,
) -> io::Result<()> {
    let peer = from.ip().to_canonical();
    let _ = stream.set_nodelay(true);
    sys::keep_alive(stream);
    stream.set_read_timeout(Some(IDLE))?;
    let mut input = BufReader::with_capacity(64 * 1024, stream);
    loop {
        if !server.lock().unwrap_or_else(PoisonError::into_inner).reaches(peer) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "no share names its network",
            ));
        }
        let Some(record) = rpc::read_record(&mut input)? else { return Ok(()) };
        if let Some(reply) = answer(server, peer, &record) {
            (&mut &*stream).write_all(&rpc::frame(&reply))?;
        }
    }
}

/// Takes connections from `listener`, a thread each, while fewer than
/// [`CONNECTIONS_MAX`] are open.
fn accept(listener: &TcpListener, server: &Arc<Mutex<Server<Pool>>>, open: &Arc<AtomicUsize>) {
    loop {
        let (stream, from) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(err) => {
                eprintln!("nfsd: cannot accept a connection: {err}");
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        if open.fetch_add(1, Ordering::SeqCst) >= CONNECTIONS_MAX {
            open.fetch_sub(1, Ordering::SeqCst);
            eprintln!("nfsd: refused a connection from {from}: {CONNECTIONS_MAX} are open");
            continue;
        }
        let (server, open) = (Arc::clone(server), Arc::clone(open));
        let spawned = thread::Builder::new().name("connection".into()).spawn(move || {
            if let Err(err) = serve(&stream, from, &server) {
                eprintln!("nfsd: a connection from {} ended: {err}", from.ip().to_canonical());
            }
            open.fetch_sub(1, Ordering::SeqCst);
        });
        if let Err(err) = spawned {
            eprintln!("nfsd: cannot start a thread for a connection: {err}");
        }
    }
}

/// nfsd: listens on port 2049.
pub fn run() -> ExitCode {
    let (boot, verifier) = match (sys::random::<4>(), sys::random::<8>()) {
        (Ok(boot), Ok(verifier)) => (u32::from_be_bytes(boot), verifier),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("nfsd: cannot read random bytes: {err}");
            return ExitCode::FAILURE;
        }
    };
    let server = Server::new(Pool::default(), boot, verifier, &sys::hostname());
    let server = Arc::new(Mutex::new(server));
    let ipv4 = match TcpListener::bind(("0.0.0.0", PORT)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("nfsd: cannot listen on port {PORT}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let open = Arc::new(AtomicUsize::new(0));
    if let Ok(ipv6) = TcpListener::bind(("::", PORT)) {
        let (server, open) = (Arc::clone(&server), Arc::clone(&open));
        let _ = thread::Builder::new()
            .name("listen-ipv6".into())
            .spawn(move || accept(&ipv6, &server, &open));
    }
    eprintln!("nfsd: listening on port {PORT}");
    accept(&ipv4, &server, &open);
    ExitCode::SUCCESS
}
