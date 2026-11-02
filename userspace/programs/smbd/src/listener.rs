// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! smbd as Spark runs it, as root: it listens on port 445, settles each
//! connection's NEGOTIATE, and signs the PC in, with NTLMv2 inside
//! SPNEGO (MS-SPNG), which every SMB client speaks.
//!
//! The listener never sees a password or its hash. It sends the three
//! NTLM messages to authd(8), with the connection and a pipe, and authd
//! checks the answer against the person's sharing password, starts their
//! share session, `smbd --worker`, as them, and tells the listener the
//! session key. The listener checks SPNEGO's mechListMIC with it, makes
//! the session's keys, answers the PC that it signed in, and hands the
//! keys to the worker on the pipe. From then on the worker alone serves
//! the connection, and the listener's thread ends.
//!
//! Before sign-in a frame may be 64 KiB at most, a sign-in takes a
//! minute at most, three wrong passwords end a connection, and at most
//! 64 connections sign in at once; authd makes each wrong password wait
//! longer, by the name and the PC's address.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use accounts::ntlm;
use accounts::request::{Answer, Request};

use crate::auth::{self, Names, State};
use crate::crypto::{self, dialect};
use crate::negotiate::{self, Negotiated, Outcome};
use crate::state::Handoff;
use crate::sys;
use crate::wire::{self, Header, Out, command, status};

/// SMB's port.
const PORT: u16 = 445;
/// The longest frame read before sign-in.
const FRAME_MAX: usize = 64 * 1024;
/// How long a PC has to sign in, besides the time authd takes.
const SIGN_IN_TIME: Duration = Duration::from_secs(60);
/// How long authd may take to answer, a wrong password's wait with it.
const AUTHD_TIME: Duration = Duration::from_secs(90);
/// The most connections signing in at once.
const CONNECTIONS_MAX: usize = 64;
/// The most sign-ins one connection tries.
const TRIES_MAX: usize = 3;
/// SMB2_SESSION_FLAG_ENCRYPT_DATA.
const ENCRYPT_DATA: u16 = 0x0004;

/// What to do after a frame.
#[derive(Debug)]
pub enum Step {
    /// Send this and read on.
    Answer(Vec<u8>),
    /// Send this, if anything, and end the connection, for this reason.
    End(Option<Vec<u8>>, &'static str),
    /// Ask authd about this AUTHENTICATE.
    Check(Box<Pending>),
}

/// An AUTHENTICATE for authd to check.
#[derive(Debug)]
pub struct Pending {
    /// The SESSION_SETUP request's header.
    header: Header,
    /// The NTLM NEGOTIATE, CHALLENGE and AUTHENTICATE messages.
    pub messages: [Vec<u8>; 3],
    /// The client's mechListMIC.
    mic: Option<Vec<u8>>,
}

/// What authd's answer leads to.
#[derive(Debug)]
pub enum Finish {
    /// Send this and read on: the PC may try again.
    Again(Vec<u8>),
    /// Send this and end the connection.
    End(Vec<u8>, String),
    /// Signed in: hand this to the worker, then send the response.
    Accepted(Box<Handoff>, Vec<u8>),
}

/// A connection, until it signed in.
pub struct SignIn {
    /// The PC's address.
    from: String,
    /// This server's GUID.
    server_guid: [u8; 16],
    /// Its names, for NTLM's CHALLENGE.
    names: Names,
    /// The preauthentication context's salt.
    salt: [u8; 32],
    /// What NEGOTIATE settled.
    negotiated: Option<Box<Negotiated>>,
    /// Whether SMB 1's NEGOTIATE came, which may come once, first.
    smb1: bool,
    /// The session's number, given at its first SESSION_SETUP.
    session_id: u64,
    /// The session's preauthentication hash.
    preauth: [u8; 64],
    /// The NTLM NEGOTIATE and CHALLENGE of the sign-in under way.
    ntlm: Option<(Vec<u8>, Vec<u8>)>,
    /// The client's mechanisms, as its NegTokenInit listed them.
    mech_types: Option<Vec<u8>>,
    /// Whether the client wraps NTLM in SPNEGO.
    spnego: bool,
    /// The lowest message number not yet used.
    next_message: u64,
    /// A challenge for each try.
    challenges: Vec<[u8; 8]>,
    /// The tries so far.
    tries: usize,
}

impl SignIn {
    /// A connection from `from`, to the server `server_guid` named
    /// `hostname`, with `salt`, a session number and challenges made at
    /// random.
    pub fn new(
        from: String,
        server_guid: [u8; 16],
        hostname: &str,
        salt: [u8; 32],
        session_id: u64,
        challenges: Vec<[u8; 8]>,
    ) -> SignIn {
        SignIn {
            from,
            server_guid,
            names: Names::of(hostname),
            salt,
            negotiated: None,
            smb1: false,
            session_id: session_id.max(1),
            preauth: [0; 64],
            ntlm: None,
            mech_types: None,
            spnego: false,
            next_message: 0,
            challenges,
            tries: 0,
        }
    }

    /// Answers one frame.
    pub fn frame(&mut self, frame: &[u8]) -> Step {
        if frame.starts_with(&wire::PROTOCOL_1) {
            if self.smb1 || self.negotiated.is_some() {
                return Step::End(None, "SMB 1 after NEGOTIATE");
            }
            self.smb1 = true;
            return match negotiate::smb1(frame, self.server_guid) {
                Some(response) => {
                    // SMB 2's NEGOTIATE follows it as message 1.
                    self.next_message = 1;
                    Step::Answer(response)
                }
                None => Step::End(None, "a PC that speaks only SMB 1, which smbd does not"),
            };
        }
        if !frame.starts_with(&wire::PROTOCOL) {
            return Step::End(None, "a message that is not SMB 2 or 3, before sign-in");
        }
        let Ok(header) = Header::parse(frame) else { return Step::End(None, "a broken header") };
        if header.command == command::CANCEL {
            return Step::Answer(Vec::new());
        }
        if header.message_id < self.next_message || header.message_id == u64::MAX {
            return Step::End(None, "a message number used before");
        }
        self.next_message =
            header.message_id.saturating_add(u64::from(header.credit_charge.max(1)));
        match header.command {
            command::NEGOTIATE => {
                if self.negotiated.is_some() {
                    return Step::End(None, "a second NEGOTIATE");
                }
                match negotiate::negotiate(frame, &header, self.server_guid, self.salt) {
                    Outcome::Settled(negotiated, response) => {
                        self.preauth = negotiated.preauth;
                        self.negotiated = Some(negotiated);
                        Step::Answer(response)
                    }
                    Outcome::Refused(response) => Step::End(
                        Some(response),
                        "no dialect in common: smbd speaks SMB 3, encrypted, and the PC does not",
                    ),
                }
            }
            command::SESSION_SETUP => self.session_setup(&header, frame),
            command::ECHO if self.negotiated.is_some() => {
                let mut body = Out::new();
                body.u16(4).u16(0);
                Step::Answer(self.response(&header, status::SUCCESS, &body.0))
            }
            _ if self.negotiated.is_some() => Step::Answer(self.response(
                &header,
                status::USER_SESSION_DELETED,
                &wire::error_body(),
            )),
            _ => Step::End(None, "a request before NEGOTIATE"),
        }
    }

    /// A response to `header` with `code` and `body`, in the session.
    fn response(&self, header: &Header, code: u32, body: &[u8]) -> Vec<u8> {
        let mut out = header.response(code);
        out.credits = header.credits.clamp(1, 64);
        if header.command == command::SESSION_SETUP {
            out.session_id = self.session_id;
        }
        wire::message(&out, body)
    }

    /// SESSION_SETUP's body with `flags` and `buffer`.
    fn setup_body(flags: u16, buffer: &[u8]) -> Vec<u8> {
        let mut out = Out::new();
        out.u16(9).u16(flags).u16((wire::HEADER + 8) as u16).u16(buffer.len() as u16).bytes(buffer);
        out.0
    }

    /// Refuses a sign-in with STATUS_LOGON_FAILURE.
    fn refuse(&mut self, header: &Header) -> Vec<u8> {
        self.ntlm = None;
        self.response(header, status::LOGON_FAILURE, &wire::error_body())
    }

    /// SESSION_SETUP (MS-SMB2 3.3.5.5), before authd is asked.
    fn session_setup(&mut self, header: &Header, frame: &[u8]) -> Step {
        let Some(negotiated) = &self.negotiated else {
            return Step::End(None, "SESSION_SETUP before NEGOTIATE");
        };
        let dialect = negotiated.dialect;
        if header.session_id != 0 && header.session_id != self.session_id {
            return Step::Answer(self.response(
                header,
                status::USER_SESSION_DELETED,
                &wire::error_body(),
            ));
        }
        // SMB2_SESSION_FLAG_BINDING: another channel of a session, which
        // smbd does not have.
        if wire::u8_at(frame, wire::HEADER + 2).unwrap_or(0) & 1 != 0 {
            return Step::Answer(self.response(
                header,
                status::REQUEST_NOT_ACCEPTED,
                &wire::error_body(),
            ));
        }
        let buffer = match (
            wire::u16_at(frame, wire::HEADER + 12),
            wire::u16_at(frame, wire::HEADER + 14),
        ) {
            (Ok(offset), Ok(length)) => {
                wire::slice_at(frame, usize::from(offset), usize::from(length))
            }
            _ => Err(status::INVALID_PARAMETER),
        };
        let Some(token) = buffer.ok().and_then(auth::read_token) else {
            return Step::Answer(self.refuse(header));
        };
        if header.session_id == 0 {
            // A sign-in starts again.
            self.preauth = negotiated.preauth;
            self.ntlm = None;
            self.mech_types = None;
            self.spnego = false;
        }
        if dialect == dialect::SMB_3_1_1 {
            self.preauth = crypto::preauth(&self.preauth, frame);
        }
        let first = token.mech_types.is_some();
        if first {
            self.mech_types.clone_from(&token.mech_types);
        }
        // NTLM's messages come bare, or wrapped in SPNEGO's tokens; the
        // answers go as the questions came.
        self.spnego = !buffer.is_ok_and(|bytes| bytes.starts_with(ntlm::SIGNATURE));
        let more = |this: &mut SignIn, security: &[u8]| {
            let body = SignIn::setup_body(0, security);
            let response = this.response(header, status::MORE_PROCESSING_REQUIRED, &body);
            if dialect == dialect::SMB_3_1_1 {
                this.preauth = crypto::preauth(&this.preauth, &response);
            }
            Step::Answer(response)
        };
        let Some(message) = token.ntlm else {
            if first && token.ntlm_offered {
                // The PC's first choice is another mechanism: NTLM, it
                // is told, and it sends NTLM's NEGOTIATE next.
                return more(self, &auth::answer(State::Incomplete, true, None, None));
            }
            let mut response = self.refuse(header);
            if first {
                let body = SignIn::setup_body(0, &auth::answer(State::Rejected, false, None, None));
                response = self.response(header, status::LOGON_FAILURE, &body);
            }
            return Step::Answer(response);
        };
        match wire::u32_at(&message, 8) {
            Ok(1) => {
                let Some(&challenge) = self.challenges.get(self.tries) else {
                    return Step::End(Some(self.refuse(header)), "too many tries");
                };
                let challenge = auth::challenge(&message, challenge, wire::now(), &self.names);
                let security = if self.spnego {
                    auth::answer(State::Incomplete, true, Some(&challenge), None)
                } else {
                    challenge.clone()
                };
                self.ntlm = Some((message, challenge));
                more(self, &security)
            }
            Ok(3) => {
                let Some((negotiate, challenge)) = self.ntlm.take() else {
                    return Step::Answer(self.refuse(header));
                };
                self.tries += 1;
                Step::Check(Box::new(Pending {
                    header: *header,
                    messages: [negotiate, challenge, message],
                    mic: token.mic,
                }))
            }
            _ => Step::Answer(self.refuse(header)),
        }
    }

    /// What authd's answer to `pending` leads to: its account and the
    /// session key, or why it refused.
    pub fn finish(
        &mut self,
        pending: &Pending,
        answer: Result<(String, [u8; 16]), String>,
    ) -> Finish {
        let header = &pending.header;
        let (user, key) = match answer {
            Ok(signed_in) => signed_in,
            Err(why) => {
                eprintln!("smbd: refused a sign-in from {}: {why}", self.from);
                let response = self.refuse(header);
                if self.tries >= TRIES_MAX {
                    return Finish::End(response, "three wrong sign-ins".into());
                }
                return Finish::Again(response);
            }
        };
        let flags = auth::authenticate_flags(&pending.messages[2]);
        let server_mic = match (&self.mech_types, &pending.mic) {
            (Some(types), Some(mic)) => {
                if !auth::mic_is_right(&key, flags, types, mic) {
                    // The list of mechanisms was changed on the way.
                    let response = self.refuse(header);
                    return Finish::End(
                        response,
                        format!("a wrong mechListMIC signing in as {user}"),
                    );
                }
                Some(auth::server_mic(&key, flags, types))
            }
            _ => None,
        };
        let Some(negotiated) = self.negotiated.take() else {
            return Finish::End(self.refuse(header), "no NEGOTIATE".into());
        };
        let security = if self.spnego {
            auth::answer(State::Completed, false, None, server_mic.as_ref().map(|mic| &mic[..]))
        } else {
            Vec::new()
        };
        let keys = crypto::keys(&key, negotiated.dialect, &self.preauth, negotiated.cipher);
        let mut response =
            self.response(header, status::SUCCESS, &SignIn::setup_body(ENCRYPT_DATA, &security));
        crypto::sign(&mut response, &keys.signing, negotiated.signing);
        let handoff = Handoff {
            negotiated: *negotiated,
            session_id: self.session_id,
            next_message: self.next_message,
            keys,
            user,
            from: self.from.clone(),
        };
        Finish::Accepted(Box::new(handoff), response)
    }
}

/// Asks authd to check `pending`, sending it the pipe's read end, the
/// connection and the log: the account and the session key.
fn ask_authd(
    pending: &Pending,
    from: &str,
    pipe: &impl AsRawFd,
    connection: &TcpStream,
) -> Result<(String, [u8; 16]), String> {
    let [negotiate, challenge, authenticate] = &pending.messages;
    let request = Request::ShareSignIn {
        from: from.into(),
        negotiate: negotiate.clone(),
        challenge: challenge.clone(),
        authenticate: authenticate.clone(),
    };
    let unreachable = |err: io::Error| format!("cannot ask authd at {}: {err}", accounts::SOCKET);
    let stream = UnixStream::connect(accounts::SOCKET).map_err(unreachable)?;
    let fds = [pipe.as_raw_fd(), connection.as_raw_fd(), io::stderr().as_raw_fd()];
    sys::send_with_fds(&stream, &request.encode(), &fds).map_err(unreachable)?;
    stream.shutdown(Shutdown::Write).map_err(unreachable)?;
    let line = answer_of(&stream, AUTHD_TIME).map_err(unreachable)?;
    match Answer::decode(&line) {
        Answer::Done(text) => {
            let (user, key) = text.split_once(' ').ok_or("authd answered without a session key")?;
            let key: [u8; 16] = ntlm::unhex(key)
                .and_then(|key| key.try_into().ok())
                .ok_or("authd answered with a broken session key")?;
            Ok((user.to_string(), key))
        }
        Answer::Refused(why) => Err(why),
    }
}

/// authd's answer on `stream`, a line, which it waits at most `time` for.
fn answer_of(stream: &UnixStream, time: Duration) -> io::Result<String> {
    let until = Instant::now() + time;
    let mut answer = Vec::new();
    let mut buffer = [0; 512];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || !sys::readable(stream, left)? {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer in time"));
        }
        match (&*stream).read(&mut buffer)? {
            0 => break,
            read => answer.extend_from_slice(&buffer[..read]),
        }
        if answer.len() > 4096 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "an answer of more than 4 KiB"));
        }
    }
    String::from_utf8(answer)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "an answer not in UTF-8"))
}

/// The connection before sign-in, read with what is left of its time,
/// so that a PC that sends a byte now and then holds no place for long.
struct Timed<'a> {
    /// The connection.
    stream: &'a TcpStream,
    /// When its time is up.
    until: Instant,
}

impl Read for Timed<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no sign-in within a minute"));
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buffer)
    }
}

/// Serves one connection until it signed in or ended.
fn serve(stream: &TcpStream, from: SocketAddr, server_guid: [u8; 16]) -> Result<(), String> {
    let failed = |err: io::Error| err.to_string();
    stream.set_write_timeout(Some(SIGN_IN_TIME)).map_err(failed)?;
    let _ = stream.set_nodelay(true);
    let address = from.ip().to_canonical().to_string();
    let challenges = (0..TRIES_MAX)
        .map(|_| sys::random::<8>())
        .collect::<io::Result<Vec<_>>>()
        .map_err(failed)?;
    let salt = sys::random::<32>().map_err(failed)?;
    let session_id = u64::from_le_bytes(sys::random::<8>().map_err(failed)?);
    let mut sign_in =
        SignIn::new(address.clone(), server_guid, &sys::hostname(), salt, session_id, challenges);
    let mut timed = Timed { stream, until: Instant::now() + SIGN_IN_TIME };
    loop {
        // Frames are read whole, and no further: what comes after the
        // sign-in is the worker's to read.
        let Some(frame) = wire::read_frame_max(&mut timed, FRAME_MAX).map_err(failed)? else {
            return Ok(());
        };
        let pending = match sign_in.frame(&frame) {
            Step::Answer(response) if response.is_empty() => continue,
            Step::Answer(response) => {
                wire::write_frame(&mut &*stream, &response).map_err(failed)?;
                continue;
            }
            Step::End(response, why) => {
                if let Some(response) = response {
                    let _ = wire::write_frame(&mut &*stream, &response);
                }
                return Err(why.into());
            }
            Step::Check(pending) => pending,
        };
        let (read, write) = sys::pipe().map_err(failed)?;
        let asked = Instant::now();
        let answer = ask_authd(&pending, &address, &read, stream);
        drop(read);
        // authd's time, a wrong password's wait in it, is not the PC's.
        timed.until += asked.elapsed();
        match sign_in.finish(&pending, answer) {
            Finish::Again(response) => {
                wire::write_frame(&mut &*stream, &response).map_err(failed)?;
            }
            Finish::End(response, why) => {
                // The pipe's write end closes unused: a worker authd
                // started reads no keys, and ends.
                let _ = wire::write_frame(&mut &*stream, &response);
                return Err(why);
            }
            Finish::Accepted(handoff, response) => {
                handoff
                    .send(&mut File::from(write))
                    .map_err(|err| format!("cannot hand the session over: {err}"))?;
                wire::write_frame(&mut &*stream, &response).map_err(failed)?;
                eprintln!("smbd: {} signed in from {address}", handoff.user);
                return Ok(());
            }
        }
    }
}

/// Takes connections from `listener`, a thread each, while fewer than
/// [`CONNECTIONS_MAX`] are signing in.
fn accept(listener: &TcpListener, server_guid: [u8; 16], active: &Arc<AtomicUsize>) {
    loop {
        let (stream, from) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(err) => {
                eprintln!("smbd: cannot accept a connection: {err}");
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        if active.fetch_add(1, Ordering::SeqCst) >= CONNECTIONS_MAX {
            active.fetch_sub(1, Ordering::SeqCst);
            eprintln!("smbd: refused a connection from {from}: {CONNECTIONS_MAX} are signing in");
            continue;
        }
        let active = Arc::clone(active);
        let spawned = thread::Builder::new().name("sign-in".into()).spawn(move || {
            if let Err(why) = serve(&stream, from, server_guid) {
                eprintln!("smbd: a connection from {} ended: {why}", from.ip().to_canonical());
            }
            active.fetch_sub(1, Ordering::SeqCst);
        });
        if let Err(err) = spawned {
            eprintln!("smbd: cannot start a thread for a connection: {err}");
        }
    }
}

/// Waits for SIGTERM, which Spark sends to stop smbd, then has authd end
/// every share session, as `share off` does, and exits.
fn on_stop() {
    // SAFETY: an empty set, then SIGTERM added, then waited for; every
    // thread blocks SIGTERM, so this one takes it.
    let signal = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        let mut signal = 0;
        libc::sigwait(&raw const set, &raw mut signal);
        signal
    };
    if signal == libc::SIGTERM {
        match end_shares() {
            Ok(text) => eprintln!("smbd: stopping; share sessions ended: {text}"),
            Err(err) => eprintln!("smbd: stopping; cannot end the share sessions: {err}"),
        }
    }
    std::process::exit(0);
}

/// Has authd end every share session: how many it ended.
pub fn end_shares() -> io::Result<String> {
    let stream = UnixStream::connect(accounts::SOCKET)?;
    (&stream).write_all(&Request::EndShares.encode())?;
    stream.shutdown(Shutdown::Write)?;
    let line = answer_of(&stream, Duration::from_secs(10))?;
    match Answer::decode(&line) {
        Answer::Done(text) => Ok(text),
        Answer::Refused(why) => Err(io::Error::other(why)),
    }
}

/// smbd: listens on port 445 and signs PCs in.
pub fn run() -> ExitCode {
    // SIGTERM is taken by one thread, with sigwait, so every thread
    // blocks it, those started after this among them.
    // SAFETY: a set with SIGTERM, blocked for this thread and those it
    // starts.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
    }
    let server_guid = match sys::random::<16>() {
        Ok(guid) => guid,
        Err(err) => {
            eprintln!("smbd: cannot read random bytes: {err}");
            return ExitCode::FAILURE;
        }
    };
    let ipv4 = match TcpListener::bind(("0.0.0.0", PORT)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("smbd: cannot listen on port {PORT}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let active = Arc::new(AtomicUsize::new(0));
    if let Ok(ipv6) = TcpListener::bind(("::", PORT)) {
        let active = Arc::clone(&active);
        let _ = thread::Builder::new()
            .name("listen-ipv6".into())
            .spawn(move || accept(&ipv6, server_guid, &active));
    }
    if let Err(err) = thread::Builder::new().name("stop".into()).spawn(on_stop) {
        eprintln!("smbd: cannot start the thread that waits for SIGTERM: {err}");
    }
    eprintln!("smbd: listening on port {PORT}");
    accept(&ipv4, server_guid, &active);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preauthentication_hash_is_smb2s() {
        let mut hasher = smb2::crypto::kdf::PreauthHasher::new();
        hasher.update(b"negotiate request");
        hasher.update(b"negotiate response");
        let mut ours = crypto::preauth(&[0; 64], b"negotiate request");
        ours = crypto::preauth(&ours, b"negotiate response");
        assert_eq!(&ours, hasher.value());
    }

    #[test]
    fn authds_answer_is_waited_for_and_no_longer() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        (&theirs).write_all(b"ok 3\n").unwrap();
        drop(theirs);
        assert_eq!(answer_of(&ours, Duration::from_secs(5)).unwrap(), "ok 3\n");
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let silent = answer_of(&ours, Duration::from_millis(200)).unwrap_err();
        assert_eq!(silent.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn a_sign_in_has_a_minute_in_all() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _pc = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut timed =
            Timed { stream: &stream, until: Instant::now() + Duration::from_millis(100) };
        assert!(wire::read_frame_max(&mut timed, FRAME_MAX).is_err());
        let mut late = Timed { stream: &stream, until: Instant::now() };
        assert_eq!(late.read(&mut [0; 4]).unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn smb1_and_garbage_end_the_connection() {
        let mut sign_in = SignIn::new("192.0.2.7".into(), [1; 16], "box", [2; 32], 9, vec![[3; 8]]);
        assert!(matches!(sign_in.frame(b"GET / HTTP/1.1\r\n"), Step::End(None, _)));
        let mut smb1 = vec![0xff, b'S', b'M', b'B', 0x72];
        smb1.resize(32, 0);
        smb1.push(0);
        smb1.extend_from_slice(&11u16.to_le_bytes());
        smb1.extend_from_slice(b"\x02NT LM 0.12\0");
        assert!(matches!(sign_in.frame(&smb1), Step::End(None, _)));
    }
}
