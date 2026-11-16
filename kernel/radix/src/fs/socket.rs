// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sockets of the AF_UNIX family: POSIX's stream sockets (Phase 9) and
//! datagram sockets (Phase 16, ADR 0525). Two connected stream sockets
//! (radix-process(2)'s `socket_pair`), as `socketpair` makes; and one
//! alone (`socket`), which can be given a name in the tree
//! (radix-directory(2)'s `bind_socket`), connect to a name
//! (`connect_socket`), or listen (radix-file(2)'s `listen`) and take the
//! connections made to its name (`accept`).
//!
//! A connection is two pipes ([`Pipe`]), one each way. Each [`End`] reads
//! one and writes the other, and counts as that pipe's reader and this
//! one's writer, so the pipes' rules carry over: a read waits for data,
//! and returns 0 once the other end is gone or shut for writing; a write
//! waits for room, and once the other end is gone or shut for reading
//! fails with gone, raising SIGPIPE unless the send asked not to. Shutting
//! an end down lets go of its part of one pipe or both early, once for
//! every description of the end, as POSIX's `shutdown` does. Files sent
//! with `send_files` ride on the pipe's bytes (`pipe`).
//!
//! A [`Socket`] is a node on the pipes' detached filesystem, so its
//! descriptions are copied, passed and inherited as a pipe's are (ADR
//! 0085). A stream socket starts fresh and moves on once: to listening,
//! or to connected, when it gets an end. Its name is a node of the tree
//! that holds a [`Binding`], through which a connecting socket finds the
//! socket's [`Listener`] while it listens: the connection's pipes are made
//! then, and the listening side's end waits in the listener's queue until
//! `accept` gives it a socket of its own.
//!
//! A datagram socket (`datagram_socket`, `datagram_pair`) has a queue of
//! its own instead, where what is sent to its name, or by the socket it
//! is connected to, waits: [`datagram`].
//!
//! Unix sockets may be sent among the files a socket of either type
//! sends, under a rule that keeps one from coming to hold itself open
//! through its own queues, which no process could then close: [`flight`].
//! Network sockets are `inet`'s.

use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloc::sync::Weak;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::file::{BACKLOG_MAX, SOCKET_NAME_MAX, SocketOption};
use radix_abi::network::SocketType;
use radix_abi::process::PipeFlags;
use radix_cap::{ObjectKind, Rights, SlotRef};
use radix_mm::Kmalloc;
use radix_sync::{Mutex, Once, SpinLock};
use radix_vfs::mount::{Location, Vfs};
use radix_vfs::{In, KArc, Kind, Node, NodeRef, Out, SocketBinding, Stat, mem, next_id};

use self::datagram::{Datagram, Inbox};
use self::flight::Queue;
use super::pipe::{self, Files, Pipe};
use super::{Description, FileObject, FileRef};
use crate::sched::{self, wait};
use crate::user::Fifo;
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

mod datagram;
mod flight;

/// A socket's name: the path `bind_socket` was given, as the program gave
/// it to `bind`, of at most SOCKET_NAME_MAX bytes; empty for none.
#[derive(Clone, Copy)]
pub struct Name {
    len: u8,
    bytes: [u8; SOCKET_NAME_MAX as usize],
}

impl Name {
    /// No name.
    pub const NONE: Name = Name { len: 0, bytes: [0; SOCKET_NAME_MAX as usize] };

    /// `bytes` as a name, unless it is too long for one.
    pub fn of(bytes: &[u8]) -> Option<Name> {
        let mut name = Name::NONE;
        name.bytes.get_mut(..bytes.len())?.copy_from_slice(bytes);
        name.len = bytes.len() as u8;
        Some(name)
    }

    /// Its bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

/// One end of a connection.
pub struct End {
    /// The pipe it reads, which the other end writes.
    rx: KArc<Pipe>,
    /// The pipe it writes, which the other end reads.
    tx: KArc<Pipe>,
    /// Whether it has been shut for reading, and for writing: its part of
    /// `rx` or `tx` has gone.
    shut_read: AtomicBool,
    shut_write: AtomicBool,
    /// The name of the socket at the other end, and its process, as they
    /// were when the connection was made.
    peer_name: Name,
    peer: Peer,
}

/// The process at one end of a connection, as it was when the connection
/// was made, or when its socket began to listen: what `socket_state`
/// gives, and POSIX's SO_PEERCRED.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its number.
    pub process: u64,
    /// The user it acted for (ADR 0214).
    pub user: u32,
    /// That user's group.
    pub group: u32,
}

impl Peer {
    /// No process: what a socket gives that has none to give.
    pub const NONE: Peer = Peer { process: 0, user: 0, group: 0 };

    /// `process` as it is now.
    pub fn of(process: &crate::user::process::ProcessObject) -> Peer {
        let identity = process.identity();
        Peer { process: process.number(), user: identity.user, group: identity.group }
    }
}

/// What a receive took.
pub struct Received {
    /// How many bytes came; and how many the datagram held, which is
    /// more when the buffer had no room for the rest, which is lost (a
    /// stream's are the same).
    pub count: usize,
    pub size: usize,
    /// The files that came with them.
    pub files: Option<Files>,
    /// The name of the socket they came from, and its process: a
    /// datagram's sender as it was at the send, or the other end of a
    /// connection as it was when the connection was made.
    pub from: Name,
    pub sender: Peer,
}

impl Received {
    /// Nothing, from no one: a receive at the end.
    const NOTHING: Received =
        Received { count: 0, size: 0, files: None, from: Name::NONE, sender: Peer::NONE };

    /// Closes the files that came, for a receive that does not take
    /// them, with no lock held: a Unix socket among them goes now, not
    /// at the store's next change (`pipe::Passed`).
    pub fn close_files(&mut self) {
        let files = self.files.take();
        if files.is_some() {
            drop(files);
            super::drop_deferred();
        }
    }
}

impl End {
    /// The ends of a new connection: the first's peer is `first_peer`,
    /// a name and a process, and the second's `second_peer`.
    fn pair(first_peer: (Name, Peer), second_peer: (Name, Peer)) -> Result<(End, End), Status> {
        // Each pipe starts with one reader and one writer, which the ends
        // are from here.
        let (one, other) = (Pipe::new()?, Pipe::new()?);
        let end = |rx: &KArc<Pipe>, tx: &KArc<Pipe>, (peer_name, peer)| End {
            rx: rx.clone(),
            tx: tx.clone(),
            shut_read: AtomicBool::new(false),
            shut_write: AtomicBool::new(false),
            peer_name,
            peer,
        };
        Ok((end(&one, &other, first_peer), end(&other, &one, second_peer)))
    }

    /// Receives up to `out.len()` bytes into `out`, waiting for some
    /// unless `non_blocking`, with the files that came with them; returns
    /// 0 once the other end has stopped writing and all it wrote has been
    /// read. With `peek`, the bytes and files stay for the next receive,
    /// and copies of the files come (MSG_PEEK, ADR 0341).
    pub fn receive(
        &self,
        out: &mut dyn Out,
        non_blocking: bool,
        peek: bool,
    ) -> Result<(usize, Option<Files>), Status> {
        if self.shut_read.load(Ordering::Acquire) {
            return Ok((0, None));
        }
        if peek {
            return self.rx.peek_files(out, non_blocking);
        }
        self.rx.read_files(out, non_blocking)
    }

    /// Sends `data`, with `files` riding on its first byte, waiting for
    /// room unless `non_blocking`; fails with gone, after SIGPIPE if
    /// `signal`, once the other end has stopped reading or this one was
    /// shut for writing.
    pub fn send(
        &self,
        data: &dyn In,
        files: Option<Files>,
        non_blocking: bool,
        signal: bool,
    ) -> Result<usize, Status> {
        if self.shut_write.load(Ordering::Acquire) {
            return pipe::broken(0, signal);
        }
        self.tx.write_files(data, files, non_blocking, signal)
    }

    /// Shuts it for reading, for writing, or both: POSIX's `shutdown`.
    pub fn shutdown(&self, read: bool, write: bool) {
        if read && !self.shut_read.swap(true, Ordering::AcqRel) {
            self.rx.close_reader();
        }
        if write && !self.shut_write.swap(true, Ordering::AcqRel) {
            self.tx.close_writer();
        }
    }

    /// How many bytes wait to be received.
    pub fn pending(&self) -> usize {
        self.rx.len()
    }

    /// The events `poll` finds on it, as Linux's for a stream socket: it
    /// is readable while data waits or once a read would return the end,
    /// writable while [`pipe::ATOMIC`] bytes fit or once a write would
    /// fail, and hung up once nothing can come or go either way.
    pub fn events(&self) -> u16 {
        let read_done = self.shut_read.load(Ordering::Acquire) || self.rx.writers() == 0;
        let write_done = self.shut_write.load(Ordering::Acquire) || self.tx.readers() == 0;
        let mut events = 0;
        if self.rx.len() > 0 || read_done {
            events |= poll::READABLE;
        }
        if self.tx.has_room() || write_done {
            events |= poll::WRITABLE;
        }
        if read_done && write_done {
            events |= poll::HANG_UP;
        }
        events
    }
}

impl Drop for End {
    fn drop(&mut self) {
        self.shutdown(true, true);
    }
}

/// What takes a listening socket's connections.
pub struct Listener {
    /// The listening sides of the connections that wait for `accept`.
    queue: Mutex<Fifo<End>>,
    /// How many wait, as the queue says, for the waits.
    waiting: AtomicUsize,
    /// How many may wait at once.
    backlog: AtomicUsize,
    /// Whether the socket has gone.
    closed: AtomicBool,
    /// The socket's name, and its process, as they were when it began to
    /// listen.
    name: Name,
    process: Peer,
    /// Its pollers.
    polled: Polled,
}

impl Listener {
    fn key(&self) -> usize {
        core::ptr::from_ref::<Listener>(self) as usize
    }

    /// Something changed: the waits look again, and so do its pollers.
    fn changed(&self) {
        wait::wake_all(self.key());
        self.polled.changed();
    }

    /// Queues `end`, the listening side of a new connection, for
    /// `accept`, waiting for room unless `non_blocking`; fails with
    /// refused once the socket has gone. An end that does not go in goes
    /// once the queue is unlocked, as its pipes take their own locks.
    fn enqueue(&self, end: End, non_blocking: bool) -> Result<(), Status> {
        loop {
            {
                let mut queue = self.queue.lock();
                if self.closed.load(Ordering::Acquire) {
                    drop(queue);
                    return Err(Kernel::Refused.status(0));
                }
                if queue.len() < self.backlog.load(Ordering::Acquire) {
                    if let Err(lost) = queue.push_back(end) {
                        drop(queue);
                        drop(lost);
                        return Err(Kernel::NoMemory.status(0));
                    }
                    self.waiting.store(queue.len(), Ordering::Release);
                    drop(queue);
                    self.changed();
                    return Ok(());
                }
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            wait::wait_until(self.key(), "socket connect", || {
                self.waiting.load(Ordering::Acquire) < self.backlog.load(Ordering::Acquire)
                    || self.closed.load(Ordering::Acquire)
                    || sched::interrupted()
            });
            if sched::interrupted() {
                return Err(interrupted(Restart::Always));
            }
        }
    }

    /// Takes the connection that has waited longest, waiting for one
    /// unless `non_blocking`.
    fn accept(&self, non_blocking: bool) -> Result<End, Status> {
        loop {
            {
                let mut queue = self.queue.lock();
                if let Some(end) = queue.pop_front() {
                    self.waiting.store(queue.len(), Ordering::Release);
                    drop(queue);
                    self.changed();
                    return Ok(end);
                }
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            wait::wait_until(self.key(), "socket accept", || {
                self.waiting.load(Ordering::Acquire) > 0 || sched::interrupted()
            });
            if sched::interrupted() && self.waiting.load(Ordering::Acquire) == 0 {
                return Err(interrupted(Restart::Always));
            }
        }
    }

    /// Puts `end`, which an accept took but could not give a socket, back
    /// at the front of the queue for the next accept. It goes instead if
    /// the socket has gone meanwhile, if there is no memory to keep it, or
    /// if a Unix socket rides in the pipe it reads: while it was out, no
    /// socket kept that pipe, so sockets sent meanwhile were checked
    /// without it, and its coming back could close a cycle (`flight`).
    fn give_back(&self, end: End) {
        let lost = {
            // Nothing rides into the pipe while the rule's lock is held.
            let _sending = flight::hold();
            let mut queue = self.queue.lock();
            if self.closed.load(Ordering::Acquire) || end.rx.sockets() > 0 {
                Some(end)
            } else {
                let lost = queue.push_front(end).err();
                self.waiting.store(queue.len(), Ordering::Release);
                lost
            }
        };
        // Outside the locks, as the other end then sees the end; a Unix
        // socket riding in it goes now (`pipe::Passed`).
        if lost.is_some() {
            drop(lost);
            super::drop_deferred();
        }
        self.changed();
    }

    /// The socket has gone: the connections that wait go too, and the
    /// sockets that connected see the end.
    fn close(&self) {
        let waiting = {
            let mut queue = self.queue.lock();
            self.closed.store(true, Ordering::Release);
            self.waiting.store(0, Ordering::Release);
            queue.take()
        };
        drop(waiting);
        self.changed();
    }
}

/// What a socket's name holds, through which a socket that connects or
/// sends to the name finds the socket.
pub struct Binding {
    /// Whether the socket is still there.
    alive: AtomicBool,
    /// A stream socket's listener, while it listens.
    listener: SpinLock<Option<KArc<Listener>>>,
    /// A datagram socket's inbox.
    inbox: Option<Weak<Inbox, Kmalloc>>,
    /// The socket's name, which a datagram socket that connects to it
    /// gives as its peer's.
    name: Name,
}

impl Binding {
    /// The binding `named`, a node of the tree, holds, if its socket is
    /// still there: refused for a name whose socket has gone and for any
    /// other file.
    fn of(named: &NodeRef) -> Result<KArc<Binding>, Status> {
        let refused = Kernel::Refused.status(0);
        let held = named.socket_binding().ok_or(refused)?;
        let binding = held.downcast::<Binding>().map_err(|_| refused)?;
        if !binding.alive.load(Ordering::Acquire) {
            return Err(refused);
        }
        Ok(binding)
    }
}

/// The inbox of the datagram socket named `named`, a node of the tree,
/// and its name: refused for a name whose socket has gone and for any
/// other file, wrong_type for a stream socket's name.
fn inbox_named(named: &NodeRef) -> Result<(KArc<Inbox>, Name), Status> {
    let binding = Binding::of(named)?;
    let inbox = binding.inbox.as_ref().ok_or(Kernel::WrongType.status(0))?;
    Ok((inbox.upgrade().ok_or(Kernel::Refused.status(0))?, binding.name))
}

/// Where a socket is in its life.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Neither listening nor connected.
    Fresh,
    /// Being named or connected, by a call under way.
    Busy,
    /// Made by `accept`, which is giving it its connection: it can be
    /// sent nowhere until it has it (`flight`).
    Accepting,
    /// Taking connections.
    Listening,
    /// Connected: it has an end.
    Connected,
}

/// What a socket keeps under its lock.
struct State {
    phase: Phase,
    /// Its name: the one it was given, or for one that `accept` made, the
    /// listening socket's.
    name: Name,
    /// What its name holds, once it was given one.
    binding: Option<KArc<Binding>>,
    /// While a stream socket connects, the pipe its end will read, which
    /// the other side can write to before the end is the socket's: the
    /// rule for sockets on their way counts it as the socket's
    /// (`flight`).
    connecting: Option<KArc<Pipe>>,
}

/// A socket, as a node.
pub struct Socket {
    /// Its number, which its node shows.
    id: u64,
    state: SpinLock<State>,
    /// A stream socket's end of a connection, once it is connected.
    end: Once<End>,
    /// What takes a stream socket's connections, once it listens.
    listener: Once<KArc<Listener>>,
    /// A datagram socket's part; none for a stream socket.
    datagram: Option<Datagram>,
    /// SO_PASSCRED: whether the program asked for the credentials of what
    /// it receives (`receive_unix`).
    pass_credentials: AtomicBool,
}

impl Socket {
    fn new(phase: Phase, name: Name, datagram: Option<Datagram>) -> Socket {
        Socket {
            id: next_id(),
            state: SpinLock::new(State { phase, name, binding: None, connecting: None }),
            end: Once::new(),
            listener: Once::new(),
            datagram,
            pass_credentials: AtomicBool::new(false),
        }
    }

    /// A connected stream socket, with `end`.
    fn connected_with(end: End, name: Name) -> Socket {
        let socket = Socket::new(Phase::Connected, name, None);
        // It is new, so its end is not set.
        let _ = socket.end.set(end);
        socket
    }

    /// Whether it is a datagram socket.
    pub fn is_datagram(&self) -> bool {
        self.datagram.is_some()
    }

    /// A stream socket's end of its connection; not_connected if it has
    /// none, as a datagram socket never has.
    fn connected(&self) -> Result<&End, Status> {
        self.end.get().ok_or(Kernel::NotConnected.status(0))
    }

    /// Receives up to `out.len()` bytes into `out`, waiting for some
    /// unless `non_blocking`, with the files that came with them: as
    /// [`End::receive`] does on a stream socket's connection, and the next
    /// datagram on a datagram socket, of which what `out` has no room for
    /// is lost. With `peek`, what came stays for the next receive, and
    /// copies of the files come.
    pub fn receive(
        &self,
        out: &mut dyn Out,
        non_blocking: bool,
        peek: bool,
    ) -> Result<Received, Status> {
        if let Some(datagram) = &self.datagram {
            return datagram.receive(out, non_blocking, peek);
        }
        let end = self.connected()?;
        let (count, files) = end.receive(out, non_blocking, peek)?;
        Ok(Received { count, size: count, files, from: end.peer_name, sender: end.peer })
    }

    /// Sends `data`, with `files`, for `sender`, waiting for room unless
    /// `non_blocking`: on a stream socket's connection as [`End::send`]
    /// does, and on a datagram socket as one datagram to the socket it is
    /// connected to. A Unix socket among `files` goes under the rule of
    /// [`flight`].
    pub fn send(
        &self,
        data: &dyn In,
        files: Option<Files>,
        sender: Peer,
        non_blocking: bool,
        signal: bool,
    ) -> Result<usize, Status> {
        if let Some(datagram) = &self.datagram {
            let from = self.state.lock().name;
            return datagram.send_to_peer(data, files, from, sender, non_blocking);
        }
        let end = self.connected()?;
        match files {
            Some(files) if pipe::sockets_among(&files) > 0 => {
                flight::send_stream(end, data, files, non_blocking, signal)
            }
            files => end.send(data, files, non_blocking, signal),
        }
    }

    /// Shuts it for reading, for writing, or both: POSIX's `shutdown`. A
    /// stream socket must be connected.
    pub fn shutdown(&self, read: bool, write: bool) -> Result<(), Status> {
        match &self.datagram {
            Some(datagram) => datagram.shutdown(read, write),
            None => {
                self.connected()?.shutdown(read, write);
                // The files that rode in the pipe it read have gone: a
                // Unix socket among them goes now (`pipe::Passed`).
                super::drop_deferred();
            }
        }
        Ok(())
    }

    /// How many bytes wait to be received: for a datagram socket, how many
    /// the next datagram holds.
    pub fn pending(&self) -> usize {
        match &self.datagram {
            Some(datagram) => datagram.pending(),
            None => self.end.get().map_or(0, End::pending),
        }
    }

    /// Whether it listens.
    pub fn listens(&self) -> bool {
        self.listener.get().is_some()
    }

    /// Whether it is connected: a stream socket once it has an end, a
    /// datagram socket while it has a peer.
    pub fn is_connected(&self) -> bool {
        match &self.datagram {
            Some(datagram) => datagram.is_connected(),
            None => self.end.get().is_some(),
        }
    }

    /// The process at the other end, as `socket_state` gives it
    /// (SO_PEERCRED): a stream connection's, as it was when the connection
    /// was made; for one of a pair of either type, the pair's maker; none
    /// otherwise.
    pub fn peer(&self) -> Peer {
        match &self.datagram {
            Some(datagram) => datagram.pair_maker(),
            None => self.end.get().map_or(Peer::NONE, |end| end.peer),
        }
    }

    /// Its name, or with `peer` the name of the socket it is connected to.
    pub fn name(&self, peer: bool) -> Result<Name, Status> {
        if !peer {
            return Ok(self.state.lock().name);
        }
        match &self.datagram {
            Some(datagram) => datagram.peer_name(),
            None => Ok(self.connected()?.peer_name),
        }
    }

    /// Has a datagram socket forget its peer (radix-file(2)'s
    /// `disconnect`); not_supported for a stream socket.
    pub fn disconnect(&self) -> Result<(), Status> {
        let datagram = self.datagram.as_ref().ok_or(Kernel::NotSupported.status(0))?;
        datagram.disconnect();
        Ok(())
    }

    /// Whether the program asked for the credentials of what it receives
    /// (SO_PASSCRED).
    pub fn passes_credentials(&self) -> bool {
        self.pass_credentials.load(Ordering::Acquire)
    }

    /// Reads, and with `set` changes, one of its options (radix-file(2)'s
    /// `socket_option`), returning it as it was: pass_credentials, and
    /// unix_kind, which is read only. The others are a network socket's,
    /// which fail with not_supported.
    pub fn option(&self, option: SocketOption, set: Option<u64>) -> Result<u64, Status> {
        match option {
            SocketOption::PassCredentials => {
                let old = match set {
                    Some(value) => self.pass_credentials.swap(value != 0, Ordering::AcqRel),
                    None => self.passes_credentials(),
                };
                Ok(u64::from(old))
            }
            SocketOption::UnixKind => {
                if set.is_some() {
                    return Err(Kernel::InvalidArgument.status(0));
                }
                let kind =
                    if self.is_datagram() { SocketType::Datagram } else { SocketType::Stream };
                Ok(u64::from(kind.raw()))
            }
            _ => Err(Kernel::NotSupported.status(0)),
        }
    }

    /// The events `poll` finds on it: a connected stream socket's are its
    /// end's, and a datagram socket's its own; a listening one is readable
    /// while a connection waits; a stream socket that is neither is
    /// writable and hung up, as on Linux.
    pub fn events(&self) -> u16 {
        if let Some(datagram) = &self.datagram {
            datagram.events()
        } else if let Some(end) = self.end.get() {
            end.events()
        } else if let Some(listener) = self.listener.get() {
            if listener.waiting.load(Ordering::Acquire) > 0 { poll::READABLE } else { 0 }
        } else {
            poll::WRITABLE | poll::HANG_UP
        }
    }

    /// Calls `each` with what its pollers count themselves in, as
    /// [`Socket::events`] looks: both pipes of its connection, its
    /// listener, or a datagram socket's own.
    pub fn polled(&self, each: &mut dyn FnMut(&Polled)) {
        if let Some(datagram) = &self.datagram {
            datagram.polled(each);
        } else if let Some(end) = self.end.get() {
            each(end.rx.polled());
            each(end.tx.polled());
        } else if let Some(listener) = self.listener.get() {
            each(&listener.polled);
        }
    }

    /// Calls `each` with each queue it receives from, and how many Unix
    /// sockets ride in it, for the rule of [`flight`]: a stream socket's
    /// pipe it reads, or will once it is connected; a listening one's
    /// those of the connections that wait for `accept`; a datagram
    /// socket's own queue. Returns false, having called nothing, for one
    /// that `accept` is giving its connection, which cannot say yet.
    fn each_kept(&self, each: &mut dyn FnMut(Queue, usize)) -> bool {
        if let Some(datagram) = &self.datagram {
            let inbox = datagram.inbox();
            each(Queue::inbox(inbox), inbox.sockets());
            return true;
        }
        {
            let state = self.state.lock();
            if state.phase == Phase::Accepting {
                return false;
            }
            // Looked at under the lock, without a reference of its own:
            // dropping one here could free the pipe.
            if let Some(rx) = &state.connecting {
                each(Queue::pipe(rx), rx.sockets());
            }
        }
        if let Some(end) = self.end.get() {
            each(Queue::pipe(&end.rx), end.rx.sockets());
        }
        if let Some(listener) = self.listener.get() {
            for end in listener.queue.lock().iter() {
                each(Queue::pipe(&end.rx), end.rx.sockets());
            }
        }
        true
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let binding = self.state.lock().binding.take();
        // No socket finds it through its name from here.
        if let Some(binding) = &binding {
            binding.alive.store(false, Ordering::Release);
            if self.listener.get().is_some() {
                *binding.listener.lock() = None;
            }
        }
        if let Some(listener) = self.listener.get() {
            listener.close();
        }
    }
}

impl Node for Socket {
    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::Socket,
            size: self.pending() as u64,
            allocated: 0,
            id: self.id,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::Socket
    }

    // Reads and writes through the node itself (the kernel's own) never
    // wait.
    fn read(&self, _offset: u64, out: &mut dyn Out) -> radix_vfs::Result<usize> {
        let mut received =
            self.receive(out, true, false).map_err(|_| radix_vfs::Error::NotSupported)?;
        received.close_files();
        Ok(received.count)
    }

    fn write(&self, _offset: u64, data: &dyn In) -> radix_vfs::Result<usize> {
        self.send(data, None, Peer::NONE, true, true).map_err(|_| radix_vfs::Error::NotSupported)
    }

    fn set_size(&self, _size: u64) -> radix_vfs::Result<()> {
        Err(radix_vfs::Error::NotSupported)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A description of `socket`, with `flags`, acting for `user`'s process's
/// name.
fn file(user: &crate::user::UserRef, socket: Socket, flags: PipeFlags) -> Result<FileRef, Status> {
    let mount = pipe::mount().map_err(super::status)?;
    let node: NodeRef = mem::arc(socket).map_err(|_| Kernel::NoMemory.status(0))?;
    let description = Description {
        non_blocking: flags.contains(PipeFlags::NON_BLOCKING),
        ..Description::default()
    };
    FileObject::with(Location::new(mount, node), description, user.process.identity())
        .ok_or(Kernel::NoMemory.status(0))
}

/// Puts a capability to `file`, a socket's description, in `user`'s slot
/// `slot`, closed by exec if `flags` say so.
fn install(
    user: &crate::user::UserRef,
    slot: u32,
    file: FileRef,
    flags: PipeFlags,
) -> Result<(), Status> {
    let rights = Rights::COPY.union(Rights::TRANSFER).union(Rights::READ).union(Rights::WRITE);
    crate::user::files::install_file(
        user,
        SlotRef::new(user.process.cspace(), slot),
        ObjectKind::File,
        rights,
        file,
        flags.contains(PipeFlags::CLOSE_ON_EXEC),
    )
}

/// Puts the two descriptions of a new pair in `user`'s slots `first` and
/// `second`, with `flags`, or neither.
fn install_pair(
    user: &crate::user::UserRef,
    (first, second): (u32, u32),
    (one, other): (FileRef, FileRef),
    flags: PipeFlags,
) -> Result<(), Status> {
    install(user, first, one, flags)?;
    if let Err(err) = install(user, second, other, flags) {
        let _ = crate::caps::delete(SlotRef::new(user.process.cspace(), first));
        return Err(err);
    }
    Ok(())
}

/// Makes a socket pair for `user`'s process: a description of one end in
/// slot `first` and of the other in slot `second`, with `flags`.
pub fn make_pair(
    user: &crate::user::UserRef,
    first: u32,
    second: u32,
    flags: PipeFlags,
) -> Result<(), Status> {
    if first == second {
        return Err(Kernel::InvalidArgument.status(1));
    }
    // Each end's peer is the process that made the pair.
    let process = Peer::of(&user.process);
    let (one, other) = End::pair((Name::NONE, process), (Name::NONE, process))?;
    // If a step fails, the sockets made go, and with them their ends.
    let one = file(user, Socket::connected_with(one, Name::NONE), flags)?;
    let other = file(user, Socket::connected_with(other, Name::NONE), flags)?;
    install_pair(user, (first, second), (one, other), flags)
}

/// Makes a socket for `user`'s process, neither named nor connected, in
/// slot `to`, with `flags`.
pub fn make(user: &crate::user::UserRef, to: u32, flags: PipeFlags) -> Result<(), Status> {
    install(user, to, file(user, Socket::new(Phase::Fresh, Name::NONE, None), flags)?, flags)
}

/// Makes a datagram socket for `user`'s process, neither named nor
/// connected, in slot `to`, with `flags` (radix-process(2)'s
/// `datagram_socket`).
pub fn make_datagram(user: &crate::user::UserRef, to: u32, flags: PipeFlags) -> Result<(), Status> {
    let socket = Socket::new(Phase::Fresh, Name::NONE, Some(Datagram::new()?));
    install(user, to, file(user, socket, flags)?, flags)
}

/// Makes a pair of datagram sockets for `user`'s process, each connected
/// to the other: one in slot `first` and the other in slot `second`,
/// with `flags` (radix-process(2)'s `datagram_pair`).
pub fn make_datagram_pair(
    user: &crate::user::UserRef,
    first: u32,
    second: u32,
    flags: PipeFlags,
) -> Result<(), Status> {
    if first == second {
        return Err(Kernel::InvalidArgument.status(1));
    }
    let (one, other) = Datagram::pair(Peer::of(&user.process))?;
    let one = file(user, Socket::new(Phase::Fresh, Name::NONE, Some(one)), flags)?;
    let other = file(user, Socket::new(Phase::Fresh, Name::NONE, Some(other)), flags)?;
    install_pair(user, (first, second), (one, other), flags)
}

/// Gives `socket` the name `path` beneath `dir` (radix-directory(2)'s
/// `bind_socket`), which it knows as `name`, with `access`.
pub fn bind(
    socket: &Socket,
    vfs: &Vfs,
    who: &radix_vfs::Who,
    dir: &Location,
    path: &str,
    name: Name,
    access: radix_vfs::Access,
) -> Result<(), Status> {
    {
        let mut state = socket.state.lock();
        match state.phase {
            Phase::Fresh if state.binding.is_none() => state.phase = Phase::Busy,
            Phase::Busy => return Err(Kernel::Busy.status(0)),
            // Named already, listening or connected.
            _ => return Err(Kernel::InvalidArgument.status(0)),
        }
    }
    let binding = Binding {
        alive: AtomicBool::new(true),
        listener: SpinLock::new(None),
        inbox: socket.datagram.as_ref().map(Datagram::downgrade),
        name,
    };
    let made = mem::arc(binding).map_err(|_| Kernel::NoMemory.status(0)).and_then(|binding| {
        let held: SocketBinding = binding.clone();
        vfs.make_socket(who, dir, path, held, access).map_err(|err| match err {
            // FAT has no sockets' names: Linux says EPERM.
            radix_vfs::Error::NotSupported => Kernel::NotPermitted.status(0),
            err => super::status(err),
        })?;
        Ok(binding)
    });
    let mut state = socket.state.lock();
    state.phase = Phase::Fresh;
    state.binding = Some(made?);
    state.name = name;
    Ok(())
}

/// Has `socket`, a stream socket which has a name and is not connected,
/// take connections, at most `backlog` waiting at once (radix-file(2)'s
/// `listen`), for the process `process`. A socket that listens already
/// takes the new backlog. A datagram socket fails with not_supported.
pub fn listen(socket: &Socket, backlog: u32, process: Peer) -> Result<(), Status> {
    if socket.is_datagram() {
        return Err(Kernel::NotSupported.status(0));
    }
    let backlog = u64::from(backlog).clamp(1, BACKLOG_MAX) as usize;
    if let Some(listener) = socket.listener.get() {
        listener.backlog.store(backlog, Ordering::Release);
        listener.changed();
        return Ok(());
    }
    // Made before the lock is taken, as making it may wait for memory.
    let name = socket.state.lock().name;
    let listener = mem::arc(Listener {
        queue: Mutex::new(Fifo::new()),
        waiting: AtomicUsize::new(0),
        backlog: AtomicUsize::new(backlog),
        closed: AtomicBool::new(false),
        name,
        process,
        polled: Polled::new(),
    })
    .map_err(|_| Kernel::NoMemory.status(0))?;
    let mut state = socket.state.lock();
    let binding = match (state.phase, &state.binding) {
        (Phase::Fresh, Some(binding)) => binding.clone(),
        (Phase::Busy, _) => return Err(Kernel::Busy.status(0)),
        // Another thread made it listen meanwhile.
        (Phase::Listening, _) => return Ok(()),
        // Connected, or with no name, as POSIX and Linux have it.
        _ => return Err(Kernel::InvalidArgument.status(0)),
    };
    state.phase = Phase::Listening;
    // It was fresh, so no listener was set.
    let _ = socket.listener.set(listener.clone());
    *binding.listener.lock() = Some(listener);
    Ok(())
}

/// Connects `socket` to the socket named `named`, a node of the tree
/// (radix-directory(2)'s `connect_socket`), for the process `process`. A
/// stream socket connects to one that listens, waiting for room in its
/// queue unless `non_blocking`; a datagram socket sends there from here
/// on, and takes datagrams from there alone (`Datagram::connect`).
pub fn connect(
    socket: &Socket,
    named: &NodeRef,
    process: Peer,
    non_blocking: bool,
) -> Result<(), Status> {
    if let Some(datagram) = &socket.datagram {
        let (target, name) = inbox_named(named)?;
        return datagram.connect(&target, name);
    }
    let name = {
        let mut state = socket.state.lock();
        match state.phase {
            Phase::Fresh => state.phase = Phase::Busy,
            Phase::Busy | Phase::Accepting => return Err(Kernel::Busy.status(0)),
            Phase::Connected => return Err(Kernel::Connected.status(0)),
            Phase::Listening => return Err(Kernel::InvalidArgument.status(0)),
        }
        state.name
    };
    let connected = connect_to(socket, named, name, process, non_blocking).map(|end| {
        // It is busy, so no other call set its end.
        let _ = socket.end.set(end);
    });
    let connecting = {
        let mut state = socket.state.lock();
        state.phase = if connected.is_ok() { Phase::Connected } else { Phase::Fresh };
        state.connecting.take()
    };
    // Outside the lock: it may be the last reference to the pipe.
    drop(connecting);
    connected
}

/// The end of a new connection from `socket`, named `name`, of the
/// process `process`, to the socket listening at `named`: refused for a
/// name whose socket does not listen, or has gone, and for any other
/// file; wrong_type for a datagram socket's name.
fn connect_to(
    socket: &Socket,
    named: &NodeRef,
    name: Name,
    process: Peer,
    non_blocking: bool,
) -> Result<End, Status> {
    let binding = Binding::of(named)?;
    if binding.inbox.is_some() {
        return Err(Kernel::WrongType.status(0));
    }
    let listener = binding.listener.lock().clone().ok_or(Kernel::Refused.status(0))?;
    let (end, theirs) = End::pair((listener.name, listener.process), (name, process))?;
    // Once the other side is queued, it can be accepted and write to the
    // pipe this end reads.
    socket.state.lock().connecting = Some(end.rx.clone());
    listener.enqueue(theirs, non_blocking)?;
    Ok(end)
}

/// Takes a connection that waits at `socket`, which listens, waiting for
/// one unless `non_blocking`, and puts a description of a new socket,
/// connected, with the listening one's name, in `user`'s slot `to`, with
/// `flags`. A connection that cannot go in the slot, as another thread
/// filled it first, goes back to the front of the queue, so none is lost,
/// unless a Unix socket rides in it (`Listener::give_back`). A datagram
/// socket fails with not_supported.
pub fn accept(
    user: &crate::user::UserRef,
    socket: &Socket,
    to: u32,
    flags: PipeFlags,
    non_blocking: bool,
) -> Result<(), Status> {
    if socket.is_datagram() {
        return Err(Kernel::NotSupported.status(0));
    }
    let listener = socket.listener.get().ok_or(Kernel::InvalidArgument.status(0))?;
    let end = listener.accept(non_blocking)?;
    // The new socket can be sent nowhere until it is in the slot and has
    // the end, which can go back until then.
    let file = match file(user, Socket::new(Phase::Accepting, listener.name, None), flags) {
        Ok(file) => file,
        Err(err) => {
            listener.give_back(end);
            return Err(err);
        }
    };
    if let Err(err) = install(user, to, file.clone(), flags) {
        listener.give_back(end);
        return Err(err);
    }
    let accepted = file.socket().ok_or(Kernel::NotSocket.status(0))?;
    // It is new and accepting, so no other call set its end.
    let _ = accepted.end.set(end);
    accepted.state.lock().phase = Phase::Connected;
    Ok(())
}

/// Sends `data`, with `files`, as one datagram from `socket` for `sender`
/// to the datagram socket named by the node `find` finds
/// (radix-directory(2)'s `send_datagram`), waiting for room unless
/// `non_blocking`; returns how many bytes went, all of them. A stream
/// socket fails with connected if it is connected, else with
/// not_supported, before the name is looked up, as on Linux; so does a
/// datagram that cannot go from `socket` at all (`Datagram::may_send`).
pub fn send_datagram(
    socket: &Socket,
    data: &dyn In,
    files: Option<Files>,
    sender: Peer,
    non_blocking: bool,
    find: &mut dyn FnMut() -> Result<NodeRef, Status>,
) -> Result<usize, Status> {
    let Some(datagram) = &socket.datagram else {
        let connected = socket.end.get().is_some();
        return Err(if connected { Kernel::Connected } else { Kernel::NotSupported }.status(0));
    };
    datagram.may_send(data.len())?;
    let from = socket.state.lock().name;
    let mut target = || inbox_named(&find()?).map(|(inbox, _)| inbox);
    datagram.send_to(&mut target, data, files, from, sender, non_blocking)
}

/// The socket `node` is, if it is one.
pub fn socket_of(node: &NodeRef) -> Option<&Socket> {
    radix_vfs::downcast::<Socket>(&**node)
}
