// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A pane's shell, from its start to its end: admind opens the
//! pseudo-terminal, authd starts the shell on its terminal side
//! (`console\0terminal`), and two tasks read the master's output into the
//! pane and write the pane's typing to it. When the shell and everything
//! that held its terminal are gone, the master reads its end, and the pane
//! ends as `exited`.

use std::io;
use std::os::fd::AsRawFd;
use std::rc::Rc;

use accounts::console::Console;
use accounts::request::Answer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;
use tokio::sync::Notify;

use super::protocol::{CHUNK_MAX, Why};
use super::{Pane, Shell, pty};
use crate::admind::authd;
use crate::admind::errors::{self, Attempt, Failure, Kind};
use crate::admind::shared::{Shared, unix_now};
use crate::signin;

/// The terminal type the page's emulator is.
pub const TERM: &str = "xterm-256color";

/// What a new pane asks for.
#[derive(Debug)]
pub struct Asked<'a> {
    /// Its id, and its terminal's.
    pub id: u32,
    pub terminal: u32,
    /// The console session that asks, as authd names it.
    pub session: &'a str,
    /// Whether root's shell is asked for.
    pub admin: bool,
    pub cols: u16,
    pub rows: u16,
}

/// A pane whose shell runs, and the ends of its master its tasks read and
/// write.
#[derive(Debug)]
pub struct Started {
    pub pane: Pane,
    reader: pipe::Receiver,
    writer: pipe::Sender,
}

/// Opens a pseudo-terminal of the size `asked` gives and has authd start
/// the shell on it.
///
/// # Errors
///
/// ADM-121 when the terminal cannot be opened, ADM-111 when authd does not
/// answer, and authd's refusal, which may carry an ID of its own (ADM-130,
/// ADM-132).
pub async fn start(shared: &Shared, asked: &Asked<'_>) -> Result<Started, Failure> {
    let failed =
        |err: io::Error| Failure::new(Kind::Internal, format!("cannot open a terminal: {err}"));
    let (master, terminal) = pty::open(asked.cols, asked.rows).map_err(failed)?;
    let tty = signin::terminal_name(&terminal);
    let request = Console::Terminal {
        session: asked.session.to_string(),
        admin: asked.admin,
        term: TERM.into(),
        // The machine's language: the console's own words are English.
        language: Vec::new(),
    };
    let answer = authd::ask(&shared.authd, request, &[terminal.as_raw_fd()]).await;
    // The shell holds the terminal now, if it started.
    drop(terminal);
    let process = match answer.map_err(|why| Failure::new(Kind::AuthdSilent, why))?.0 {
        Answer::Done(text) => text.trim().parse::<i32>().map_err(|_| {
            Failure::new(Kind::AuthdSilent, "authd answered a process that does not read")
        })?,
        Answer::Refused(why) => return Err(errors::refusal(Attempt::Other, &why)),
    };
    let (reader, writer) = pty::ends(&master).map_err(failed)?;
    let shell = Shell { master, process, tty };
    let admin = asked.admin.then(|| asked.session.to_string());
    let pane =
        Pane::new(asked.id, asked.terminal, admin, shell, asked.cols, asked.rows, unix_now());
    Ok(Started { pane, reader, writer })
}

/// Starts the tasks of the pane `started` was, which is kept now.
pub fn run(shared: &Rc<Shared>, id: u32, reader: pipe::Receiver, writer: pipe::Sender) {
    let Some((reading, writing)) = shared.terminals.borrow().notifies(id) else { return };
    let read = tokio::task::spawn_local(read_output(Rc::clone(shared), id, reader, reading));
    let write = tokio::task::spawn_local(write_typing(Rc::clone(shared), id, writer, writing));
    shared.terminals.borrow_mut().set_tasks(id, vec![read.abort_handle(), write.abort_handle()]);
}

impl Started {
    /// The pane, and the ends its tasks take once it is kept.
    pub fn into_parts(self) -> (Pane, pipe::Receiver, pipe::Sender) {
        (self.pane, self.reader, self.writer)
    }
}

/// Reads pane `id`'s output from its master into the pane, except while
/// it is held back, until the master ends; then the pane ends.
async fn read_output(shared: Rc<Shared>, id: u32, mut reader: pipe::Receiver, go_on: Rc<Notify>) {
    let mut buffer = vec![0u8; CHUNK_MAX];
    loop {
        loop {
            match shared.terminals.borrow().paused(id) {
                None => return,
                Some(false) => break,
                Some(true) => {}
            }
            go_on.notified().await;
        }
        match reader.read(&mut buffer).await {
            // A terminal no one holds any more reads EIO, or its end.
            Ok(0) => break,
            Ok(read) => {
                let now = std::time::Instant::now();
                shared.terminals.borrow_mut().output(id, &buffer[..read], now);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // An administrator's shell also ends when its unlock runs out, which
    // authd sees as soon as admind does: the page is told `locked` then.
    let admin = shared.terminals.borrow().admin_of(id);
    let why = match admin {
        Some(session) if !shared.unlocked_at(&session, unix_now()) => Why::Locked,
        _ => Why::Exited,
    };
    shared.terminals.borrow_mut().end_pane(id, why, None);
}

/// Writes pane `id`'s typing to its master as it comes, and makes room for
/// more as each piece is taken.
async fn write_typing(shared: Rc<Shared>, id: u32, mut writer: pipe::Sender, typed: Rc<Notify>) {
    loop {
        let next = shared.terminals.borrow_mut().next_typing(id);
        match next {
            Err(()) => return,
            Ok(None) => typed.notified().await,
            Ok(Some(bytes)) => {
                if writer.write_all(&bytes).await.is_err() {
                    // The master is gone: the reader ends the pane.
                    return;
                }
                shared.terminals.borrow_mut().typing_taken(id, bytes.len());
            }
        }
    }
}
