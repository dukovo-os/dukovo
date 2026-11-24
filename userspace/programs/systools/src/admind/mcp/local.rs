// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MCP server's socket on this machine (ADR 0690): the way in for a
//! client that connects over SSH.
//!
//! Such a client's settings run `ssh OWNER@HOST admin mcp NAME`. `admin
//! mcp`, as the owner, asks authd for a pass for that client, connects
//! here, and carries the client's messages to this socket and the
//! answers back (admin(8)). Nothing listens on the network for it: what
//! lets the client in is its owner's own SSH sign-in.
//!
//! The socket is `/system/live/admind/mcp`, which any account may
//! connect to. A connection gives its pass as its first line, within 5
//! seconds, and is answered in one line:
//!
//! ```text
//! dkp_OWNER_SECRET
//! ok OWNER NAME          or   no ADM-NNN: words
//! ```
//!
//! After `ok` it sends one JSON-RPC message a line, of at most 64 KiB,
//! and reads one answer a line, in whatever order they are ready; a
//! notification has no answer. A line that is longer is answered with an
//! error and ends the connection, since what follows it cannot be told
//! from the rest of it.
//!
//! A pass is taken from the account it was given to and from no other:
//! the kernel says who connected, and that must be the client's owner.
//! authd is asked about the pass again with every message, so a client
//! that is removed, or a pass that has ended, stops at its next message:
//! the connection is closed, and `admin mcp` asks authd for a new pass.
//!
//! At most 32 connections are open, 8 of one account, so that one
//! account cannot keep the others out; and 8 messages of one connection
//! are being answered at a time, the next waiting to be read. An account
//! that brings 10 passes in a minute that are not taken waits the minute
//! out before authd is asked about another of its passes: `admin mcp`
//! brings only passes authd has just given it, so nothing that works is
//! held back.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, mpsc};

use super::super::caller::{TOKEN_MAX, Token};
use super::super::errors::{Failure, Kind};
use super::super::limits::{TRIES_MAX, TRIES_WINDOW};
use super::super::log;
use super::super::shared::Shared;
use super::super::who::{self, Account};
use super::client::{self, Client, Door, PASS_PREFIX};
use super::rpc::{self, MESSAGE_MAX};
use super::server;
use crate::util;

/// The socket.
pub const PATH: &str = "/system/live/admind/mcp";

/// The most connections open at once.
pub const CONNECTIONS_MAX: usize = 32;
/// The most of one account.
pub const CONNECTIONS_EACH: usize = 8;
/// The most messages of one connection that are being answered at once.
const AT_ONCE: usize = 8;
/// How long a connection has to give its pass.
const PASS_TIME: Duration = Duration::from_secs(5);
/// How long an answer may wait to be read before the connection is
/// closed: far longer than `admin mcp` takes.
const WRITE_TIME: Duration = Duration::from_secs(30);

/// What the socket counts, by the account that connected.
#[derive(Default)]
struct Open {
    /// The connections open.
    connections: RefCell<HashMap<u32, usize>>,
    /// The passes brought that were not taken: when the first of them
    /// came, and how many came in the minute since.
    wrong: RefCell<HashMap<u32, (Instant, u32)>>,
}

impl Open {
    /// The seconds `user` waits at `now` before authd is asked about
    /// another pass of its, when it has brought [`TRIES_MAX`] that were
    /// not taken in the minute since the first.
    fn waits(&self, user: u32, now: Instant) -> Option<u64> {
        let mut wrong = self.wrong.borrow_mut();
        let &(since, count) = wrong.get(&user)?;
        let gone = now.saturating_duration_since(since);
        if gone >= TRIES_WINDOW {
            wrong.remove(&user);
            return None;
        }
        (count >= TRIES_MAX).then(|| TRIES_WINDOW.saturating_sub(gone).as_secs() + 1)
    }

    /// Counts a pass of `user`'s that was not taken at `now`.
    fn wrong(&self, user: u32, now: Instant) {
        let mut wrong = self.wrong.borrow_mut();
        let (since, count) = wrong.entry(user).or_insert((now, 0));
        if now.saturating_duration_since(*since) >= TRIES_WINDOW {
            (*since, *count) = (now, 0);
        }
        *count = count.saturating_add(1);
    }
}

/// One connection's place among those open, given back when it goes.
struct Place {
    open: Rc<Open>,
    user: u32,
}

impl Place {
    /// A place for a connection of `user`, unless as many are open as
    /// may be, of everyone or of that account.
    fn take(open: &Rc<Open>, user: u32) -> Option<Place> {
        let mut counts = open.connections.borrow_mut();
        let all: usize = counts.values().sum();
        let own = counts.get(&user).copied().unwrap_or(0);
        if all >= CONNECTIONS_MAX || own >= CONNECTIONS_EACH {
            return None;
        }
        counts.insert(user, own + 1);
        Some(Place { open: Rc::clone(open), user })
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut counts = self.open.connections.borrow_mut();
        if let Some(count) = counts.get_mut(&self.user) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.user);
            }
        }
    }
}

/// Listens at `path`, which any account may connect to: admind checks
/// who asks.
///
/// # Errors
///
/// If the socket cannot be made.
pub fn listen(path: &Path) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    let open = |path: &Path, mode| std::fs::set_permissions(path, PermissionsExt::from_mode(mode));
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
        open(folder, 0o755)?;
    }
    // One left by an admind that did not stop in order.
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    open(path, 0o666)?;
    listener.set_nonblocking(true)?;
    UnixListener::from_std(listener)
}

/// Accepts connections on `socket` as long as admind runs.
pub async fn accepting(shared: Rc<Shared>, socket: UnixListener) {
    let open = Rc::new(Open::default());
    loop {
        match socket.accept().await {
            Ok((stream, _)) => {
                tokio::task::spawn_local(connection(Rc::clone(&shared), stream, Rc::clone(&open)));
            }
            Err(err) => {
                let why = util::strip_os_error(&err);
                log(4, &format!("cannot accept a connection of an AI client's: {why}"));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// The user who connected `stream`, as the kernel says.
#[cfg(target_os = "radix")]
fn user_of(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let slot = crate::sys::slot_of(stream.as_raw_fd());
    radix_abi::file::socket_state(&crate::sys::SYS, slot).ok().map(|state| state.user)
}

/// The user who connected `stream`, as the build machine's kernel says:
/// the unit tests run there.
#[cfg(not(target_os = "radix"))]
fn user_of(stream: &UnixStream) -> Option<u32> {
    stream.peer_cred().ok().map(|peer| peer.uid())
}

/// A line of at most `limit` bytes from `reader`, without its end;
/// `None` when the other end has closed with nothing more to say.
///
/// # Errors
///
/// `InvalidData` for a line that is longer; `UnexpectedEof` for one the
/// other end left unfinished; the socket's own otherwise.
async fn line(reader: &mut BufReader<OwnedReadHalf>, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    // One byte more than a line may have, for its end.
    let most = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let read = (&mut *reader).take(most).read_until(b'\n', &mut line).await?;
    if read == 0 {
        return Ok(None);
    }
    if line.pop() != Some(b'\n') {
        let kind =
            if read > limit { io::ErrorKind::InvalidData } else { io::ErrorKind::UnexpectedEof };
        return Err(io::Error::from(kind));
    }
    Ok(Some(line))
}

/// Writes `words` and a line end to `writer`, if the other end reads
/// them in time.
async fn say(writer: &mut OwnedWriteHalf, words: &[u8]) -> bool {
    let written = async {
        writer.write_all(words).await?;
        writer.write_all(b"\n").await
    };
    matches!(tokio::time::timeout(WRITE_TIME, written).await, Ok(Ok(())))
}

/// The refusal that a connection is told in its one line, `no ID:
/// words`.
fn no(failure: &Failure) -> Vec<u8> {
    let id = failure.own_id.as_deref().unwrap_or(failure.kind.id());
    format!("no {id}: {}", failure.detail).into_bytes()
}

/// Checks that `client`'s owner is the account of the user number
/// `user`, who connected.
///
/// # Errors
///
/// ADM-143 for a pass that another account brings.
fn owned_by(shared: &Shared, client: &Client, user: u32) -> Result<(), Failure> {
    let asker = who::read(&shared.accounts, Account::Number(user));
    if asker.is_ok_and(|asker| asker.name == client.owner) {
        return Ok(());
    }
    let why = format!("this pass is {}'s, and is taken from that account alone", client.owner);
    Err(Failure::new(Kind::ClientRefused, why))
}

/// The client whose pass `stream` gives as its first line.
///
/// # Errors
///
/// ADM-143 for no pass in time, one that is not a pass, one authd does
/// not take, or one of another account's; ADM-111 when authd does not
/// answer.
async fn greeted(
    shared: &Shared,
    reader: &mut BufReader<OwnedReadHalf>,
    user: u32,
) -> Result<(Client, Token), Failure> {
    let refused = |why: &str| Failure::new(Kind::ClientRefused, why);
    let pass = match tokio::time::timeout(PASS_TIME, line(reader, TOKEN_MAX)).await {
        Ok(Ok(Some(pass))) => Token::new(pass),
        Ok(_) => return Err(refused("the first line is the AI client's pass, dkp_…")),
        Err(_) => return Err(refused("no pass came in 5 seconds")),
    };
    let formed =
        pass.bytes().starts_with(PASS_PREFIX) && pass.bytes().iter().all(u8::is_ascii_graphic);
    if !formed {
        return Err(refused("the first line is the AI client's pass, dkp_…"));
    }
    let again = Token::new(pass.bytes().to_vec());
    let client = client::check(shared, Door::Socket, pass).await?;
    owned_by(shared, &client, user)?;
    Ok((client, again))
}

/// Serves one connection: its pass, then its messages until it goes or
/// its pass is no longer taken.
async fn connection(shared: Rc<Shared>, stream: UnixStream, open: Rc<Open>) {
    let user = user_of(&stream);
    let (reading, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reading);
    let Some(user) = user else {
        let why = Failure::new(Kind::ClientRefused, "admind cannot tell who is asking");
        say(&mut writer, &no(&why)).await;
        return;
    };
    let Some(_place) = Place::take(&open, user) else {
        let why = Failure::wait("as many AI clients are connected as may be", 5);
        say(&mut writer, &no(&why)).await;
        return;
    };
    if let Some(seconds) = open.waits(user, Instant::now()) {
        let why = "this account brought too many passes that were not taken";
        say(&mut writer, &no(&Failure::wait(why, seconds))).await;
        return;
    }
    let (client, pass) = match greeted(&shared, &mut reader, user).await {
        Ok(greeted) => greeted,
        Err(failure) => {
            // A pass that is not one, and not authd's silence.
            if failure.kind == Kind::ClientRefused {
                open.wrong(user, Instant::now());
            }
            say(&mut writer, &no(&failure)).await;
            return;
        }
    };
    let told = client.told();
    if !say(&mut writer, format!("ok {} {}", client.owner, client.name).as_bytes()).await {
        return;
    }
    log(6, &format!("{told} connected over SSH"));
    // The answers go out by one writer, each whole.
    let (answers, mut due) = mpsc::channel::<Vec<u8>>(AT_ONCE);
    let writing = tokio::task::spawn_local(async move {
        while let Some(answer) = due.recv().await {
            if !say(&mut writer, &answer).await {
                break;
            }
        }
    });
    let slots = Arc::new(Semaphore::new(AT_ONCE));
    let why: String = loop {
        // Before the next message is read: one more than may be answered
        // at once waits in the socket.
        let Ok(slot) = Arc::clone(&slots).acquire_owned().await else {
            break "admind stops".into();
        };
        if answers.is_closed() {
            break "it does not read its answers".into();
        }
        let message = match line(&mut reader, MESSAGE_MAX).await {
            Ok(Some(message)) => message,
            Ok(None) => break "it went".into(),
            Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                let error = rpc::Error::invalid(format!(
                    "a message is one line of at most {MESSAGE_MAX} bytes"
                ));
                let _ = answers.send(rpc::answer(None, Err(error)).to_string().into_bytes()).await;
                break "it sent a line that is too long".into();
            }
            Err(_) => break "its connection failed".into(),
        };
        if message.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Asked of authd again each time, as a token is with each request.
        let again = Token::new(pass.bytes().to_vec());
        let client = match client::check(&shared, Door::Socket, again).await {
            Ok(client) => client,
            Err(failure) => break format!("its pass is not taken ({})", failure.kind.id()),
        };
        let (shared, answers) = (Rc::clone(&shared), answers.clone());
        tokio::task::spawn_local(async move {
            let answered = server::handle(&shared, &client, &message, None).await;
            if let Some(body) = answered.body {
                // A connection that has gone has no one to tell.
                let _ = answers.send(body.to_string().into_bytes()).await;
            }
            drop(slot);
        });
    };
    log(6, &format!("{told} disconnected: {why}"));
    // What is being answered still is written before the writer ends:
    // it ends when the last task has let its sender go.
    drop(answers);
    let _ = writing.await;
}

#[cfg(test)]
mod tests;
