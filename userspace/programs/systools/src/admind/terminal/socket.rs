// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A Terminal page's WebSocket, once its request was answered 101: one
//! task reads the page's frames and does what its messages ask, another
//! writes what waits for the page (`Terminals::next_frame`). admind pings
//! every 25 seconds, and closes a socket that has answered none for 60
//! (1008). A close from the page is answered with one, and the
//! connection ends; one admind closes (the session ended: 4001; admind
//! stops: 1001) stops the reader too.

use std::future::{Future, poll_fn};
use std::io;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use super::protocol::{self, Asked};
use super::{Next, Typed};
use crate::admind::limits::Slot;
use crate::admind::log;
use crate::admind::shared::{Shared, unix_now};
use crate::admind::websocket::{Code, Frames, Message};

/// How often admind pings, and how long it waits for a pong.
pub const PING_EVERY: Duration = Duration::from_secs(25);
pub const PONG_WAIT: Duration = Duration::from_secs(60);

/// How much is read from the connection at once.
const READ_CHUNK: usize = 16 << 10;

/// How long the writer has to send the close frame once the reader is
/// done.
const CLOSE_TIME: Duration = Duration::from_secs(5);

/// How long after the first of an attach's two sizes the second comes.
const NUDGE_TIME: Duration = Duration::from_millis(50);

/// Serves socket `id` over `io` until it closes, holding the connection's
/// place `slot` until then.
pub async fn serve<T>(shared: Rc<Shared>, id: u64, io: T, slot: Option<Slot>)
where
    T: AsyncRead + AsyncWrite + 'static,
{
    let Some(wake) = shared.terminals.borrow().socket_wake(id) else { return };
    let (reader, writer) = tokio::io::split(io);
    let writing = tokio::task::spawn_local(write_frames(Rc::clone(&shared), id, writer, wake));
    read_messages(&shared, id, reader).await;
    shared.terminals.borrow_mut().close(id, Code::Done);
    let abort = writing.abort_handle();
    if tokio::time::timeout(CLOSE_TIME, writing).await.is_err() {
        abort.abort();
    }
    shared.terminals.borrow_mut().close_socket(id, std::time::Instant::now());
    drop(slot);
}

/// Writes socket `id`'s frames to `writer` as they wait, until it closes
/// or the connection fails.
async fn write_frames<W: AsyncWrite>(
    shared: Rc<Shared>,
    id: u64,
    writer: W,
    wake: Rc<tokio::sync::Notify>,
) {
    let mut writer = pin!(writer);
    loop {
        let next = shared.terminals.borrow_mut().next_frame(id);
        match next {
            Next::Frame(frame) => {
                if writer.write_all(&frame).await.is_err() {
                    break;
                }
            }
            // What was written goes out before the wait, a key's echo
            // among it.
            Next::Wait => {
                if writer.flush().await.is_err() {
                    break;
                }
                wake.notified().await;
            }
            Next::Stop => break,
        }
    }
    let _ = writer.flush().await;
    let _ = writer.shutdown().await;
}

/// What woke the reader.
enum Woke {
    Read(io::Result<usize>),
    Stop,
    Time,
}

/// Reads socket `id`'s frames from `reader` and does what their messages
/// ask, until the page closes, a frame breaks a rule, the connection
/// ends, or the socket is closed.
async fn read_messages<R: AsyncRead>(shared: &Rc<Shared>, id: u64, reader: R) {
    let mut reader = pin!(reader);
    let mut frames = Frames::new();
    let mut buffer = vec![0u8; READ_CHUNK];
    let mut heard = Instant::now();
    let mut ping_at = heard + PING_EVERY;
    let Some(stop) = shared.terminals.borrow().socket_stop(id) else { return };
    loop {
        loop {
            match frames.next() {
                Ok(Some(message)) => {
                    if !handle(shared, id, message, &mut heard).await {
                        return;
                    }
                }
                Ok(None) => break,
                Err(code) => {
                    shared.terminals.borrow_mut().close(id, code);
                    return;
                }
            }
        }
        if shared.terminals.borrow().closing(id) {
            return;
        }
        let deadline = ping_at.min(heard + PONG_WAIT);
        let woke = {
            let mut read = pin!(reader.read(&mut buffer));
            let mut stopped = pin!(stop.notified());
            let mut late = pin!(tokio::time::sleep_until(deadline));
            poll_fn(|cx| {
                if let Poll::Ready(read) = read.as_mut().poll(cx) {
                    return Poll::Ready(Woke::Read(read));
                }
                if stopped.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Woke::Stop);
                }
                if late.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Woke::Time);
                }
                Poll::Pending
            })
            .await
        };
        match woke {
            Woke::Read(Ok(0) | Err(_)) | Woke::Stop => return,
            Woke::Read(Ok(read)) => frames.push(&buffer[..read]),
            Woke::Time => {
                let now = Instant::now();
                if now >= heard + PONG_WAIT {
                    log(6, "a terminal's socket answered no ping for 60 seconds: closed");
                    shared.terminals.borrow_mut().close(id, Code::Policy);
                    return;
                }
                if now >= ping_at {
                    shared.terminals.borrow_mut().ping(id);
                    ping_at = now + PING_EVERY;
                }
            }
        }
    }
}

/// Does what `message` asks on socket `id`; `false` once the socket is to
/// close.
async fn handle(shared: &Rc<Shared>, id: u64, message: Message, heard: &mut Instant) -> bool {
    match message {
        Message::Text(text) => {
            let Some(asked) = protocol::asked(&text) else {
                shared.terminals.borrow_mut().close(id, Code::Policy);
                return false;
            };
            ask(shared, id, asked);
            true
        }
        Message::Binary(bytes) => {
            let Some((pane, typed)) = protocol::typed(&bytes) else {
                shared.terminals.borrow_mut().close(id, Code::Policy);
                return false;
            };
            type_in(shared, id, pane, typed).await;
            true
        }
        Message::Ping(body) => {
            shared.terminals.borrow_mut().pong(id, &body);
            true
        }
        Message::Pong(_) => {
            *heard = Instant::now();
            true
        }
        Message::Close(_) => {
            shared.terminals.borrow_mut().close(id, Code::Done);
            false
        }
    }
}

/// Does what the text message `asked` asks on socket `id`. A failure goes
/// to the page as an `error` about the pane.
fn ask(shared: &Rc<Shared>, id: u64, asked: Asked) {
    let now = std::time::Instant::now();
    let mut terminals = shared.terminals.borrow_mut();
    match asked {
        Asked::Attach { pane, cols, rows, since } => {
            match terminals.attach(id, pane, (cols, rows), since, (now, unix_now())) {
                Ok(replay) => {
                    log(6, &format!("pane {pane} attached"));
                    if replay {
                        terminals.nudge(pane, true);
                        let shared = Rc::clone(shared);
                        tokio::task::spawn_local(async move {
                            tokio::time::sleep(NUDGE_TIME).await;
                            shared.terminals.borrow().nudge(pane, false);
                        });
                    }
                }
                Err(failure) => terminals.send_text(id, &protocol::error(&failure, pane)),
            }
        }
        Asked::Detach { pane } => {
            terminals.detach(id, pane, now);
            log(6, &format!("pane {pane} detached"));
        }
        Asked::Resize { pane, cols, rows } => {
            if let Err(failure) = terminals.resize(id, pane, cols, rows) {
                terminals.send_text(id, &protocol::error(&failure, pane));
            }
        }
        Asked::Ack { pane, offset } => terminals.ack(id, pane, offset, now),
        // An answer that finds the pane's typing full is dropped, as is one
        // from a page that does not answer for the pane: its program asks
        // again, or goes on without it.
        Asked::Answer { pane, text } => {
            let _ = terminals.answer(id, pane, text.as_bytes());
        }
    }
}

/// Queues `typed` for pane `pane`, waiting while the pane has as much
/// typing as it may hold: the socket is not read meanwhile. A socket that
/// closes, or a pane that ends, stops the wait.
async fn type_in(shared: &Rc<Shared>, id: u64, pane: u32, typed: &[u8]) {
    let Some(stop) = shared.terminals.borrow().socket_stop(id) else { return };
    loop {
        let room = match shared.terminals.borrow_mut().type_in(id, pane, typed) {
            Typed::Full(room) => room,
            Typed::Queued | Typed::Dropped => return,
        };
        let mut notified = pin!(room.notified());
        notified.as_mut().enable();
        // Room may have come between the try and the wait.
        match shared.terminals.borrow_mut().type_in(id, pane, typed) {
            Typed::Full(_) => {}
            Typed::Queued | Typed::Dropped => return,
        }
        let mut stopped = pin!(stop.notified());
        let stopping = poll_fn(|cx| {
            if notified.as_mut().poll(cx).is_ready() {
                return Poll::Ready(false);
            }
            if stopped.as_mut().poll(cx).is_ready() {
                return Poll::Ready(true);
            }
            Poll::Pending
        })
        .await;
        if stopping || shared.terminals.borrow().closing(id) {
            return;
        }
    }
}
