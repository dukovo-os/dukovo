// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `sshd --forward`: an SSH connection's forwarder (ADR 0349). authd(8)
//! starts it as the person who signed in, with the network their session
//! has (ADR 0257), the first time their client forwards a port. It reads
//! sshd's frames ([`accounts::forwarding`]) on its standard input and
//! writes its own on its standard output: it makes the connections the
//! client asks for, listens on the ports it forwards from here, and
//! passes the bytes of each, as the person's own programs would. What
//! goes wrong goes to its standard error, which sshd logs. It ends when
//! sshd closes its input, as the connection ends, and every connection
//! it holds ends with it.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use accounts::forwarding::{Frame, Frames, PAYLOAD_MAX, QUEUE_MAX};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::unix::pipe;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::AbortHandle;

use crate::sys;

/// How long a connection the client asked for may take.
const CONNECT_TIME: Duration = Duration::from_secs(30);
/// The most connections at once, and ports listened on.
const STREAMS_MAX: usize = 256;
const LISTENS_MAX: usize = 64;
/// How many bytes of frames go to sshd in one write, at most.
const BATCH: usize = 256 * 1024;
/// How long to wait before taking connections again when there are no
/// descriptors left for them.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// What the forwarder hears.
enum Event {
    /// A frame from sshd.
    Frame(Frame),
    /// sshd closed the forwarder's input, or what it sent does not read.
    Ended(Option<String>),
    /// A connection asked for was made, or not.
    Connected { stream: u32, made: Result<TcpStream, String> },
    /// A port listened on took a connection.
    Accepted { listen: u32, socket: TcpStream, peer: SocketAddr },
    /// One way of a stream is done: reading met the end, or writing was
    /// shut down.
    HalfDone { stream: u32 },
    /// A stream's connection broke.
    Broken { stream: u32 },
}

/// What is written to a stream's connection.
enum Write {
    Bytes(Vec<u8>),
    /// The client sent its end: the connection's sending is shut down.
    End,
}

/// The room sshd gave a stream to send in.
#[derive(Default)]
struct Room {
    bytes: AtomicU64,
    more: Notify,
}

impl Room {
    fn give(&self, bytes: u32) {
        self.bytes.fetch_add(u64::from(bytes), Ordering::Relaxed);
        self.more.notify_one();
    }

    /// Waits for room, and says how much, up to a frame's worth.
    async fn wait(&self) -> usize {
        loop {
            let bytes = self.bytes.load(Ordering::Relaxed);
            if bytes > 0 {
                return usize::try_from(bytes).unwrap_or(usize::MAX).min(PAYLOAD_MAX);
            }
            self.more.notified().await;
        }
    }

    fn take(&self, bytes: usize) {
        self.bytes.fetch_sub(bytes as u64, Ordering::Relaxed);
    }
}

/// A stream.
enum Stream {
    /// Its connection is being made.
    Connecting(AbortHandle),
    Open(Open),
    /// sshd closed it, and its writer is writing what came before, as
    /// OpenSSH's sshd drains a channel's output when the client closes
    /// it. It counts as a connection until then.
    Closing(AbortHandle),
}

/// A stream whose connection is made.
struct Open {
    writes: mpsc::UnboundedSender<Write>,
    /// The bytes it holds that are not written yet.
    queued: Arc<AtomicUsize>,
    room: Arc<Room>,
    /// How many ways are done.
    done: u8,
    tasks: [AbortHandle; 2],
}

impl Stream {
    fn stop(&self) {
        match self {
            Stream::Connecting(task) | Stream::Closing(task) => task.abort(),
            Stream::Open(open) => open.tasks.iter().for_each(AbortHandle::abort),
        }
    }
}

/// Where frames to sshd go, encoded.
type Out = mpsc::UnboundedSender<Vec<u8>>;

fn send(out: &Out, frame: &Frame) {
    let mut bytes = Vec::new();
    frame.encode(&mut bytes);
    let _ = out.send(bytes);
}

/// The forwarder's state.
struct Forwarder {
    out: Out,
    events: mpsc::UnboundedSender<Event>,
    streams: HashMap<u32, Stream>,
    listens: HashMap<u32, Vec<AbortHandle>>,
    /// The number the next connection taken gets: odd.
    next: u32,
}

/// `sshd --forward`: serves sshd's frames on standard input.
pub fn run() -> ExitCode {
    // SAFETY: authd gave this process its pipes from sshd as 0 and 1,
    // which nothing else here uses: what it says goes to 2.
    let (input, output) = unsafe { (OwnedFd::from_raw_fd(0), OwnedFd::from_raw_fd(1)) };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("cannot start: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(input, output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(input: OwnedFd, output: OwnedFd) -> Result<(), String> {
    let pipes = |err: io::Error| format!("cannot use the pipes from sshd: {err}");
    sys::set_nonblocking(&input).map_err(pipes)?;
    sys::set_nonblocking(&output).map_err(pipes)?;
    let input = pipe::Receiver::from_owned_fd_unchecked(input).map_err(pipes)?;
    let output = pipe::Sender::from_owned_fd_unchecked(output).map_err(pipes)?;
    let (out, frames) = mpsc::unbounded_channel();
    let (events, mut heard) = mpsc::unbounded_channel();
    tokio::spawn(write_frames(output, frames, events.clone()));
    tokio::spawn(read_frames(input, events.clone()));
    let mut forwarder =
        Forwarder { out, events, streams: HashMap::new(), listens: HashMap::new(), next: 1 };
    while let Some(event) = heard.recv().await {
        match event {
            Event::Frame(frame) => forwarder.frame(frame),
            Event::Ended(None) => return Ok(()),
            Event::Ended(Some(why)) => return Err(why),
            Event::Connected { stream, made } => forwarder.connected(stream, made),
            Event::Accepted { listen, socket, peer } => forwarder.accepted(listen, socket, peer),
            Event::HalfDone { stream } => forwarder.half_done(stream),
            Event::Broken { stream } => forwarder.close(stream),
        }
    }
    Ok(())
}

impl Forwarder {
    fn send(&self, frame: &Frame) {
        send(&self.out, frame);
    }

    fn frame(&mut self, frame: Frame) {
        match frame {
            Frame::Connect { stream, host, port } => self.connect(stream, host, port),
            Frame::Listen { listen, address, port } => match self.listen(listen, &address, port) {
                Ok(port) => self.send(&Frame::Listening { listen, port }),
                Err(why) => self.send(&Frame::Failed { id: listen, why }),
            },
            Frame::Unlisten { listen } => {
                for task in self.listens.remove(&listen).unwrap_or_default() {
                    task.abort();
                }
            }
            Frame::Data { stream, bytes } => {
                // What comes for a stream that ended is passed over.
                let Some(Stream::Open(open)) = self.streams.get(&stream) else { return };
                let queued = open.queued.fetch_add(bytes.len(), Ordering::Relaxed) + bytes.len();
                if queued <= QUEUE_MAX {
                    let _ = open.writes.send(Write::Bytes(bytes));
                    return;
                }
                eprintln!("stream {stream}: more came than its window lets; it is closed");
                self.close(stream);
            }
            Frame::Eof { stream } => {
                if let Some(Stream::Open(open)) = self.streams.get(&stream) {
                    let _ = open.writes.send(Write::End);
                }
            }
            Frame::Close { stream } => self.closed(stream),
            Frame::Credit { stream, bytes } => {
                if let Some(Stream::Open(open)) = self.streams.get(&stream) {
                    open.room.give(bytes);
                }
            }
            Frame::Connected { .. }
            | Frame::Failed { .. }
            | Frame::Listening { .. }
            | Frame::Accepted { .. } => eprintln!("sshd sent a frame of the forwarder's own"),
        }
    }

    /// Makes the connection to `host` and `port` that stream `stream` is.
    fn connect(&mut self, stream: u32, host: String, port: u16) {
        if !stream.is_multiple_of(2) || self.streams.contains_key(&stream) {
            let why = format!("stream {stream} is not one sshd may ask for");
            return self.send(&Frame::Failed { id: stream, why });
        }
        if self.streams.len() >= STREAMS_MAX {
            let why = format!("{STREAMS_MAX} connections are forwarded already");
            return self.send(&Frame::Failed { id: stream, why });
        }
        let events = self.events.clone();
        let task = tokio::spawn(async move {
            // A name is looked up as the person's programs look it up.
            let made =
                match tokio::time::timeout(CONNECT_TIME, TcpStream::connect((host.as_str(), port)))
                    .await
                {
                    Ok(Ok(socket)) => Ok(socket),
                    Ok(Err(err)) => Err(format!("cannot connect to {host} port {port}: {err}")),
                    Err(_) => Err(format!(
                        "cannot connect to {host} port {port}: no answer in {} seconds",
                        CONNECT_TIME.as_secs()
                    )),
                };
            let _ = events.send(Event::Connected { stream, made });
        });
        self.streams.insert(stream, Stream::Connecting(task.abort_handle()));
    }

    fn connected(&mut self, stream: u32, made: Result<TcpStream, String>) {
        // sshd may have closed it as it was made.
        if !matches!(self.streams.get(&stream), Some(Stream::Connecting(_))) {
            return;
        }
        match made {
            Ok(socket) => {
                self.open(stream, socket);
                self.send(&Frame::Connected { stream });
            }
            Err(why) => {
                self.streams.remove(&stream);
                self.send(&Frame::Failed { id: stream, why });
            }
        }
    }

    /// Listens at `address` and `port` as listen `listen`: `localhost` is
    /// both loopback addresses, as OpenSSH's sshd listens there, and an
    /// empty address both of every address. Returns the port.
    fn listen(&mut self, listen: u32, address: &str, port: u16) -> Result<u16, String> {
        if !listen.is_multiple_of(2) || self.listens.contains_key(&listen) {
            return Err(format!("listen {listen} is not one sshd may ask for"));
        }
        if self.listens.len() >= LISTENS_MAX {
            return Err(format!("{LISTENS_MAX} ports are forwarded already"));
        }
        let (first, second): (IpAddr, Option<IpAddr>) = match address {
            "localhost" => (Ipv4Addr::LOCALHOST.into(), Some(Ipv6Addr::LOCALHOST.into())),
            "" => (Ipv4Addr::UNSPECIFIED.into(), Some(Ipv6Addr::UNSPECIFIED.into())),
            address => {
                let ip = address.parse().map_err(|_| {
                    format!("cannot listen at {address}: a port is listened on at an address")
                })?;
                (ip, None)
            }
        };
        let shown = if address.is_empty() { "every address" } else { address };
        let first = bind(first, port)
            .map_err(|err| format!("cannot listen at {shown} port {port}: {err}"))?;
        // Port 0 has the system choose: the other address takes the same.
        let port = first.local_addr().map_or(port, |address| address.port());
        let mut listeners = vec![first];
        if let Some(second) = second
            && let Ok(listener) = bind(second, port)
        {
            listeners.push(listener);
        }
        let tasks = listeners
            .into_iter()
            .map(|listener| {
                tokio::spawn(accept(listener, listen, self.events.clone())).abort_handle()
            })
            .collect();
        self.listens.insert(listen, tasks);
        Ok(port)
    }

    fn accepted(&mut self, listen: u32, socket: TcpStream, peer: SocketAddr) {
        // A port no longer listened on, or too many at once: the
        // connection closes.
        if !self.listens.contains_key(&listen) || self.streams.len() >= STREAMS_MAX {
            return;
        }
        let mut stream = self.next;
        while self.streams.contains_key(&stream) {
            stream = stream.wrapping_add(2);
        }
        self.next = stream.wrapping_add(2);
        self.open(stream, socket);
        let address = peer.ip().to_canonical().to_string();
        self.send(&Frame::Accepted { listen, stream, address, port: peer.port() });
    }

    /// Passes the bytes of stream `stream`, on `socket`: none are read
    /// until sshd gives room.
    fn open(&mut self, stream: u32, socket: TcpStream) {
        let (read, write) = socket.into_split();
        let room = Arc::new(Room::default());
        let queued = Arc::new(AtomicUsize::new(0));
        let (writes, to_write) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read_from(
            read,
            stream,
            Arc::clone(&room),
            self.out.clone(),
            self.events.clone(),
        ));
        let writer = tokio::spawn(write_to(
            write,
            stream,
            to_write,
            Arc::clone(&queued),
            self.out.clone(),
            self.events.clone(),
        ));
        let tasks = [reader.abort_handle(), writer.abort_handle()];
        self.streams.insert(stream, Stream::Open(Open { writes, queued, room, done: 0, tasks }));
    }

    fn half_done(&mut self, stream: u32) {
        match self.streams.get_mut(&stream) {
            Some(Stream::Open(open)) => {
                open.done += 1;
                if open.done == 2 {
                    self.close(stream);
                }
            }
            // Its writer wrote what came before sshd closed it.
            Some(Stream::Closing(_)) => {
                self.streams.remove(&stream);
            }
            _ => {}
        }
    }

    /// sshd closed stream `stream`: nothing more comes for it, and nothing
    /// is said back. Its connection is read no more, but what sshd sent
    /// before is written first, then its sending is shut down.
    fn closed(&mut self, stream: u32) {
        match self.streams.remove(&stream) {
            Some(Stream::Open(Open { tasks: [reader, writer], .. })) => {
                reader.abort();
                // With its queue's sender gone, the writer ends when the
                // queue is written; one that ended said so already.
                if !writer.is_finished() {
                    self.streams.insert(stream, Stream::Closing(writer));
                }
            }
            Some(other) => other.stop(),
            None => {}
        }
    }

    /// Closes stream `stream`, and tells sshd unless it closed it.
    fn close(&mut self, stream: u32) {
        match self.streams.remove(&stream) {
            Some(Stream::Closing(writer)) => writer.abort(),
            Some(open) => {
                open.stop();
                self.send(&Frame::Close { stream });
            }
            None => {}
        }
    }
}

/// A listener at `ip` and `port`.
fn bind(ip: IpAddr, port: u16) -> io::Result<TcpListener> {
    let listener = std::net::TcpListener::bind((ip, port))?;
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener)
}

/// Takes the connections that come to `listener`.
async fn accept(listener: TcpListener, listen: u32, events: mpsc::UnboundedSender<Event>) {
    loop {
        match listener.accept().await {
            Ok((socket, peer)) => {
                if events.send(Event::Accepted { listen, socket, peer }).is_err() {
                    return;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
                ) => {}
            // Out of descriptors, most likely: a moment later there may be
            // some.
            Err(_) => tokio::time::sleep(ACCEPT_PAUSE).await,
        }
    }
}

/// Reads what comes on a stream's connection, as sshd gives room for it,
/// and sends it on.
async fn read_from(
    mut socket: OwnedReadHalf,
    stream: u32,
    room: Arc<Room>,
    out: Out,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut buffer = vec![0u8; PAYLOAD_MAX];
    loop {
        let may = room.wait().await;
        match socket.read(&mut buffer[..may]).await {
            Ok(0) => {
                send(&out, &Frame::Eof { stream });
                let _ = events.send(Event::HalfDone { stream });
                return;
            }
            Ok(n) => {
                room.take(n);
                send(&out, &Frame::Data { stream, bytes: buffer[..n].to_vec() });
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                let _ = events.send(Event::Broken { stream });
                return;
            }
        }
    }
}

/// Writes what the client sent to a stream's connection, and gives sshd
/// room for as much again; at its end, or when sshd closed the stream
/// and all it sent is written, shuts the connection's sending down.
async fn write_to(
    mut socket: OwnedWriteHalf,
    stream: u32,
    mut writes: mpsc::UnboundedReceiver<Write>,
    queued: Arc<AtomicUsize>,
    out: Out,
    events: mpsc::UnboundedSender<Event>,
) {
    while let Some(write) = writes.recv().await {
        match write {
            Write::Bytes(bytes) => {
                if socket.write_all(&bytes).await.is_err() {
                    let _ = events.send(Event::Broken { stream });
                    return;
                }
                queued.fetch_sub(bytes.len(), Ordering::Relaxed);
                let bytes = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                send(&out, &Frame::Credit { stream, bytes });
            }
            Write::End => break,
        }
    }
    let _ = socket.shutdown().await;
    let _ = events.send(Event::HalfDone { stream });
}

/// Writes the frames to sshd, as many as wait at once in one write.
async fn write_frames(
    mut to: pipe::Sender,
    mut frames: mpsc::UnboundedReceiver<Vec<u8>>,
    events: mpsc::UnboundedSender<Event>,
) {
    while let Some(mut batch) = frames.recv().await {
        while batch.len() < BATCH
            && let Ok(more) = frames.try_recv()
        {
            batch.extend_from_slice(&more);
        }
        if let Err(err) = to.write_all(&batch).await {
            let _ = events.send(Event::Ended(Some(format!("cannot write to sshd: {err}"))));
            return;
        }
    }
}

/// Reads sshd's frames, until it closes the pipe.
async fn read_frames(mut from: pipe::Receiver, events: mpsc::UnboundedSender<Event>) {
    let mut frames = Frames::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let ended = loop {
        match from.read(&mut buffer).await {
            Ok(0) if frames.partial() => break Some("sshd ended in the middle of a frame".into()),
            Ok(0) => break None,
            Ok(n) => {
                frames.push(&buffer[..n]);
                let malformed = loop {
                    match frames.pop() {
                        Ok(Some(frame)) => {
                            let _ = events.send(Event::Frame(frame));
                        }
                        Ok(None) => break None,
                        Err(malformed) => break Some(malformed),
                    }
                };
                if let Some(malformed) = malformed {
                    break Some(format!("sshd's frames do not read: {malformed}"));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => break Some(format!("cannot read from sshd: {err}")),
        }
    };
    let _ = events.send(Event::Ended(ended));
}
