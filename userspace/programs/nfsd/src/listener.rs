// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `nfsd`: listens on port 2049 for NFS 4.1 and 4.2 over TCP, a thread
//! for each connection, and keeps the workers authd starts, one for each
//! person calls come as.

use std::collections::HashMap;
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

use crate::ipc::{self, Answer, Request};
use crate::rpc::{self, NFS_PROGRAM, NFS_VERSION};
use crate::server::{Backend, Context, Server};
use crate::sys;
use crate::xdr::Reader;

/// NFS's port.
pub const PORT: u16 = 2049;
/// The most connections at once.
const CONNECTIONS_MAX: usize = 256;
/// How long a connection may say nothing: Linux's client closes one it
/// left idle for five minutes, and calls at least once a lease.
const IDLE: Duration = Duration::from_mins(15);
/// The most workers kept: past it, the one called longest ago ends.
const WORKERS_MAX: usize = 64;
/// How long a worker may take to answer: a search for a handle through a
/// large share is the longest.
const WORKER_TIME: Duration = Duration::from_secs(120);
/// How long authd may take to start one.
const AUTHD_TIME: Duration = Duration::from_secs(10);

/// The workers, each by the person it serves, and when it was last
/// called.
#[derive(Default)]
pub struct Pool {
    workers: HashMap<u32, (UnixStream, Instant)>,
}

impl Pool {
    /// The worker of the person numbered `uid`, started if there is none.
    fn worker(&mut self, uid: u32) -> io::Result<&mut UnixStream> {
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
        let (stream, at) =
            self.workers.get_mut(&uid).ok_or_else(|| io::Error::other("no worker"))?;
        *at = Instant::now();
        Ok(stream)
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
fn exchange(stream: &mut UnixStream, message: &[u8]) -> io::Result<Answer> {
    ipc::send(stream, message)?;
    if !sys::readable(stream, WORKER_TIME)? {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer in time"));
    }
    let answer = ipc::receive(stream)?.ok_or(io::ErrorKind::UnexpectedEof)?;
    ipc::decode_answer(&answer)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a broken answer"))
}

/// Has authd start a worker as the person numbered `uid`: nfsd's end of
/// its socket.
fn start_worker(uid: u32) -> io::Result<UnixStream> {
    let (ours, theirs) = UnixStream::pair()?;
    let stream = UnixStream::connect(accounts::SOCKET)?;
    let request = AuthdRequest::NfsWorker { uid };
    sys::send_with_fds(
        &stream,
        &request.encode(),
        &[theirs.as_raw_fd(), io::stderr().as_raw_fd()],
    )?;
    stream.shutdown(Shutdown::Write)?;
    drop(theirs);
    if !sys::readable(&stream, AUTHD_TIME)? {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "authd did not answer in time"));
    }
    let mut line = String::new();
    (&stream).take(4096).read_to_string(&mut line)?;
    match AuthdAnswer::decode(&line) {
        AuthdAnswer::Done(_) => Ok(ours),
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
    match call.procedure {
        0 => Some(rpc::accepted(call.xid, rpc::SUCCESS).bytes),
        1 => {
            // Root is served as no one, as is a call with no credential.
            let uid = if call.flavor == rpc::AUTH_SYS && call.caller.uid != 0 {
                call.caller.uid
            } else {
                rpc::NOBODY
            };
            let cx = Context { peer, uid };
            let args = &record[r.position()..];
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

/// Serves one connection until it ends, or its client's address may
/// reach no share.
fn serve<B: Backend>(
    stream: &TcpStream,
    from: SocketAddr,
    server: &Mutex<Server<B>>,
) -> io::Result<()> {
    let peer = from.ip().to_canonical();
    let _ = stream.set_nodelay(true);
    sys::keep_alive(stream);
    stream.set_read_timeout(Some(IDLE))?;
    let mut input = BufReader::with_capacity(256 * 1024, stream);
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
