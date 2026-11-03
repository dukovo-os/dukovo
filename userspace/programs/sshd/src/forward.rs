// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A connection's forwarding (ADR 0349), as sshd holds it: the client's
//! `direct-tcpip` channels, which sshd has the forwarder connect for, and
//! the ports it forwards from here with `tcpip-forward`, on which the
//! forwarder listens, and whose connections sshd opens `forwarded-tcpip`
//! channels to the client for (RFC 4254 7). sshd holds no network beyond
//! port 22: the forwarder, `sshd --forward`, which authd(8) starts as the
//! person who signed in, makes every connection, with the network their
//! session has.
//!
//! One task here hears the client's requests and the forwarder's frames,
//! and two pass each channel's bytes: one from the client to the
//! forwarder, one back, which waits for the channel's window, so that a
//! slow reader slows its own stream and no other. The forwarder runs as
//! the person, so what it sends is checked as what a client sends is: it
//! may send no more than it has room for, and nothing that was not asked.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use accounts::forwarding::{self, CREDIT, Frame, Frames};
use accounts::request::{Answer, Request, SshRun};
use russh::server::{ChannelOpenHandle, Handle, Msg};
use russh::{
    Channel, ChannelId, ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::unix::pipe;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::session::{self, SignedIn};
use crate::{authd, sys};

/// The most forwarded channels a connection has at once, of both kinds.
const STREAMS_MAX: usize = 128;
/// The most ports a connection forwards from here at once.
const LISTENS_MAX: usize = 16;
/// How many bytes of frames go to the forwarder in one write, at most.
const BATCH: usize = 256 * 1024;
/// The longest line the forwarder logs.
const LINE_MAX: u64 = 512;

/// What the forwarding's task hears.
enum Command {
    /// The client asked for a connection to `host` and `port` on
    /// `channel`.
    Connect { channel: Channel<Msg>, reply: ChannelOpenHandle, host: String, port: u16 },
    /// The client asked for a port here, at `asked`, which listens at
    /// `address`.
    Listen {
        asked: String,
        address: String,
        port: u16,
        answer: oneshot::Sender<Result<u16, String>>,
    },
    /// The client no longer wants the port it asked for at `asked` and
    /// `port`.
    Unlisten { asked: String, port: u32, answer: oneshot::Sender<bool> },
    /// A frame from the forwarder.
    Frame(Frame),
    /// The forwarder's output ended, or does not read.
    Ended(String),
    /// The client took the channel opened for an accepted connection, or
    /// not.
    Opened { stream: u32, channel: Option<Channel<Msg>> },
    /// A stream's bytes were passed to the client.
    Took { stream: u32, bytes: usize },
    /// The client closed a stream's channel.
    Closed { stream: u32 },
}

/// What comes for the client on a stream.
enum Event {
    Data(Vec<u8>),
    Eof,
    Close,
}

/// The forwarding of a connection, once its client forwarded anything.
pub struct Forwarding {
    commands: mpsc::UnboundedSender<Command>,
    /// The forwarder's process.
    pub process: String,
    /// The task that hears it all.
    hears: AbortHandle,
    /// Those that pass its frames and its log.
    tasks: [AbortHandle; 3],
}

impl Drop for Forwarding {
    fn drop(&mut self) {
        self.hears.abort();
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// `fd` read or written without blocking: tokio's pipes, as tokio's
/// AsyncFd waits on Radix for one event only.
fn reader(fd: OwnedFd) -> io::Result<pipe::Receiver> {
    sys::set_nonblocking(&fd).and_then(|()| pipe::Receiver::from_owned_fd_unchecked(fd))
}

fn writer(fd: OwnedFd) -> io::Result<pipe::Sender> {
    sys::set_nonblocking(&fd).and_then(|()| pipe::Sender::from_owned_fd_unchecked(fd))
}

impl Forwarding {
    /// Has authd start the forwarder for `who`, on `connection`, whose
    /// client is `from`, and passes its frames for the connection whose
    /// handle is `handle`.
    pub async fn start(
        handle: Handle,
        who: &SignedIn,
        connection: &str,
        from: &str,
    ) -> Result<Forwarding, String> {
        let failed = |err: io::Error| format!("cannot make the forwarder's pipes: {err}");
        let (input, to_input) = sys::pipe().map_err(failed)?;
        let (from_output, output) = sys::pipe().map_err(failed)?;
        let (from_error, error) = sys::pipe().map_err(failed)?;
        let request = Request::SshSession {
            connection: connection.into(),
            name: who.name.clone(),
            proof: who.proof.clone(),
            term: String::new(),
            run: SshRun::Forward,
            language: Vec::new(),
        };
        let given = [input, output, error];
        let fds: Vec<RawFd> = given.iter().map(AsRawFd::as_raw_fd).collect();
        let (answer, _) = authd::ask(&request, &fds).await?;
        // The forwarder holds its ends now.
        drop(given);
        let process = match answer {
            Answer::Done(process) => process,
            Answer::Refused(why) => return Err(why),
        };
        let to = writer(to_input).map_err(failed)?;
        let output = reader(from_output).map_err(failed)?;
        let log = reader(from_error).map_err(failed)?;
        let (commands, heard) = mpsc::unbounded_channel();
        let (frames, to_send) = mpsc::unbounded_channel();
        let who = format!("{} from {from}", who.name);
        let tasks = [
            tokio::spawn(write_frames(to, to_send)).abort_handle(),
            tokio::spawn(read_frames(output, commands.clone())).abort_handle(),
            tokio::spawn(read_log(log, who.clone())).abort_handle(),
        ];
        let state = State {
            handle,
            commands: commands.clone(),
            frames,
            who,
            next: 2,
            connecting: HashMap::new(),
            opening: HashMap::new(),
            streams: HashMap::new(),
            listens: HashMap::new(),
        };
        let hears = tokio::spawn(hear(state, heard)).abort_handle();
        Ok(Forwarding { commands, process, hears, tasks })
    }

    /// Whether the forwarder still runs.
    pub fn alive(&self) -> bool {
        !self.hears.is_finished()
    }

    /// Has the forwarder connect to `host` and `port` for the client's
    /// `channel`, which `reply` then takes or refuses.
    pub fn connect(
        &self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        host: String,
        port: u16,
    ) {
        // Should the task have ended, the reply goes with the command,
        // which refuses the channel.
        let _ = self.commands.send(Command::Connect { channel, reply, host, port });
    }

    /// Has the forwarder listen at `address` and `port` for the port the
    /// client asked for at `asked`; returns the port.
    pub async fn listen(&self, asked: String, address: String, port: u16) -> Result<u16, String> {
        let (answer, answered) = oneshot::channel();
        let _ = self.commands.send(Command::Listen { asked, address, port, answer });
        answered.await.unwrap_or_else(|_| Err("the forwarder ended".into()))
    }

    /// Has the forwarder stop listening on the port the client asked for
    /// at `asked` and `port`; whether there was one.
    pub async fn unlisten(&self, asked: String, port: u32) -> bool {
        let (answer, answered) = oneshot::channel();
        let _ = self.commands.send(Command::Unlisten { asked, port, answer });
        answered.await.unwrap_or(false)
    }
}

/// A connection the forwarder is making.
struct Connecting {
    channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    /// Where, for the log.
    to: String,
}

/// A port the client forwards from here.
struct Listen {
    /// The address it asked for, which the client knows it by.
    asked: String,
    /// The port, once it listens.
    port: u16,
    /// Who waits for it to listen.
    answer: Option<oneshot::Sender<Result<u16, String>>>,
}

/// A stream whose channel is open.
struct Stream {
    id: ChannelId,
    events: mpsc::UnboundedSender<Event>,
    /// How many bytes the forwarder may send on it.
    room: u64,
    tasks: [AbortHandle; 2],
}

/// What the forwarding's task holds.
struct State {
    handle: Handle,
    commands: mpsc::UnboundedSender<Command>,
    frames: mpsc::UnboundedSender<Vec<u8>>,
    /// Whose forwarding, for the log.
    who: String,
    /// The number the next stream or listen sshd asks for gets: even.
    next: u32,
    connecting: HashMap<u32, Connecting>,
    /// The streams of accepted connections whose channels the client has
    /// not taken yet, with what came for them meanwhile.
    opening: HashMap<u32, Vec<Event>>,
    streams: HashMap<u32, Stream>,
    listens: HashMap<u32, Listen>,
}

/// Hears the client's requests and the forwarder's frames until the
/// forwarder ends, or breaks its protocol; then every forwarded channel
/// closes.
async fn hear(mut state: State, mut heard: mpsc::UnboundedReceiver<Command>) {
    while let Some(command) = heard.recv().await {
        if let Err(why) = state.command(command).await {
            eprintln!("sshd: forwarding for {} ended: {why}", state.who);
            break;
        }
    }
    state.shut();
}

impl State {
    fn send(&self, frame: &Frame) {
        let mut bytes = Vec::new();
        frame.encode(&mut bytes);
        let _ = self.frames.send(bytes);
    }

    /// A number for a stream or listen, which no other has.
    fn number(&mut self) -> u32 {
        let mut number = self.next;
        while self.known(number) {
            number = number.wrapping_add(2);
        }
        self.next = number.wrapping_add(2);
        number
    }

    fn known(&self, number: u32) -> bool {
        self.connecting.contains_key(&number)
            || self.opening.contains_key(&number)
            || self.streams.contains_key(&number)
            || self.listens.contains_key(&number)
    }

    fn channels(&self) -> usize {
        self.connecting.len() + self.opening.len() + self.streams.len()
    }

    async fn command(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::Connect { channel, reply, host, port } => {
                if self.channels() >= STREAMS_MAX {
                    eprintln!(
                        "sshd: refused a forwarded connection for {}: {STREAMS_MAX} are open",
                        self.who
                    );
                    reply.reject(ChannelOpenFailure::ResourceShortage).await;
                    return Ok(());
                }
                let stream = self.number();
                let to = format!("{host} port {port}");
                self.send(&Frame::Connect { stream, host, port });
                self.connecting.insert(stream, Connecting { channel, reply, to });
            }
            Command::Listen { asked, address, port, answer } => {
                if self.listens.len() >= LISTENS_MAX {
                    let _ = answer.send(Err(format!("{LISTENS_MAX} ports are forwarded already")));
                    return Ok(());
                }
                let listen = self.number();
                self.send(&Frame::Listen { listen, address, port });
                self.listens.insert(listen, Listen { asked, port: 0, answer: Some(answer) });
            }
            Command::Unlisten { asked, port, answer } => {
                let found = self.listens.iter().find_map(|(&listen, l)| {
                    (l.answer.is_none() && l.asked == asked && u32::from(l.port) == port)
                        .then_some(listen)
                });
                if let Some(listen) = found {
                    self.listens.remove(&listen);
                    self.send(&Frame::Unlisten { listen });
                }
                let _ = answer.send(found.is_some());
            }
            Command::Frame(frame) => return self.frame(frame).await,
            Command::Ended(why) => return Err(why),
            Command::Opened { stream, channel } => {
                let Some(waiting) = self.opening.remove(&stream) else { return Ok(()) };
                let Some(channel) = channel else {
                    // The client refused it: the connection closes.
                    self.send(&Frame::Close { stream });
                    return Ok(());
                };
                self.open(stream, channel);
                let closed = waiting.iter().any(|event| matches!(event, Event::Close));
                for event in waiting {
                    self.pass(stream, event);
                }
                if !closed {
                    self.give(stream, CREDIT);
                }
            }
            Command::Took { stream, bytes } => {
                if let Some(open) = self.streams.get_mut(&stream) {
                    open.room += bytes as u64;
                    let bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
                    self.send(&Frame::Credit { stream, bytes });
                }
            }
            Command::Closed { stream } => {
                if let Some(open) = self.streams.remove(&stream) {
                    open.tasks.iter().for_each(AbortHandle::abort);
                    self.send(&Frame::Close { stream });
                }
            }
        }
        Ok(())
    }

    /// A frame from the forwarder: what it says of what sshd asked, and
    /// the bytes of streams.
    async fn frame(&mut self, frame: Frame) -> Result<(), String> {
        if !frame.from_forwarder() {
            return Err("the forwarder sent a frame of sshd's own".into());
        }
        match frame {
            Frame::Connected { stream } => {
                let Some(connecting) = self.connecting.remove(&stream) else {
                    return Err(format!("the forwarder connected stream {stream}, not asked for"));
                };
                self.open(stream, connecting.channel);
                connecting.reply.accept().await;
                self.give(stream, CREDIT);
            }
            Frame::Failed { id, why } => {
                if let Some(connecting) = self.connecting.remove(&id) {
                    eprintln!("sshd: cannot forward to {} for {}: {why}", connecting.to, self.who);
                    connecting.reply.reject(ChannelOpenFailure::ConnectFailed).await;
                } else if let Some(answer) =
                    self.listens.get_mut(&id).and_then(|listen| listen.answer.take())
                {
                    self.listens.remove(&id);
                    let _ = answer.send(Err(why));
                } else {
                    return Err(format!("the forwarder said {id} failed, not asked for"));
                }
            }
            Frame::Listening { listen, port } => {
                let Some(answer) = self.listens.get_mut(&listen).and_then(|l| {
                    l.port = port;
                    l.answer.take()
                }) else {
                    return Err(format!("the forwarder listens for {listen}, not asked for"));
                };
                if answer.send(Ok(port)).is_err() {
                    // Asked for too long ago: the client was told no.
                    self.listens.remove(&listen);
                    self.send(&Frame::Unlisten { listen });
                }
            }
            Frame::Accepted { listen, stream, address, port } => {
                if stream.is_multiple_of(2) || self.known(stream) {
                    return Err(format!("the forwarder took a connection as stream {stream}"));
                }
                let Some(asked) = self
                    .listens
                    .get(&listen)
                    .filter(|l| l.answer.is_none())
                    .map(|l| (l.asked.clone(), l.port))
                else {
                    // Taken as the port stopped listening.
                    self.send(&Frame::Close { stream });
                    return Ok(());
                };
                if self.channels() >= STREAMS_MAX {
                    self.send(&Frame::Close { stream });
                    return Ok(());
                }
                self.opening.insert(stream, Vec::new());
                let (handle, commands) = (self.handle.clone(), self.commands.clone());
                tokio::spawn(async move {
                    let (asked, listening) = asked;
                    let channel = handle
                        .channel_open_forwarded_tcpip(
                            asked,
                            u32::from(listening),
                            address,
                            u32::from(port),
                        )
                        .await
                        .ok();
                    let _ = commands.send(Command::Opened { stream, channel });
                });
            }
            Frame::Data { stream, bytes } => {
                if let Some(open) = self.streams.get_mut(&stream) {
                    if bytes.len() as u64 > open.room {
                        return Err(format!(
                            "the forwarder sent more on {stream} than it had room for"
                        ));
                    }
                    open.room -= bytes.len() as u64;
                    let _ = open.events.send(Event::Data(bytes));
                } else if self.opening.contains_key(&stream)
                    || self.connecting.contains_key(&stream)
                {
                    return Err(format!("the forwarder sent on {stream} with no room"));
                }
                // On a stream that closed, it is passed over.
            }
            Frame::Eof { stream } => self.pass(stream, Event::Eof),
            Frame::Close { stream } => self.pass(stream, Event::Close),
            Frame::Credit { stream, bytes } => {
                if let Some(open) = self.streams.get(&stream) {
                    session::give_room(&self.handle, open.id, bytes as usize);
                }
            }
            Frame::Connect { .. } | Frame::Listen { .. } | Frame::Unlisten { .. } => {}
        }
        Ok(())
    }

    /// Passes `event` on to stream `stream`'s client, or keeps it while
    /// its channel opens. A closed stream is forgotten.
    fn pass(&mut self, stream: u32, event: Event) {
        let close = matches!(event, Event::Close);
        if let Some(waiting) = self.opening.get_mut(&stream) {
            waiting.push(event);
        } else if let Some(open) = self.streams.get(&stream) {
            let _ = open.events.send(event);
            if close {
                // Its channel closes once what came before is sent; the
                // client's last bytes on it are passed over.
                self.streams.remove(&stream);
            }
        }
    }

    /// Gives the forwarder room for `bytes` on `stream`.
    fn give(&mut self, stream: u32, bytes: u32) {
        if let Some(open) = self.streams.get_mut(&stream) {
            open.room += u64::from(bytes);
            self.send(&Frame::Credit { stream, bytes });
        }
    }

    /// Passes stream `stream`'s bytes between the forwarder and the
    /// client on `channel`.
    fn open(&mut self, stream: u32, channel: Channel<Msg>) {
        let id = channel.id();
        let (read, write) = channel.split();
        let (events, to_pass) = mpsc::unbounded_channel();
        let from =
            tokio::spawn(from_client(read, stream, self.frames.clone(), self.commands.clone()));
        let to = tokio::spawn(to_client(write, stream, to_pass, self.commands.clone()));
        let tasks = [from.abort_handle(), to.abort_handle()];
        self.streams.insert(stream, Stream { id, events, room: 0, tasks });
    }

    /// The forwarder ended: every forwarded channel closes, and the ports
    /// stop listening with it.
    fn shut(&mut self) {
        for (_, open) in self.streams.drain() {
            open.tasks.iter().for_each(AbortHandle::abort);
            let handle = self.handle.clone();
            tokio::spawn(async move {
                let _ = handle.close(open.id).await;
            });
        }
        // Their replies, as they go, refuse the channels still connecting,
        // and those still opening close as they open.
        self.connecting.clear();
        self.opening.clear();
        if !self.listens.is_empty() {
            eprintln!(
                "sshd: the ports forwarded for {} no longer listen: the forwarder ended",
                self.who
            );
        }
        self.listens.clear();
    }
}

/// Passes what the client sends on a stream's channel to the forwarder,
/// until it closes the channel.
async fn from_client(
    mut read: ChannelReadHalf,
    stream: u32,
    frames: mpsc::UnboundedSender<Vec<u8>>,
    commands: mpsc::UnboundedSender<Command>,
) {
    while let Some(message) = read.wait().await {
        let mut bytes = Vec::new();
        match message {
            ChannelMsg::Data { data } => forwarding::data(stream, &data, &mut bytes),
            ChannelMsg::Eof => Frame::Eof { stream }.encode(&mut bytes),
            ChannelMsg::Close => break,
            // Extended data means nothing to a connection: the client is
            // given room for it as it comes (connection.rs).
            _ => continue,
        }
        if frames.send(bytes).is_err() {
            return;
        }
    }
    let _ = commands.send(Command::Closed { stream });
}

/// Passes what comes on a stream to the client as the channel's window
/// lets it, says how much went, and closes the channel at the end.
async fn to_client(
    write: ChannelWriteHalf<Msg>,
    stream: u32,
    mut events: mpsc::UnboundedReceiver<Event>,
    commands: mpsc::UnboundedSender<Command>,
) {
    while let Some(event) = events.recv().await {
        match event {
            Event::Data(bytes) => {
                let length = bytes.len();
                if write.data_bytes(bytes).await.is_err() {
                    return;
                }
                let _ = commands.send(Command::Took { stream, bytes: length });
            }
            Event::Eof => {
                let _ = write.eof().await;
            }
            Event::Close => {
                let _ = write.close().await;
                return;
            }
        }
    }
}

/// Writes the frames to the forwarder, as many as wait at once in one
/// write.
async fn write_frames(mut to: pipe::Sender, mut frames: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(mut batch) = frames.recv().await {
        while batch.len() < BATCH
            && let Ok(more) = frames.try_recv()
        {
            batch.extend_from_slice(&more);
        }
        // The forwarder's end is heard where its frames are read.
        if session::write_all(&mut to, &batch).await.is_err() {
            return;
        }
    }
}

/// Reads the forwarder's frames until it ends.
async fn read_frames(mut from: pipe::Receiver, commands: mpsc::UnboundedSender<Command>) {
    let mut frames = Frames::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let why = loop {
        match from.read(&mut buffer).await {
            Ok(0) => break "the forwarder ended".to_string(),
            Ok(n) => {
                frames.push(&buffer[..n]);
                let malformed = loop {
                    match frames.pop() {
                        Ok(Some(frame)) => {
                            let _ = commands.send(Command::Frame(frame));
                        }
                        Ok(None) => break None,
                        Err(malformed) => break Some(malformed),
                    }
                };
                if let Some(malformed) = malformed {
                    break format!("the forwarder's frames do not read: {malformed}");
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => break format!("cannot read the forwarder's frames: {err}"),
        }
    };
    let _ = commands.send(Command::Ended(why));
}

/// Logs what the forwarder says, a line at a time.
async fn read_log(from: pipe::Receiver, who: String) {
    let mut from = BufReader::new(from);
    loop {
        let mut line = Vec::new();
        match (&mut from).take(LINE_MAX).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&line);
                let line: String = line.trim_end().chars().filter(|c| !c.is_control()).collect();
                eprintln!("sshd: forwarding for {who}: {line}");
            }
        }
    }
}
