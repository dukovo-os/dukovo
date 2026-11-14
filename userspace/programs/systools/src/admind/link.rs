// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The link between admind and a session's worker (ADR 0514). It is two
//! pipes ([`pipes`]): the calls' pipe, which admind writes and the worker
//! reads as its descriptor 0, and the replies' pipe, which the worker
//! writes as its descriptor 3 and admind reads. Pipes, not a pair of
//! sockets, since a socket cannot be sent to authd, which starts the
//! worker (ADR 0091), and nothing else: the worker opens no socket and
//! listens on none. A frame is 4 bytes, the length of what follows in big-endian
//! order, and then a JSON object: `{id, method, params}` going to the
//! worker, `{id, result}` or `{id, error: {id, title, detail}}` coming
//! back. Calls are told apart by `id`, so that a slow one does not hold up
//! the others, and the answers may come in any order.
//!
//! No frame is longer than [`FRAME_MAX`]. A length past it is refused
//! from the 4 bytes alone, and what a frame says it holds is only taken
//! in as it comes, so a worker that lies about it makes admind keep no
//! more than it sent.
//!
//! [`Link`] is admind's end: each call waits at most [`CALL_TIME`] for its
//! answer, and [`Link::closed`] says when the worker has gone, so that
//! admind ends the session. [`serve`] is the worker's. When a session
//! seems stuck, look in the log for the lines that begin `admind: the
//! worker` and `admind: the link`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::future::Future;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::unix::pipe::{Receiver, Sender};
use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::JoinHandle;

use super::errors::Kind;
use super::log;

/// The most bytes a frame holds, past its length.
pub const FRAME_MAX: usize = 4 << 20;

/// How long a call waits for its answer.
pub const CALL_TIME: Duration = Duration::from_secs(30);

/// The most calls a worker works on at once; another is answered with a
/// wait.
const CALLS_MAX: usize = 16;

/// Why a frame could not be read or written.
#[derive(Debug)]
pub enum FrameError {
    /// Its length is more than [`FRAME_MAX`].
    TooBig(usize),
    /// The link ended inside it.
    Cut,
    /// The pipe failed.
    Io(io::Error),
    /// What it holds is not the JSON that was meant.
    Json(serde_json::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooBig(length) => {
                write!(f, "a frame of {length} bytes is more than the {FRAME_MAX} a frame holds")
            }
            FrameError::Cut => f.write_str("the link ended inside a frame"),
            FrameError::Io(err) => write!(f, "the pipe failed: {err}"),
            FrameError::Json(err) => write!(f, "a frame holds no JSON object that fits: {err}"),
        }
    }
}

/// Reads the next frame from `reader`: its bytes, or `None` when the link
/// ended between two frames.
///
/// # Errors
///
/// If the length is more than [`FRAME_MAX`] (nothing past the 4 bytes of
/// it is read), the link ended inside the frame, or reading failed.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut header = [0u8; 4];
    let mut have = 0;
    while have < header.len() {
        let read = reader.read(&mut header[have..]).await.map_err(FrameError::Io)?;
        if read == 0 {
            return if have == 0 { Ok(None) } else { Err(FrameError::Cut) };
        }
        have += read;
    }
    let length = usize::try_from(u32::from_be_bytes(header)).unwrap_or(usize::MAX);
    if length > FRAME_MAX {
        return Err(FrameError::TooBig(length));
    }
    // Grown as the bytes come, so that a frame that never arrives holds
    // no memory.
    let mut payload = Vec::new();
    let read =
        reader.take(length as u64).read_to_end(&mut payload).await.map_err(FrameError::Io)?;
    if read < length {
        return Err(FrameError::Cut);
    }
    Ok(Some(payload))
}

/// Writes `payload` to `writer` as one frame.
///
/// # Errors
///
/// If it is longer than [`FRAME_MAX`], or writing failed.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), FrameError> {
    if payload.len() > FRAME_MAX {
        return Err(FrameError::TooBig(payload.len()));
    }
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::TooBig(payload.len()))?;
    writer.write_all(&length.to_be_bytes()).await.map_err(FrameError::Io)?;
    writer.write_all(payload).await.map_err(FrameError::Io)?;
    writer.flush().await.map_err(FrameError::Io)
}

/// One side's ends of a link: the pipe it reads and the pipe it writes.
pub struct Ends {
    reads: OwnedFd,
    writes: OwnedFd,
}

impl Ends {
    /// The ends `reads` and `writes`: for the worker, the descriptors authd
    /// gave it.
    pub fn new(reads: OwnedFd, writes: OwnedFd) -> Ends {
        Ends { reads, writes }
    }

    /// The descriptors, the end it reads first, as authd is sent them.
    pub fn fds(&self) -> [RawFd; 2] {
        [self.reads.as_raw_fd(), self.writes.as_raw_fd()]
    }

    /// The ends as the runtime's pipes.
    ///
    /// # Errors
    ///
    /// If either is not a pipe that may be read or written as it must.
    fn open(self) -> io::Result<(Receiver, Sender)> {
        let reads = Receiver::from_file(File::from(self.reads))?;
        let writes = Sender::from_file(File::from(self.writes))?;
        Ok((reads, writes))
    }
}

/// A new link's two pipes: admind's ends, and the worker's, which authd is
/// sent. Each end is closed in the programs either side starts.
///
/// # Errors
///
/// If the pipes cannot be made: too many files open.
pub fn pipes() -> io::Result<(Ends, Ends)> {
    let (calls_read, calls_write) = io::pipe()?;
    let (replies_read, replies_write) = io::pipe()?;
    let ours = Ends::new(replies_read.into(), calls_write.into());
    let theirs = Ends::new(calls_read.into(), replies_write.into());
    Ok((ours, theirs))
}

/// What a worker says when it cannot do what a call asked: an error's ID,
/// title and detail, as [`super::errors`] words them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fault {
    /// The error's ID, `ADM-110`.
    pub id: String,
    /// Its title.
    pub title: String,
    /// What happened this time.
    pub detail: String,
}

impl Fault {
    /// A fault of `kind`, with `detail`.
    pub fn new(kind: Kind, detail: impl Into<String>) -> Fault {
        Fault { id: kind.id().into(), title: kind.title().into(), detail: detail.into() }
    }
}

/// A call: what admind asks of the worker.
#[derive(Debug, Serialize, Deserialize)]
pub struct Call {
    /// Which call it is, as the answer says.
    pub id: u64,
    /// What to do.
    pub method: String,
    /// What to do it with.
    #[serde(default)]
    pub params: Value,
}

/// How a call went.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// What it gave.
    Result(Value),
    /// Why it did not.
    Error(Fault),
}

/// A worker's answer to the call with `id`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    /// The call it answers.
    pub id: u64,
    /// How the call went.
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// Why a call gave no result.
#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    /// The worker answered that it could not.
    Worker(Fault),
    /// No answer came within [`CALL_TIME`].
    Silent,
    /// The link has ended.
    Gone,
}

/// What admind's end of a link and its reading task share.
struct State {
    /// The calls waiting for their answers.
    waiting: RefCell<HashMap<u64, oneshot::Sender<Outcome>>>,
    /// Whether the worker's end has gone.
    closed: watch::Sender<bool>,
}

struct Inner {
    writer: Mutex<Sender>,
    next: Cell<u64>,
    state: Rc<State>,
    reader: JoinHandle<()>,
}

impl Drop for Inner {
    /// The last [`Link`] to go closes the calls' pipe, which ends the
    /// worker.
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// admind's end of a worker's link. Cloning it gives another handle on
/// the same link, so that a call can be made without holding what holds
/// the session.
#[derive(Clone)]
pub struct Link(Rc<Inner>);

/// Takes a call out of the waiting ones however it ends: answered, timed
/// out, or left behind by a caller that went.
struct Waiting<'a> {
    id: u64,
    state: &'a State,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.state.waiting.borrow_mut().remove(&self.id);
    }
}

impl Link {
    /// admind's ends `ours` of a link, with its reading task started.
    ///
    /// # Errors
    ///
    /// If an end is not the pipe [`pipes`] made.
    ///
    /// # Panics
    ///
    /// Outside a `LocalSet`.
    pub fn new(ours: Ends) -> io::Result<Link> {
        let (reader, writer) = ours.open()?;
        let (closed, _) = watch::channel(false);
        let state = Rc::new(State { waiting: RefCell::new(HashMap::new()), closed });
        let reader = tokio::task::spawn_local(read_replies(reader, Rc::clone(&state)));
        Ok(Link(Rc::new(Inner { writer: Mutex::new(writer), next: Cell::new(1), state, reader })))
    }

    /// Has the worker run `method` with `params`, and waits [`CALL_TIME`]
    /// for what it gives.
    ///
    /// # Errors
    ///
    /// With the worker's own fault, or when it is silent or gone.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        self.call_within(method, params, CALL_TIME).await
    }

    async fn call_within(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, CallError> {
        let id = self.0.next.get();
        self.0.next.set(id + 1);
        let (answer, reply) = oneshot::channel();
        self.0.state.waiting.borrow_mut().insert(id, answer);
        let _waiting = Waiting { id, state: &self.0.state };
        let sent = async {
            let call = Call { id, method: method.into(), params };
            let bytes = serde_json::to_vec(&call).map_err(|_| CallError::Gone)?;
            let mut writer = self.0.writer.lock().await;
            match write_frame(&mut *writer, &bytes).await {
                Ok(()) => {}
                Err(FrameError::TooBig(length)) => {
                    let detail = format!("a request of {length} bytes is too big for the worker");
                    return Err(CallError::Worker(Fault::new(Kind::TooBig, detail)));
                }
                Err(_) => return Err(CallError::Gone),
            }
            drop(writer);
            reply.await.map_err(|_| CallError::Gone)
        };
        match tokio::time::timeout(limit, sent).await {
            Ok(Ok(Outcome::Result(value))) => Ok(value),
            Ok(Ok(Outcome::Error(fault))) => Err(CallError::Worker(fault)),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(CallError::Silent),
        }
    }

    /// A future that is ready once the worker's end has gone, or this
    /// link has been dropped. It holds no part of the link, so that
    /// waiting for it does not keep the worker alive.
    pub fn closed(&self) -> impl Future<Output = ()> + use<> {
        let mut closed = self.0.state.closed.subscribe();
        async move {
            // An error is the sender's going, which is the same news.
            let _ = closed.wait_for(|gone| *gone).await;
        }
    }
}

/// Hands each reply to the call that waits for it, until the link ends.
async fn read_replies(mut reader: Receiver, state: Rc<State>) {
    loop {
        let frame = match read_frame(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(err) => {
                log(4, &format!("the link to a worker broke: {err}"));
                break;
            }
        };
        let Ok(reply) = serde_json::from_slice::<Reply>(&frame) else {
            log(4, "the link to a worker broke: it sent a frame that is no reply");
            break;
        };
        // A call that gave up has dropped its receiver.
        if let Some(waiter) = state.waiting.borrow_mut().remove(&reply.id) {
            let _ = waiter.send(reply.outcome);
        }
    }
    // Dropping the senders has each waiting call end as gone.
    state.waiting.borrow_mut().clear();
    state.closed.send_replace(true);
}

/// Why the worker's end of a link ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// admind closed its end.
    Closed,
    /// No call came for the whole time allowed.
    Idle,
    /// The link broke: what was wrong with it.
    Broken(String),
}

/// The worker's end: reads calls from its ends `theirs` and answers each,
/// as `handle` says, while others go on. It ends when admind closes its
/// end, when `idle` passes with no call, or when the link breaks.
///
/// # Errors
///
/// If an end is not the pipe [`pipes`] made.
///
/// # Panics
///
/// Outside a `LocalSet`.
pub async fn serve<H, F>(theirs: Ends, idle: Duration, handle: H) -> io::Result<Ended>
where
    H: Fn(String, Value) -> F + 'static,
    F: Future<Output = Result<Value, Fault>> + 'static,
{
    let (mut reader, writer) = theirs.open()?;
    let writer = Rc::new(Mutex::new(writer));
    let handle = Rc::new(handle);
    let working = Rc::new(Cell::new(0usize));
    loop {
        let frame = match tokio::time::timeout(idle, read_frame(&mut reader)).await {
            Err(_) => return Ok(Ended::Idle),
            Ok(Ok(None)) => return Ok(Ended::Closed),
            Ok(Ok(Some(frame))) => frame,
            Ok(Err(err)) => return Ok(Ended::Broken(err.to_string())),
        };
        let call = match serde_json::from_slice::<Call>(&frame) {
            Ok(call) => call,
            Err(err) => return Ok(Ended::Broken(FrameError::Json(err).to_string())),
        };
        let (handle, writer, working) =
            (Rc::clone(&handle), Rc::clone(&writer), Rc::clone(&working));
        tokio::task::spawn_local(async move {
            let outcome = if working.get() >= CALLS_MAX {
                Outcome::Error(Fault::new(Kind::Wait, "the worker has as many calls as it takes"))
            } else {
                working.set(working.get() + 1);
                let done = handle(call.method, call.params).await;
                working.set(working.get() - 1);
                done.map_or_else(Outcome::Error, Outcome::Result)
            };
            if let Err(err) = answer(&writer, call.id, outcome).await {
                log(4, &format!("the worker cannot answer: {err}"));
            }
        });
    }
}

/// Writes the answer to call `id`; an answer too big for a frame is
/// replaced by the fault that says so.
async fn answer(writer: &Mutex<Sender>, id: u64, outcome: Outcome) -> Result<(), FrameError> {
    let mut bytes = serde_json::to_vec(&Reply { id, outcome }).map_err(FrameError::Json)?;
    if bytes.len() > FRAME_MAX {
        let fault = Fault::new(Kind::TooBig, "the answer is too big to send");
        bytes = serde_json::to_vec(&Reply { id, outcome: Outcome::Error(fault) })
            .map_err(FrameError::Json)?;
    }
    write_frame(&mut *writer.lock().await, &bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::net::UnixStream;
    use tokio::task::LocalSet;

    /// A link: admind's side made into a [`Link`], and the worker's pipes,
    /// which the test reads calls from and writes replies to.
    fn linked() -> (Link, Receiver, Sender) {
        let (ours, theirs) = pipes().unwrap();
        let (calls, replies) = theirs.open().unwrap();
        (Link::new(ours).unwrap(), calls, replies)
    }

    /// Runs `future` on a runtime of its own, as admind does.
    fn run<F: Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        LocalSet::new().block_on(&runtime, future)
    }

    fn frame_of(payload: &[u8]) -> Vec<u8> {
        let mut frame = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn a_short_frame_and_a_long_one_come_back_as_they_went() {
        run(async {
            let (mut ours, mut theirs) = UnixStream::pair().unwrap();
            let long = vec![b'x'; 1 << 20];
            let sender = tokio::task::spawn_local(async move {
                write_frame(&mut ours, b"{}").await.unwrap();
                write_frame(&mut ours, &long).await.unwrap();
                write_frame(&mut ours, b"").await.unwrap();
            });
            assert_eq!(read_frame(&mut theirs).await.unwrap().unwrap(), b"{}");
            let received = read_frame(&mut theirs).await.unwrap().unwrap();
            assert_eq!(received.len(), 1 << 20);
            assert!(received.iter().all(|&byte| byte == b'x'));
            assert_eq!(read_frame(&mut theirs).await.unwrap().unwrap(), b"");
            sender.await.unwrap();
            // A link that ends between frames ends cleanly.
            assert!(read_frame(&mut theirs).await.unwrap().is_none());
        });
    }

    #[test]
    fn a_frame_holds_4_mib_and_one_more_byte_is_refused_before_it_is_read() {
        run(async {
            let (mut ours, mut theirs) = UnixStream::pair().unwrap();
            let biggest = vec![b' '; FRAME_MAX];
            let sender = tokio::task::spawn_local(async move {
                write_frame(&mut ours, &biggest).await.unwrap();
                let refused = write_frame(&mut ours, &vec![b' '; FRAME_MAX + 1]).await;
                assert!(
                    matches!(refused, Err(FrameError::TooBig(length)) if length == FRAME_MAX + 1)
                );
                // The length alone, with nothing after it.
                let length = u32::try_from(FRAME_MAX + 1).unwrap().to_be_bytes();
                ours.write_all(&length).await.unwrap();
                ours
            });
            assert_eq!(read_frame(&mut theirs).await.unwrap().unwrap().len(), FRAME_MAX);
            let error = read_frame(&mut theirs).await.unwrap_err();
            assert!(matches!(error, FrameError::TooBig(length) if length == FRAME_MAX + 1));
            // The sender is still there: the refusal came from the 4 bytes.
            drop(sender.await.unwrap());
        });
    }

    #[test]
    fn a_frame_cut_off_is_an_error_and_not_a_short_frame() {
        run(async {
            for sent in [&[0u8, 0][..], &[0, 0, 0, 10, b'a', b'b', b'c'][..]] {
                let (mut ours, mut theirs) = UnixStream::pair().unwrap();
                ours.write_all(sent).await.unwrap();
                drop(ours);
                let error = read_frame(&mut theirs).await.unwrap_err();
                assert!(matches!(error, FrameError::Cut), "{sent:?}");
            }
        });
    }

    #[test]
    fn answers_may_come_in_any_order() {
        run(async {
            let (link, mut calls_pipe, mut replies) = linked();
            let first = {
                let link = link.clone();
                tokio::task::spawn_local(async move { link.call("one", json!({"n": 1})).await })
            };
            let second = {
                let link = link.clone();
                tokio::task::spawn_local(async move { link.call("two", Value::Null).await })
            };
            let mut calls = Vec::new();
            for _ in 0..2 {
                let frame = read_frame(&mut calls_pipe).await.unwrap().unwrap();
                calls.push(serde_json::from_slice::<Call>(&frame).unwrap());
            }
            calls.sort_by(|a, b| a.method.cmp(&b.method));
            assert_eq!((calls[0].method.as_str(), calls[1].method.as_str()), ("one", "two"));
            assert_ne!(calls[0].id, calls[1].id);
            assert_eq!(calls[0].params, json!({"n": 1}));
            // The later call is answered first.
            for call in calls.iter().rev() {
                let result = json!({ "of": call.method });
                let reply = Reply { id: call.id, outcome: Outcome::Result(result) };
                write_frame(&mut replies, &serde_json::to_vec(&reply).unwrap()).await.unwrap();
            }
            assert_eq!(first.await.unwrap().unwrap(), json!({"of": "one"}));
            assert_eq!(second.await.unwrap().unwrap(), json!({"of": "two"}));
        });
    }

    #[test]
    fn a_fault_a_silent_worker_and_a_gone_one_are_told_apart() {
        run(async {
            let (link, mut calls, mut replies) = linked();
            let closed = link.closed();
            let asked = {
                let link = link.clone();
                tokio::task::spawn_local(async move { link.call("steps", Value::Null).await })
            };
            let frame = read_frame(&mut calls).await.unwrap().unwrap();
            let call = serde_json::from_slice::<Call>(&frame).unwrap();
            let fault = Fault::new(Kind::Refused, "no");
            let reply = Reply { id: call.id, outcome: Outcome::Error(fault.clone()) };
            write_frame(&mut replies, &serde_json::to_vec(&reply).unwrap()).await.unwrap();
            assert_eq!(asked.await.unwrap(), Err(CallError::Worker(fault)));
            // Nothing answers this one.
            let silent = link.call_within("slow", Value::Null, Duration::from_millis(50)).await;
            assert_eq!(silent, Err(CallError::Silent));
            assert!(link.0.state.waiting.borrow().is_empty());
            // The worker goes with a call waiting.
            let waiting = {
                let link = link.clone();
                tokio::task::spawn_local(async move { link.call("again", Value::Null).await })
            };
            read_frame(&mut calls).await.unwrap().unwrap();
            drop((calls, replies));
            assert_eq!(waiting.await.unwrap(), Err(CallError::Gone));
            closed.await;
            assert_eq!(link.call("after", Value::Null).await, Err(CallError::Gone));
        });
    }

    #[test]
    fn a_worker_that_sends_what_is_no_reply_has_the_link_closed() {
        run(async {
            let (link, _calls, mut replies) = linked();
            let closed = link.closed();
            write_frame(&mut replies, b"[1, 2]").await.unwrap();
            closed.await;
            // A length past the most is the same.
            let (link, _calls, mut replies) = linked();
            let closed = link.closed();
            replies.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
            closed.await;
            drop(link);
        });
    }

    #[test]
    fn the_worker_answers_each_call_and_ends_when_the_link_does() {
        run(async {
            let (ours, theirs) = pipes().unwrap();
            let worker = tokio::task::spawn_local(serve(
                theirs,
                Duration::from_secs(60),
                |method: String, params: Value| async move {
                    match method.as_str() {
                        "echo" => Ok(params),
                        "huge" => Ok(Value::String("x".repeat(FRAME_MAX))),
                        _ => Err(Fault::new(Kind::NoRoute, format!("no method {method}"))),
                    }
                },
            ));
            let link = Link::new(ours).unwrap();
            assert_eq!(link.call("echo", json!([1, "two"])).await.unwrap(), json!([1, "two"]));
            let fault = link.call("nothing", Value::Null).await.unwrap_err();
            assert!(matches!(&fault, CallError::Worker(f) if f.id == "ADM-116"), "{fault:?}");
            let fault = link.call("huge", Value::Null).await.unwrap_err();
            assert!(matches!(&fault, CallError::Worker(f) if f.id == "ADM-113"), "{fault:?}");
            drop(link);
            assert_eq!(worker.await.unwrap().unwrap(), Ended::Closed);
        });
    }

    #[test]
    fn the_worker_ends_after_the_time_with_no_call() {
        run(async {
            let (_ours, theirs) = pipes().unwrap();
            let ended = serve(theirs, Duration::from_millis(50), |_: String, _: Value| async {
                Ok(Value::Null)
            })
            .await
            .unwrap();
            assert_eq!(ended, Ended::Idle);
        });
    }

    #[test]
    fn the_worker_takes_as_many_calls_at_once_as_it_may() {
        run(async {
            let (ours, theirs) = pipes().unwrap();
            tokio::task::spawn_local(serve(
                theirs,
                Duration::from_secs(60),
                |_: String, _: Value| async {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    Ok(Value::Null)
                },
            ));
            let link = Link::new(ours).unwrap();
            let calls: Vec<_> = (0..=CALLS_MAX)
                .map(|_| {
                    let link = link.clone();
                    tokio::task::spawn_local(async move { link.call("slow", Value::Null).await })
                })
                .collect();
            let mut busy = 0;
            for call in calls {
                match call.await.unwrap() {
                    Ok(_) => {}
                    Err(CallError::Worker(fault)) if fault.id == "ADM-105" => busy += 1,
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!(busy, 1);
        });
    }

    #[test]
    fn frames_are_a_big_endian_length_and_the_json() {
        assert_eq!(frame_of(b"{}"), [0, 0, 0, 2, b'{', b'}']);
        let call = Call { id: 7, method: "whoami".into(), params: Value::Null };
        assert_eq!(
            serde_json::to_string(&call).unwrap(),
            r#"{"id":7,"method":"whoami","params":null}"#
        );
        let result = Reply { id: 7, outcome: Outcome::Result(json!({"a": 1})) };
        assert_eq!(serde_json::to_string(&result).unwrap(), r#"{"id":7,"result":{"a":1}}"#);
        let error = Reply { id: 8, outcome: Outcome::Error(Fault::new(Kind::Locked, "x")) };
        let text = serde_json::to_string(&error).unwrap();
        assert!(text.starts_with(r#"{"id":8,"error":{"id":"ADM-106","title":"#), "{text}");
        let back: Reply = serde_json::from_str(r#"{"id":9,"result":null}"#).unwrap();
        assert!(matches!(back.outcome, Outcome::Result(Value::Null)));
    }
}
