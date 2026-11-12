// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A session channel (RFC 4254 6): the client may ask for a terminal and
//! send its language settings, then asks for a shell or a command, which
//! authd(8) starts as the person who signed in. sshd passes the bytes
//! between the channel and the session's terminal, or its three pipes,
//! and tells the client how the session ended.
//!
//! The client may send only as much as the session has taken: sshd gives
//! it room again as it writes what came to the session (RFC 4254 5.2),
//! as Linux's sshd does, so a session slow to read slows its client and
//! holds nothing else up.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use accounts::request::{self, Answer, Request, SshProof, SshRun};
use russh::server::Handle;
use russh::{ChannelId, Sig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle};

use crate::authd;
use crate::pty;
use crate::sys;

/// How much a client may send on a channel before the session takes
/// it: the window it starts with, and the most it has again.
pub const WINDOW: u32 = 2 * 1024 * 1024;

/// How long the terminal's last output is read for once the session's
/// first process has ended, while programs it left behind may hold it.
const DRAIN_TIME: Duration = Duration::from_secs(1);

/// Who signed in, and how: authd checks it again for each session.
pub struct SignedIn {
    /// The account.
    pub name: String,
    /// What the client signed with their key, and its signature; or the
    /// ticket authd gave for their password.
    pub proof: SshProof,
}

/// A session channel.
#[derive(Default)]
pub struct Channel {
    /// The terminal asked for.
    pub pty: Option<pty::Asked>,
    /// The client's language settings, as `NAME=value`.
    pub language: Vec<String>,
    /// The session, once it runs.
    pub running: Option<Running>,
}

/// A running session's ends that the connection holds.
pub struct Running {
    /// Where the client's bytes go, until it sends its end of input.
    input: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// How many of them the session has not taken yet.
    queued: Arc<AtomicUsize>,
    /// The terminal's master, to change its size.
    master: Option<OwnedFd>,
    /// What passes the bytes, and last what waits for the end and closes
    /// the channel.
    tasks: Vec<AbortHandle>,
    /// The channel, to give its client room.
    handle: Handle,
    id: ChannelId,
}

/// A client sent more than its window let it.
#[derive(Debug)]
pub struct Overrun;

impl Running {
    /// Passes the client's `bytes` on to the session, without waiting:
    /// they wait for it in a queue, which the window keeps to [`WINDOW`]
    /// bytes, unless the client does not keep to it.
    pub fn input(&mut self, bytes: &[u8]) -> Result<(), Overrun> {
        if let Some(input) = &self.input {
            let queued = self.queued.fetch_add(bytes.len(), Ordering::Relaxed) + bytes.len();
            if queued > WINDOW as usize {
                return Err(Overrun);
            }
            if input.send(bytes.to_vec()).is_ok() {
                return Ok(());
            }
            self.queued.fetch_sub(bytes.len(), Ordering::Relaxed);
            self.input = None;
        }
        // The session takes no more input: what comes is passed over, and
        // the client may send on, as Linux's sshd lets it.
        give_room(&self.handle, self.id, bytes.len());
        Ok(())
    }

    /// The client sent its end of input: a command on pipes gets the end
    /// of its standard input. A terminal is left as it is, as Linux's
    /// sshd leaves it.
    pub fn end_input(&mut self) {
        if self.master.is_none() {
            self.input = None;
        }
    }

    /// Whether the session ended and sshd closed its channel. The
    /// client's close that answers it never reaches the connection's
    /// handler: russh takes it for a channel already let go.
    pub fn ended(&self) -> bool {
        self.tasks.last().is_some_and(AbortHandle::is_finished)
    }

    /// The client's terminal changed its size.
    pub fn resize(&self, size: pty::Size) {
        if let Some(master) = &self.master {
            pty::resize(master, size);
        }
    }
}

impl Drop for Running {
    /// The channel closed: what passes its bytes stops, and the session's
    /// terminal or pipes close with them, as when a client hangs up: a
    /// terminal's session hears SIGHUP.
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The ends sshd keeps of what the session runs on.
enum Ends {
    Terminal(OwnedFd),
    Pipes { input: OwnedFd, output: OwnedFd, error: OwnedFd },
}

/// Starts `channel`'s session for `who`, running what the client asked,
/// `run`, through authd: once it runs, its bytes pass between the channel
/// `id` and it, and the client hears how it ended.
pub async fn start(
    channel: &mut Channel,
    id: ChannelId,
    handle: Handle,
    who: &SignedIn,
    connection: &str,
    run: SshRun,
) -> Result<u32, String> {
    let failed = |err: io::Error| format!("cannot make the session's terminal or pipes: {err}");
    let (ends, given): (Ends, Vec<OwnedFd>) = match &channel.pty {
        Some(asked) => {
            let (master, terminal) = pty::open(asked).map_err(failed)?;
            (Ends::Terminal(master), vec![terminal])
        }
        None => {
            let (input, to_input) = sys::pipe().map_err(failed)?;
            let (from_output, output) = sys::pipe().map_err(failed)?;
            let (from_error, error) = sys::pipe().map_err(failed)?;
            (
                Ends::Pipes { input: to_input, output: from_output, error: from_error },
                vec![input, output, error],
            )
        }
    };
    let request = Request::SshSession {
        connection: connection.into(),
        name: who.name.clone(),
        proof: who.proof.clone(),
        term: channel.pty.as_ref().map_or_default(|p| p.term.clone()),
        run,
        language: channel.language.clone(),
    };
    let fds: Vec<RawFd> = given.iter().map(AsRawFd::as_raw_fd).collect();
    let (answer, asked) = authd::ask(&request, &fds).await?;
    // The session holds its ends now.
    drop(given);
    let process = match answer {
        Answer::Done(process) => process.parse().map_err(|_| "authd's answer is not a process")?,
        Answer::Refused(why) => return Err(why),
    };
    // tokio's pipes, as tokio's AsyncFd waits on Radix for one event
    // only: mio's poll(2) takes the interest away once it is told, and
    // only mio's own sources ask for it again.
    let reader = |fd: OwnedFd| {
        sys::set_nonblocking(&fd).and_then(|()| pipe::Receiver::from_owned_fd_unchecked(fd))
    };
    let writer = |fd: OwnedFd| {
        sys::set_nonblocking(&fd).and_then(|()| pipe::Sender::from_owned_fd_unchecked(fd))
    };
    let (input_sender, input) = mpsc::unbounded_channel();
    let queued = Arc::new(AtomicUsize::new(0));
    let taken = Taken { queued: Arc::clone(&queued), handle: handle.clone(), id };
    let (master, tasks) = match ends {
        Ends::Terminal(master) => {
            // The master is read and written through copies of it: the
            // terminal ends once all of them close.
            let from = reader(master.try_clone().map_err(failed)?).map_err(failed)?;
            let to = writer(master.try_clone().map_err(failed)?).map_err(failed)?;
            let output = tokio::spawn(read_out(from, handle.clone(), id, None));
            let writer = tokio::spawn(write_in(to, input, taken));
            let mut tasks = vec![output.abort_handle(), writer.abort_handle()];
            let end = tokio::spawn(end(asked, vec![output], true, handle.clone(), id));
            tasks.push(end.abort_handle());
            (Some(master), tasks)
        }
        Ends::Pipes { input: stdin, output, error } => {
            let stdin = writer(stdin).map_err(failed)?;
            let output = reader(output).map_err(failed)?;
            let error = reader(error).map_err(failed)?;
            let out = tokio::spawn(read_out(output, handle.clone(), id, None));
            // Standard error is extended data of type 1 (RFC 4254 5.2).
            let err = tokio::spawn(read_out(error, handle.clone(), id, Some(1)));
            let writer = tokio::spawn(write_in(stdin, input, taken));
            let mut tasks = vec![out.abort_handle(), err.abort_handle(), writer.abort_handle()];
            let end = tokio::spawn(end(asked, vec![out, err], false, handle.clone(), id));
            tasks.push(end.abort_handle());
            (None, tasks)
        }
    };
    let running = Running { input: Some(input_sender), queued, master, tasks, handle, id };
    channel.running = Some(running);
    Ok(process)
}

/// Reads what the session writes to `from` and sends it on the channel,
/// as data or as extended data of type `extended`, until the end.
async fn read_out(mut from: pipe::Receiver, handle: Handle, id: ChannelId, extended: Option<u32>) {
    let mut buffer = vec![0u8; 32 * 1024];
    loop {
        let bytes = match from.read(&mut buffer).await {
            // The end of a pipe; a terminal no one holds any more reads
            // EIO.
            Ok(0) => return,
            Ok(n) => buffer[..n].to_vec(),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        let sent = match extended {
            Some(kind) => handle.extended_data(id, kind, bytes).await,
            None => handle.data(id, bytes).await,
        };
        if sent.is_err() {
            return;
        }
    }
}

/// Gives the client of channel `id` room for `bytes` more, without
/// waiting: the connection's loop, which may be the caller, sends it.
pub fn give_room(handle: &Handle, id: ChannelId, bytes: usize) {
    let handle = handle.clone();
    tokio::spawn(async move {
        let _ = handle.window_adjust(id, room(bytes)).await;
    });
}

/// `bytes` as a window's size.
fn room(bytes: usize) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

/// What the writer tells as the session takes the client's bytes.
struct Taken {
    queued: Arc<AtomicUsize>,
    handle: Handle,
    id: ChannelId,
}

/// Writes the client's bytes, as they come, to the session's `to`, and
/// gives the client room for as many again; when the client sends its
/// end of input, `to` closes. Once the session closes its input, what
/// comes is passed over.
async fn write_in(mut to: pipe::Sender, mut input: mpsc::UnboundedReceiver<Vec<u8>>, taken: Taken) {
    let mut open = true;
    while let Some(bytes) = input.recv().await {
        if open && write_all(&mut to, &bytes).await.is_err() {
            open = false;
            // What comes later is passed over where it comes.
            input.close();
        }
        taken.queued.fetch_sub(bytes.len(), Ordering::Relaxed);
        if taken.handle.window_adjust(taken.id, room(bytes.len())).await.is_err() {
            return;
        }
    }
}

/// Writes all of `bytes` to `to`.
pub async fn write_all(to: &mut pipe::Sender, bytes: &[u8]) -> io::Result<()> {
    let mut at = 0;
    while at < bytes.len() {
        match to.write(&bytes[at..]).await {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => at += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Waits for authd to say the session ended, and for what it wrote, then
/// tells the client how it ended and closes the channel. A terminal's
/// output is read for [`DRAIN_TIME`] at most after the end, as programs
/// the session left behind may hold it; pipes are read to their ends.
async fn end(
    mut asked: authd::Asked,
    outputs: Vec<JoinHandle<()>>,
    terminal: bool,
    handle: Handle,
    id: ChannelId,
) {
    let status = loop {
        match asked.line().await {
            Ok(Some(line)) => {
                if let Some(status) = request::ended_status(&line) {
                    break Some(status);
                }
            }
            Ok(None) | Err(_) => break None,
        }
    };
    for output in outputs {
        if terminal {
            let abort = output.abort_handle();
            if tokio::time::timeout(DRAIN_TIME, output).await.is_err() {
                abort.abort();
            }
        } else {
            let _ = output.await;
        }
    }
    match status {
        Some(status) if libc::WIFSIGNALED(status) => {
            let signal = libc::WTERMSIG(status);
            match signal_name(signal) {
                Some(name) => {
                    let core = libc::WCOREDUMP(status);
                    let _ = handle
                        .exit_signal_request(id, name, core, String::new(), String::new())
                        .await;
                }
                // As a shell says it: 128 and the signal's number.
                None => {
                    let _ = handle.exit_status_request(id, 128 + signal as u32).await;
                }
            }
        }
        Some(status) => {
            let _ = handle.exit_status_request(id, libc::WEXITSTATUS(status) as u32).await;
        }
        None => {}
    }
    let _ = handle.eof(id).await;
    let _ = handle.close(id).await;
}

/// SSH's name of `signal` (RFC 4254 6.10), for the signals it names.
fn signal_name(signal: libc::c_int) -> Option<Sig> {
    Some(match signal {
        libc::SIGABRT => Sig::ABRT,
        libc::SIGALRM => Sig::ALRM,
        libc::SIGFPE => Sig::FPE,
        libc::SIGHUP => Sig::HUP,
        libc::SIGILL => Sig::ILL,
        libc::SIGINT => Sig::INT,
        libc::SIGKILL => Sig::KILL,
        libc::SIGPIPE => Sig::PIPE,
        libc::SIGQUIT => Sig::QUIT,
        libc::SIGSEGV => Sig::SEGV,
        libc::SIGTERM => Sig::TERM,
        libc::SIGUSR1 => Sig::USR1,
        _ => return None,
    })
}
