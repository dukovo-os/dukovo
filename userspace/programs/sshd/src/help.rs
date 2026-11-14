// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A helper's connection to a help session (ADR 0540). The helper signs
//! in as [`HELP_NAME`], which is no account, with the code the session's
//! owner gave them, typed at the client's password or keyboard-interactive
//! prompt, or with the key the owner named; authd(8) checks either and
//! answers with the help screen's socket and the session's transcript.
//! The connection then has one session channel, a terminal and a shell:
//! its bytes go to and from the help screen through the socket, where the
//! owner sees them, and each line the helper types is written to the
//! transcript before it goes, so that nothing the helper typed goes
//! unkept. The helper gets no command, SFTP or forwarding, and the screen
//! keeps the owner's size.
//!
//! While a session waits for a code ([`HELP_WAITING`]), sshd offers
//! passwords for [`HELP_NAME`] even when sshd_config(5) turns them off,
//! and for no other name; while sshd runs for a help session alone
//! ([`HELP_ONLY`]), only [`HELP_NAME`] signs in.

use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use accounts::request::{self, HELP_ONLY, HELP_TRANSCRIPT_MAX, HELP_WAITING, HelpLines};
use accounts::sshd_config::Settings;
use russh::server::Handle;
use russh::{ChannelId, Disconnect, MethodKind};
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::session::{self, Running, Taken};
use crate::sys;

/// What the room left in a transcript must hold besides a line: its time
/// and kind, and authd's own lines at the end.
const LINE_ROOM: u64 = 256;

/// What the helper reads first, before the owner's screen.
const GREETING: &str = "\r\nYou are in a help session: you see and type in its owner's screen, \
                        as them. They see everything, and can end it at any time. What you type \
                        is kept.\r\n\r\n";

/// Whether sshd runs for a help session alone.
pub fn only() -> bool {
    Path::new(HELP_ONLY).exists()
}

/// Whether a help session waits for a helper to sign in with a code.
fn waits_for_code() -> bool {
    Path::new(HELP_WAITING).exists()
}

/// The ways a client may sign in, by the `settings` and the help
/// sessions: a key, if the settings take keys; a password, if they take
/// passwords and sshd does not run for a help session alone, or while a
/// session waits for a code, which keyboard-interactive takes too.
pub fn methods(settings: &Settings) -> Vec<MethodKind> {
    let waits = waits_for_code();
    let mut methods = Vec::new();
    if settings.keys {
        methods.push(MethodKind::PublicKey);
    }
    if waits {
        methods.push(MethodKind::KeyboardInteractive);
    }
    if waits || (settings.passwords && !only()) {
        methods.push(MethodKind::Password);
    }
    methods
}

/// A helper signed in to a help session.
pub struct Helping {
    /// The session's ID.
    pub id: String,
    /// The help screen's socket, until the shell runs on it.
    socket: Option<OwnedFd>,
    /// The transcript.
    transcript: Arc<Mutex<Transcript>>,
    /// Whether the connection opened its one session channel.
    pub opened: bool,
}

impl Helping {
    /// The helper signed in to the session `id`, whose help screen's
    /// `socket` and `transcript` authd handed over.
    pub fn new(id: String, socket: OwnedFd, transcript: File) -> Helping {
        Helping {
            id,
            socket: Some(socket),
            transcript: Arc::new(Mutex::new(Transcript::new(transcript))),
            opened: false,
        }
    }

    /// Keeps what is left of a line the helper typed, as the connection
    /// ends.
    pub fn finish(&self) {
        let mut transcript = self.transcript.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(why) = transcript.end_line() {
            eprintln!("sshd: help session {}: {why}", self.id);
        }
    }

    /// Runs the help screen on `channel`, the session channel `id`: its
    /// bytes pass between the channel and the socket, the helper's
    /// written to the transcript before they go. When the help screen
    /// ends, so does the connection.
    pub fn start(
        &mut self,
        channel: &mut session::Channel,
        id: ChannelId,
        handle: Handle,
    ) -> Result<(), String> {
        let socket = self.socket.take().ok_or("the help screen is shown on one channel only")?;
        let failed = |err: std::io::Error| format!("cannot use the help screen's socket: {err}");
        sys::set_nonblocking(&socket).map_err(failed)?;
        let socket = std::os::unix::net::UnixStream::from(socket);
        let (from, to) = tokio::net::UnixStream::from_std(socket).map_err(failed)?.into_split();
        let output = {
            let handle = handle.clone();
            tokio::spawn(async move {
                if handle.data(id, GREETING.as_bytes().to_vec()).await.is_ok() {
                    session::read_out(from, handle, id, None).await;
                }
            })
        };
        let (input_sender, input) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let taken = Taken::new(Arc::clone(&queued), handle.clone(), id);
        let transcript = Arc::clone(&self.transcript);
        let writer = tokio::spawn(write_in(to, input, taken, transcript, handle.clone()));
        let mut tasks = vec![output.abort_handle(), writer.abort_handle()];
        let end = tokio::spawn(end(output, Arc::clone(&self.transcript), handle.clone(), id));
        tasks.push(end.abort_handle());
        channel.running = Some(Running::of(input_sender, queued, tasks, handle, id));
        Ok(())
    }
}

/// Writes the helper's bytes, as they come, to the help screen, each line
/// kept in the transcript before it goes; a transcript that cannot keep
/// them ends the connection, and they do not go.
async fn write_in(
    mut to: OwnedWriteHalf,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    taken: Taken,
    transcript: Arc<Mutex<Transcript>>,
    handle: Handle,
) {
    let mut open = true;
    while let Some(bytes) = input.recv().await {
        let kept = transcript.lock().unwrap_or_else(PoisonError::into_inner).keep(&bytes);
        if let Err(why) = kept {
            eprintln!("sshd: a helper's connection ends: {why}");
            let reason = format!("the help session ends: {why}");
            let _ = handle.disconnect(Disconnect::ByApplication, reason, String::new()).await;
            return;
        }
        if open && session::write_all(&mut to, &bytes).await.is_err() {
            open = false;
            // The help screen ended: the reader sees it and ends the
            // connection; what comes meanwhile is passed over.
            input.close();
        }
        if !taken.took(bytes.len()).await {
            return;
        }
    }
}

/// Waits for the help screen to end, which its socket's end says, keeps
/// the helper's last line, and ends the connection.
async fn end(
    output: JoinHandle<()>,
    transcript: Arc<Mutex<Transcript>>,
    handle: Handle,
    id: ChannelId,
) {
    let _ = output.await;
    if let Err(why) = transcript.lock().unwrap_or_else(PoisonError::into_inner).end_line() {
        eprintln!("sshd: {why}");
    }
    let _ = handle.eof(id).await;
    let _ = handle.close(id).await;
    let reason = "the help session ended".to_string();
    let _ = handle.disconnect(Disconnect::ByApplication, reason, String::new()).await;
}

/// A help session's transcript, as sshd writes the helper's lines to it.
struct Transcript {
    file: File,
    lines: HelpLines,
}

impl Transcript {
    fn new(file: File) -> Transcript {
        Transcript { file, lines: HelpLines::default() }
    }

    /// Keeps what the helper typed, `bytes`: each line they end as a line
    /// of the transcript; then whether the transcript has room left for
    /// the line not ended yet, whose bytes go to the help screen now.
    fn keep(&mut self, bytes: &[u8]) -> Result<(), String> {
        for line in self.lines.take(bytes) {
            self.write_line(&line)?;
        }
        self.room(request::help_printable(self.lines.rest()).len())
    }

    /// Keeps the line not ended yet, if anything was typed of it.
    fn end_line(&mut self) -> Result<(), String> {
        match self.lines.end() {
            Some(line) => self.write_line(&line),
            None => Ok(()),
        }
    }

    /// Writes `line`, which the helper typed.
    fn write_line(&mut self, line: &[u8]) -> Result<(), String> {
        let seconds =
            SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        let text = request::help_transcript_line(seconds, "typed", line);
        self.room(text.len())?;
        self.file
            .write_all(text.as_bytes())
            .map_err(|err| format!("cannot write the transcript: {err}"))
    }

    /// Whether the transcript has room for `bytes` more, with
    /// [`LINE_ROOM`] to spare.
    fn room(&self, bytes: usize) -> Result<(), String> {
        let size = self
            .file
            .metadata()
            .map_err(|err| format!("cannot read the transcript's size: {err}"))?
            .len();
        if size + bytes as u64 + LINE_ROOM > HELP_TRANSCRIPT_MAX {
            return Err(format!("its transcript is full, at {} KiB", HELP_TRANSCRIPT_MAX >> 10));
        }
        Ok(())
    }
}
