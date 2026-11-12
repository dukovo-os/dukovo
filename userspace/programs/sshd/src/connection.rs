// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One SSH connection, as `sshd --connection` serves it: the listener
//! starts one for each client, with the connection as its standard input
//! and a pipe as its standard output, on which it says once someone
//! signed in.
//!
//! russh speaks the protocol (RFC 4253, 4252 and 4254). A person signs
//! in with a key their `~/.ssh/authorized_keys` lists: sshd asks authd(8)
//! whether a key may sign them in before the client signs with it, and
//! hands authd what the client signed, and the signature, to start each
//! session, which authd checks again itself (ADR 0347). Once
//! sshd_config(5) lets passwords sign in, authd checks one and answers
//! with a ticket for this connection, which starts the sessions in its
//! stead, and which goes when the connection ends (ADR 0348). The
//! settings say who may sign in at all. A connection has two minutes to
//! sign in, six tries, and four sessions at most, each a shell, a
//! command or SFTP.
//!
//! Once someone signed in, their client may forward ports, as the
//! settings let it (ADR 0349): sshd has the connection's forwarder, which
//! authd starts as them, make the connections and listen on the ports
//! ([`forward`]).

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::os::fd::FromRawFd;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::Instant;

use accounts::forwarding::HOST_MAX;
use accounts::request::{
    self, Answer, Request, SSH_COMMAND_MAX, SSH_KEY_ALGORITHMS, SshProof, SshRun,
};
use accounts::sshd_config::{self, Settings};
use russh::keys::ssh_key::HashAlg;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{
    self, Auth, ChannelOpenHandle, Handle, Msg, PublicKeyOffer, Session, VerifiedPublicKey,
};
use russh::{
    Channel, ChannelId, ChannelOpenFailure, Disconnect, MethodKind, MethodSet, Preferred, SshId,
    kex,
};

use crate::forward::Forwarding;
use crate::session::{self, SignedIn, WINDOW};
use crate::{authd, hostkey, pty};

/// How long a client has to sign in.
const SIGN_IN_TIME: Duration = Duration::from_secs(120);
/// How many keys a client may try to sign in with, as OpenSSH's
/// MaxAuthTries.
const TRIES_MAX: usize = 6;
/// The most sessions on one connection at once.
const CHANNELS_MAX: usize = 4;
/// How long a signed-in connection may be idle: clients that keep it
/// alive are kept.
const IDLE_TIME: Duration = Duration::from_hours(1);
/// How often an idle client is asked whether it is still there, and how
/// many times it may not answer.
const KEEPALIVE_TIME: Duration = Duration::from_secs(60);
const KEEPALIVES_MAX: usize = 3;
/// The longest account name and terminal type taken.
const NAME_MAX: usize = 256;
const TERM_MAX: usize = 64;
/// The most language settings one session takes.
const SETTINGS_MAX: usize = 16;
/// How long the forwarder may take to listen on a port.
const LISTEN_TIME: Duration = Duration::from_secs(10);
/// The ports below this are root's to listen on, as on Linux.
const PORTS_FROM: u16 = 1024;

/// The key exchanges sshd offers: post-quantum hybrid first, then
/// Curve25519, which every OpenSSH since 6.5 speaks. The finite-field
/// Diffie-Hellman groups are left out: they cost the server more than
/// the client.
const KEX: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::EXTENSION_SUPPORT_AS_SERVER,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
];

/// What goes wrong with a connection.
#[derive(Debug)]
pub enum Error {
    Ssh(russh::Error),
    /// The client sent more than its window let it.
    Overrun,
}

impl From<russh::Error> for Error {
    fn from(err: russh::Error) -> Self {
        Error::Ssh(err)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Ssh(err) => write!(f, "{err}"),
            Error::Overrun => f.write_str("the client sent more than its window let it"),
        }
    }
}

/// A connection's state.
struct Connection {
    /// What sshd_config(5) said as the listener started it.
    settings: Settings,
    /// Where the client is, for the log.
    from: String,
    /// The connection as SSH_CONNECTION says it.
    connection: String,
    /// The pipe on which the listener hears that someone signed in.
    notify: Option<File>,
    /// What tells the sign-in guard so.
    guard: Option<oneshot::Sender<()>>,
    /// Who signed in.
    signed_in: Option<SignedIn>,
    /// Why the last key was refused, for the log.
    refused: Option<String>,
    /// The session channels.
    channels: HashMap<ChannelId, session::Channel>,
    /// The forwarding, once the client forwarded a port.
    forwarding: Option<Forwarding>,
}

/// `sshd --connection`: serves the connection on standard input, with
/// the `settings` the listener read.
pub fn run(settings: Settings) -> ExitCode {
    // SAFETY: the listener gave this process the connection as standard
    // input, and the pipe as standard output, which nothing else uses.
    let (socket, notify) = unsafe { (TcpStream::from_raw_fd(0), File::from_raw_fd(1)) };
    let (peer, local) = match (socket.peer_addr(), socket.local_addr()) {
        (Ok(peer), Ok(local)) => (canonical(peer), canonical(local)),
        _ => {
            eprintln!("sshd: standard input is not a connection");
            return ExitCode::FAILURE;
        }
    };
    let from = format!("{} port {}", peer.ip(), peer.port());
    let key = match hostkey::load() {
        Ok(key) => key,
        Err(why) => {
            eprintln!("sshd: {why}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("sshd: cannot start: {err}");
            return ExitCode::FAILURE;
        }
    };
    let (guard, signed_in) = oneshot::channel();
    let connection = Connection {
        settings,
        connection: format!("{} {} {} {}", peer.ip(), peer.port(), local.ip(), local.port()),
        from: from.clone(),
        notify: Some(notify),
        guard: Some(guard),
        signed_in: None,
        refused: None,
        channels: HashMap::new(),
        forwarding: None,
    };
    match runtime.block_on(serve(socket, key, connection, signed_in)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("sshd: the connection from {from} ended: {why}");
            ExitCode::FAILURE
        }
    }
}

/// An address as people write it: an IPv4 one an IPv6 socket took is
/// shown as IPv4.
fn canonical(address: SocketAddr) -> SocketAddr {
    SocketAddr::new(address.ip().to_canonical(), address.port())
}

/// Serves the connection on `socket` with the host key `key`; it ends
/// if `signed_in` does not say someone signed in within
/// [`SIGN_IN_TIME`].
async fn serve(
    socket: TcpStream,
    key: PrivateKey,
    connection: Connection,
    signed_in: oneshot::Receiver<()>,
) -> Result<(), String> {
    let deadline = Instant::now() + SIGN_IN_TIME;
    socket.set_nonblocking(true).map_err(|err| err.to_string())?;
    let socket = tokio::net::TcpStream::from_std(socket).map_err(|err| err.to_string())?;
    let mut methods = Vec::new();
    if connection.settings.keys {
        methods.push(MethodKind::PublicKey);
    }
    if connection.settings.passwords {
        methods.push(MethodKind::Password);
    }
    let config = Arc::new(server::Config {
        server_id: SshId::Standard("SSH-2.0-Dukovo".into()),
        methods: MethodSet::from(methods.as_slice()),
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        window_size: WINDOW,
        preferred: Preferred { kex: KEX.into(), ..Preferred::default() },
        max_auth_attempts: TRIES_MAX,
        inactivity_timeout: Some(IDLE_TIME),
        keepalive_interval: Some(KEEPALIVE_TIME),
        keepalive_max: KEEPALIVES_MAX,
        nodelay: true,
        ..server::Config::default()
    });
    let started = tokio::time::timeout_at(deadline, server::run_stream(config, socket, connection));
    let running = match started.await {
        Ok(Ok(running)) => running,
        Ok(Err(Error::Ssh(russh::Error::IO(_) | russh::Error::Disconnect))) => {
            return Err("the client closed it before it said which SSH it speaks".into());
        }
        Ok(Err(err)) => return Err(err.to_string()),
        Err(_) => return Err("the client did not say which SSH it speaks in time".into()),
    };
    // A client that has not signed in by the deadline is let go.
    let handle = running.handle();
    let guard = tokio::spawn(async move {
        if tokio::time::timeout_at(deadline, signed_in).await.is_err() {
            let reason = "no one signed in in time".to_string();
            let _ = handle.disconnect(Disconnect::ByApplication, reason, String::new()).await;
        }
    });
    let ended = running.await;
    guard.abort();
    match ended {
        Ok(()) => Ok(()),
        // A client that goes away ends its connection as it may.
        Err(Error::Ssh(russh::Error::IO(_) | russh::Error::Disconnect)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// Whether `text` is printable ASCII, at most `max` long, and not empty.
fn printable(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether `name` may name an account: authd checks the rest.
fn account_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= NAME_MAX && !name.contains(['\0', '\n'])
}

/// Whether the settings let `name` sign in: AllowUsers and the others.
fn admitted(settings: &Settings, name: &str) -> Result<(), String> {
    let groups = if settings.checks_groups() { groups_of(name)? } else { Vec::new() };
    settings.admits(name, &groups)
}

/// The names of the groups of `name`'s account, none for a name no
/// account has.
fn groups_of(name: &str) -> Result<Vec<String>, String> {
    let read = |path: &str| {
        std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
    };
    let users = accounts::users(&read(accounts::PASSWD)?);
    let groups = accounts::groups(&read(accounts::GROUP)?);
    Ok(users
        .entries()
        .find(|user| user.name == name)
        .map_or_default(|user| sshd_config::group_names(user, &groups)))
}

impl Connection {
    /// Someone signed in as `name`, as `proof` shows: the listener counts
    /// the connection as signed in, and its sign-in guard stands down.
    fn sign_in(&mut self, name: &str, proof: SshProof, how: &str) {
        eprintln!("sshd: {name} signed in from {} with {how}", self.from);
        self.signed_in = Some(SignedIn { name: name.into(), proof });
        self.refused = None;
        if let Some(mut notify) = self.notify.take() {
            let _ = notify.write_all(b"in\n");
        }
        if let Some(guard) = self.guard.take() {
            let _ = guard.send(());
        }
    }

    /// The connection's forwarding, its forwarder started through authd
    /// the first time; `None`, and why in the log, if it cannot be.
    async fn forwarding(&mut self, handle: Handle) -> Option<&Forwarding> {
        let who = self.signed_in.as_ref()?;
        match &self.forwarding {
            Some(forwarding) if forwarding.alive() => {}
            // It ended, and said why: the connection forwards no more.
            Some(_) => return None,
            None => match Forwarding::start(handle, who, &self.connection, &self.from).await {
                Ok(forwarding) => {
                    eprintln!(
                        "sshd: started forwarding for {} from {}, process {}",
                        who.name, self.from, forwarding.process
                    );
                    self.forwarding = Some(forwarding);
                }
                Err(why) => {
                    eprintln!("sshd: cannot forward for {} from {}: {why}", who.name, self.from);
                    return None;
                }
            },
        }
        self.forwarding.as_ref()
    }

    /// Who signed in, if anyone, and the settings let them forward
    /// `which` way: if not, why is logged.
    fn may_forward(&self, which: &str, allowed: bool) -> Option<String> {
        let who = self.signed_in.as_ref()?;
        if !allowed {
            eprintln!(
                "sshd: refused to forward {which} for {} from {}: AllowTcpForwarding",
                who.name, self.from
            );
            return None;
        }
        Some(who.name.clone())
    }

    /// The channel `id`, if it is one of ours and its session has not
    /// started.
    fn unstarted(&mut self, id: ChannelId) -> Option<&mut session::Channel> {
        self.channels.get_mut(&id).filter(|channel| channel.running.is_none())
    }

    /// Starts the session of channel `id`, running what the client asked,
    /// `run`.
    async fn start(
        &mut self,
        id: ChannelId,
        run: SshRun,
        session: &mut Session,
    ) -> Result<(), Error> {
        let handle = session.handle();
        let started = match (&self.signed_in, self.channels.get_mut(&id)) {
            (Some(who), Some(channel)) if channel.running.is_none() => {
                let what = match run {
                    SshRun::Shell => "a shell",
                    SshRun::Command(_) => "a command",
                    SshRun::Sftp => "SFTP",
                    SshRun::Forward => "forwarding",
                };
                session::start(channel, id, handle, who, &self.connection, run)
                    .await
                    .map(|process| (who.name.clone(), what, process))
            }
            _ => Err("the channel's session has started already".into()),
        };
        match started {
            Ok((name, what, process)) => {
                eprintln!("sshd: started {what} for {name} from {}, process {process}", self.from);
                session.channel_success(id)?;
            }
            Err(why) => {
                eprintln!("sshd: cannot start a session for {}: {why}", self.from);
                session.channel_failure(id)?;
            }
        }
        Ok(())
    }
}

/// Whether a key of `algorithm` may sign anyone in: those authd checks
/// signatures of.
fn key_algorithm(algorithm: &Algorithm) -> bool {
    SSH_KEY_ALGORITHMS.contains(&algorithm.as_str())
}

impl server::Handler for Connection {
    type Error = Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Error> {
        Ok(Auth::reject())
    }

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        offer: PublicKeyOffer<'_>,
    ) -> Result<Auth, Error> {
        if !self.settings.keys {
            return Ok(Auth::reject());
        }
        if !account_name(user) {
            self.refused = Some("a name no account has".into());
            return Ok(Auth::reject());
        }
        if let Err(why) = admitted(&self.settings, user) {
            self.refused = Some(format!("{user}: {why}"));
            return Ok(Auth::reject());
        }
        // The algorithm the client signs with must be the key's own:
        // an RSA key signed with SHA-2 is not taken, as authd checks no
        // RSA signatures.
        if !key_algorithm(offer.algorithm)
            || offer.algorithm.as_str() != offer.public_key.algorithm().as_str()
        {
            self.refused = Some(format!("{} keys are not taken", offer.algorithm.as_str()));
            return Ok(Auth::reject());
        }
        let key = offer.public_key.to_bytes().map_err(|_| russh::Error::Inconsistent)?;
        let request = Request::SshKey { name: user.into(), key };
        let fingerprint = offer.public_key.fingerprint(HashAlg::Sha256);
        match authd::ask(&request, &[]).await {
            Ok((Answer::Done(_), _)) => Ok(Auth::Accept),
            Ok((Answer::Refused(why), _)) => {
                self.refused = Some(format!("key {fingerprint} for {user}: {why}"));
                Ok(Auth::reject())
            }
            Err(why) => {
                eprintln!("sshd: {why}");
                self.refused = Some(why);
                Ok(Auth::reject())
            }
        }
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Error> {
        // Checked again, as a client may sign without asking first.
        if !self.settings.keys
            || !account_name(user)
            || self.signed_in.is_some()
            || admitted(&self.settings, user).is_err()
        {
            return Ok(Auth::reject());
        }
        let fingerprint = key.public_key.fingerprint(HashAlg::Sha256).to_string();
        let proof =
            SshProof::Key { signed: key.signed.to_vec(), signature: key.signature.to_vec() };
        self.sign_in(user, proof, &format!("key {fingerprint}"));
        Ok(Auth::Accept)
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Error> {
        if !self.settings.passwords || !account_name(user) || self.signed_in.is_some() {
            return Ok(Auth::reject());
        }
        if let Err(why) = admitted(&self.settings, user) {
            self.refused = Some(format!("{user}: {why}"));
            return Ok(Auth::reject());
        }
        let request = Request::SshPassword {
            connection: self.connection.clone(),
            name: user.into(),
            password: password.as_bytes().to_vec(),
        };
        let answer = authd::ask(&request, &[]).await;
        // The request's password is wiped as it goes.
        drop(request);
        match answer {
            Ok((Answer::Done(ticket), _)) => match request::unhex(&ticket) {
                Some(ticket) if ticket.len() == request::SSH_TICKET_LENGTH => {
                    self.sign_in(user, SshProof::Ticket(ticket), "a password");
                    Ok(Auth::Accept)
                }
                _ => {
                    eprintln!("sshd: authd answered a password with no ticket");
                    Ok(Auth::reject())
                }
            },
            Ok((Answer::Refused(why), _)) => {
                self.refused = Some(format!("a password for {user}: {why}"));
                Ok(Auth::reject())
            }
            Err(why) => {
                eprintln!("sshd: {why}");
                self.refused = Some(why);
                Ok(Auth::reject())
            }
        }
    }

    fn adjusts_window(&self) -> bool {
        true
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Error> {
        // Dropping the reply refuses the channel.
        if self.may_forward("a connection", self.settings.forwards_local()).is_none() {
            return Ok(());
        }
        let Ok(port) = u16::try_from(port) else { return Ok(()) };
        if !printable(host, HOST_MAX) {
            return Ok(());
        }
        match self.forwarding(session.handle()).await {
            Some(forwarding) => forwarding.connect(channel, reply, host.into(), port),
            None => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Error> {
        let Some(name) = self.may_forward("a port", self.settings.forwards_remote()) else {
            return Ok(false);
        };
        let Ok(asked) = u16::try_from(*port) else { return Ok(false) };
        if address.len() > HOST_MAX || address.chars().any(char::is_control) {
            return Ok(false);
        }
        // As OpenSSH's sshd, for anyone but root, whom it does not sign in.
        if asked != 0 && asked < PORTS_FROM {
            eprintln!(
                "sshd: refused to forward port {asked} for {name} from {}: the ports below \
                 {PORTS_FROM} are root's",
                self.from
            );
            return Ok(false);
        }
        let listening_at = self.settings.listen_address(address);
        let from = self.from.clone();
        let Some(forwarding) = self.forwarding(session.handle()).await else { return Ok(false) };
        let listened = forwarding.listen(address.into(), listening_at.clone(), asked);
        match tokio::time::timeout(LISTEN_TIME, listened).await {
            Ok(Ok(listening)) => {
                let at = if listening_at.is_empty() { "every address" } else { &listening_at };
                eprintln!("sshd: forwarding port {listening} at {at} for {name} from {from}");
                if asked == 0 {
                    *port = u32::from(listening);
                }
                Ok(true)
            }
            Ok(Err(why)) => {
                eprintln!("sshd: cannot forward port {asked} for {name} from {from}: {why}");
                Ok(false)
            }
            Err(_) => {
                eprintln!(
                    "sshd: cannot forward port {asked} for {name} from {from}: the forwarder did \
                     not listen in {} seconds",
                    LISTEN_TIME.as_secs()
                );
                Ok(false)
            }
        }
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        _session: &mut Session,
    ) -> Result<bool, Error> {
        match &self.forwarding {
            Some(forwarding) if forwarding.alive() => {
                Ok(forwarding.unlisten(address.into(), port).await)
            }
            _ => Ok(false),
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Error> {
        // sshd takes the channel's traffic in the calls below, not from
        // the channel's own queue, which would hold the connection up.
        let id = channel.id();
        drop(channel);
        // Sessions that ended free their places: sshd closed their
        // channels, and the client's answer does not come back here.
        self.channels.retain(|_, open| !open.running.as_ref().is_some_and(session::Running::ended));
        if self.channels.len() >= CHANNELS_MAX {
            eprintln!("sshd: refused a session for {}: {CHANNELS_MAX} are open", self.from);
            // Dropping the reply refuses the channel.
            return Ok(());
        }
        self.channels.insert(id, session::Channel::default());
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        id: ChannelId,
        term: &str,
        columns: u32,
        rows: u32,
        width: u32,
        height: u32,
        modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Error> {
        // An empty type is a client's with no TERM of its own, as ssh -tt
        // from cron or a script: OpenSSH's sshd gives it a terminal too,
        // and authd makes it a dumb one.
        match self.unstarted(id) {
            Some(channel)
                if channel.pty.is_none() && (term.is_empty() || printable(term, TERM_MAX)) =>
            {
                channel.pty = Some(pty::Asked {
                    term: term.into(),
                    size: pty::Size { columns, rows, width, height },
                    modes: modes.to_vec(),
                });
                session.channel_success(id)?;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }

    async fn env_request(
        &mut self,
        id: ChannelId,
        name: &str,
        value: &str,
        session: &mut Session,
    ) -> Result<(), Error> {
        let setting = format!("{name}={value}");
        let named = format!("{name}=");
        match self.unstarted(id) {
            // As Linux's sshd, which passes LANG and LC_* on (AcceptEnv).
            Some(channel)
                if request::ssh_language(&setting)
                    && (channel.language.len() < SETTINGS_MAX
                        || channel.language.iter().any(|s| s.starts_with(&named))) =>
            {
                channel.language.retain(|s| !s.starts_with(&named));
                channel.language.push(setting);
                session.channel_success(id)?;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }

    async fn shell_request(&mut self, id: ChannelId, session: &mut Session) -> Result<(), Error> {
        self.start(id, SshRun::Shell, session).await
    }

    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Error> {
        if command.len() > SSH_COMMAND_MAX || command.contains(&0) {
            eprintln!("sshd: refused a command from {}: too long, or with a NUL in it", self.from);
            session.channel_failure(id)?;
            return Ok(());
        }
        self.start(id, SshRun::Command(command.to_vec()), session).await
    }

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Error> {
        // SFTP, which scp uses too: OpenSSH's sftp-server, as the person.
        if name == "sftp" {
            return self.start(id, SshRun::Sftp, session).await;
        }
        eprintln!("sshd: refused the subsystem {name:?} for {}: SFTP is the only one", self.from);
        session.channel_failure(id)?;
        Ok(())
    }

    async fn x11_request(
        &mut self,
        id: ChannelId,
        _single: bool,
        _protocol: &str,
        _cookie: &str,
        _screen: u32,
        session: &mut Session,
    ) -> Result<(), Error> {
        session.channel_failure(id)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        id: ChannelId,
        columns: u32,
        rows: u32,
        width: u32,
        height: u32,
        _session: &mut Session,
    ) -> Result<(), Error> {
        let size = pty::Size { columns, rows, width, height };
        if let Some(channel) = self.channels.get_mut(&id) {
            match (&channel.running, &mut channel.pty) {
                (Some(running), _) => running.resize(size),
                (None, Some(asked)) => asked.size = size,
                (None, None) => {}
            }
        }
        Ok(())
    }

    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Error> {
        match self.channels.get_mut(&id).map(|channel| channel.running.as_mut()) {
            Some(Some(running)) => running.input(data).map_err(|_| Error::Overrun),
            // Before the session starts, input is passed over.
            Some(None) => {
                session::give_room(&session.handle(), id, data.len());
                Ok(())
            }
            // A forwarded connection's channel, whose bytes its own task
            // passes on (forward.rs).
            None => Ok(()),
        }
    }

    async fn extended_data(
        &mut self,
        id: ChannelId,
        _code: u32,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Error> {
        // A client's extended data means nothing to a session.
        session::give_room(&session.handle(), id, data.len());
        Ok(())
    }

    async fn channel_eof(&mut self, id: ChannelId, _session: &mut Session) -> Result<(), Error> {
        if let Some(running) = self.channels.get_mut(&id).and_then(|c| c.running.as_mut()) {
            running.end_input();
        }
        Ok(())
    }

    async fn channel_close(&mut self, id: ChannelId, _session: &mut Session) -> Result<(), Error> {
        // What passes the session's bytes stops, and its terminal or
        // pipes close.
        self.channels.remove(&id);
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.signed_in.is_none()
            && let Some(why) = &self.refused
        {
            eprintln!("sshd: no one signed in from {}: {why}", self.from);
        }
        // A password's ticket holds no longer than its connection.
        if let Some(SignedIn { proof: SshProof::Ticket(ticket), .. }) = self.signed_in.take() {
            authd::tell(&Request::SshEnded { ticket });
        }
    }
}
