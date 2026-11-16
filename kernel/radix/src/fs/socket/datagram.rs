// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Unix datagram sockets (ADR 0525): POSIX's AF_UNIX sockets of type
//! SOCK_DGRAM, as Linux has them. Each send is one datagram, which a
//! receive takes whole.
//!
//! A datagram socket's [`Datagram`] owns its [`Inbox`], where the
//! datagrams sent to it wait to be received. A sender finds an inbox
//! through the socket's name, whose `Binding` keeps a weak reference to
//! it, or through the weak reference a connected socket keeps to its
//! peer's, so no socket keeps another alive. When a socket goes, its inbox
//! closes and what waits in it goes too; a send to it then fails with
//! refused, and a socket connected to it forgets it at its next send.
//!
//! An inbox holds at most DATAGRAM_QUEUE_MAX datagrams and
//! DATAGRAM_BYTES_MAX bytes of them, and a datagram always goes into an
//! empty one. Linux bounds what a sender has on its way instead; bounding
//! the receiver keeps each socket's memory fixed where it lies. A send to
//! a full inbox waits for room, or fails with would_block.
//!
//! A socket connected to another takes datagrams from that one alone, as
//! Linux's `unix_may_send` has it: a send from any other fails with
//! not_permitted, even once the peer has gone.

use alloc::boxed::Box;
use alloc::sync::Weak;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::file::{DATAGRAM_BYTES_MAX, DATAGRAM_MAX, DATAGRAM_QUEUE_MAX, FILES_IN_FLIGHT_MAX};
use radix_mm::Kmalloc;
use radix_sync::{Mutex, MutexGuard, SpinLock};
use radix_vfs::{In, KArc, Out, mem};

use super::flight::{self, Queue};
use super::{Name, Peer, Received};
use crate::fs::pipe::{self, Files};
use crate::sched::{self, wait};
use crate::user::Fifo;
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// What the pollers of every datagram socket count themselves in for
/// room in its peer's inbox: the peer can change while a poll lasts,
/// and a poll must count itself out of what it counted itself in.
static ROOM: Polled = Polled::new();

/// A datagram on its way.
struct Message {
    data: Vec<u8, Kmalloc>,
    files: Option<Files>,
    /// How many of `files` are Unix sockets.
    sockets: usize,
    /// The name of the socket that sent it, and its process, as they were
    /// at the send.
    from: Name,
    sender: Peer,
}

impl Message {
    /// A copy of `data`, with `files`, made before any lock is taken: the
    /// copy may wait for a page of the program's memory, and the memory
    /// for it.
    fn new(
        data: &dyn In,
        files: Option<Files>,
        from: Name,
        sender: Peer,
    ) -> Result<Box<Message, Kmalloc>, Status> {
        let mut copied = pipe::storage(data.len())?;
        if !copied.is_empty() {
            data.get(0, &mut copied).map_err(crate::fs::status)?;
        }
        let sockets = files.as_ref().map_or(0, pipe::sockets_among);
        let message = Message { data: copied, files, sockets, from, sender };
        let heap = radix_mm::HEAP.with(radix_mm::Flags::MAY_SLEEP);
        Box::try_new_in(message, heap).map_err(|_| Kernel::NoMemory.status(0))
    }

    /// Copies it into `out`, as much as fits; the rest is lost.
    fn receive(self: Box<Message, Kmalloc>, out: &mut dyn Out) -> Result<Received, Status> {
        let count = self.data.len().min(out.len());
        if count > 0 {
            out.put(0, &self.data[..count]).map_err(crate::fs::status)?;
        }
        let Message { data, files, from, sender, .. } = *self;
        Ok(Received { count, size: data.len(), files, from, sender })
    }
}

/// The socket a socket is connected to.
struct Connected {
    inbox: Weak<Inbox, Kmalloc>,
    /// Its name, as its binding holds it, for `getpeername`.
    name: Name,
}

/// A datagram socket's queue.
pub(super) struct Inbox {
    queue: Mutex<Fifo<Box<Message, Kmalloc>>>,
    /// How many datagrams wait, how many bytes they hold, and how many
    /// bytes the next holds, as the queue says, for the waits and for
    /// what does not lock it: changed only with it locked.
    count: AtomicUsize,
    bytes: AtomicUsize,
    first: AtomicUsize,
    /// How many files ride with them, and how many of those are Unix
    /// sockets (`flight`).
    files: AtomicUsize,
    sockets: AtomicUsize,
    /// Whether its socket was shut for reading, and whether it has gone.
    shut_read: AtomicBool,
    closed: AtomicBool,
    /// What its socket is connected to.
    connected: SpinLock<Option<Connected>>,
    /// Its pollers.
    polled: Polled,
}

impl Inbox {
    fn new() -> Inbox {
        Inbox {
            queue: Mutex::new(Fifo::new()),
            count: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            first: AtomicUsize::new(0),
            files: AtomicUsize::new(0),
            sockets: AtomicUsize::new(0),
            shut_read: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            connected: SpinLock::new(None),
            polled: Polled::new(),
        }
    }

    fn key(&self) -> usize {
        core::ptr::from_ref::<Inbox>(self) as usize
    }

    /// How many Unix sockets ride in it.
    pub(super) fn sockets(&self) -> usize {
        self.sockets.load(Ordering::Acquire)
    }

    /// A datagram came: the waits look again, and so do its pollers.
    fn changed(&self) {
        wait::wake_all(self.key());
        self.polled.changed();
    }

    /// Room was made, or a send to it now fails at once: the sockets that
    /// send to it look again too.
    fn room_changed(&self) {
        self.changed();
        ROOM.changed();
    }

    /// Whether a datagram of `len` bytes waits for room.
    fn full(&self, len: usize) -> bool {
        let count = self.count.load(Ordering::Acquire);
        count >= DATAGRAM_QUEUE_MAX as usize
            || (count > 0 && self.bytes.load(Ordering::Acquire) + len > DATAGRAM_BYTES_MAX as usize)
    }

    /// Whether a poll of a socket that sends to it says that it cannot
    /// send: no room for another datagram, or more than a quarter of the
    /// bytes it holds taken, as Linux calls a socket writable while what
    /// it has sent is at most a quarter of its buffer. A send that such a
    /// poll lets through finds room for any datagram up to three quarters
    /// of the most, where room for a byte alone would leave a bigger one
    /// to fail and poll again at once.
    fn crowded(&self) -> bool {
        self.count.load(Ordering::Acquire) >= DATAGRAM_QUEUE_MAX as usize
            || self.bytes.load(Ordering::Acquire) > DATAGRAM_BYTES_MAX as usize / 4
    }

    /// Whether its socket takes datagrams from the socket whose inbox is
    /// `from`: one connected takes its peer's alone, gone or not.
    fn takes_from(&self, from: &Inbox) -> bool {
        self.connected.lock().as_ref().is_none_or(|peer| core::ptr::eq(peer.inbox.as_ptr(), from))
    }

    /// Puts `message` in, from the socket whose inbox is `from`, if it can
    /// go now. Gives it back with what it failed with otherwise.
    fn try_put(
        &self,
        message: Box<Message, Kmalloc>,
        from: &Inbox,
    ) -> Result<(), (Status, Box<Message, Kmalloc>)> {
        let mut queue = self.queue.lock();
        let sending = match self.admit(&mut queue, &message, from) {
            Ok(sending) => sending,
            Err(err) => return Err((err, message)),
        };
        let len = message.data.len();
        if self.count.fetch_add(1, Ordering::AcqRel) == 0 {
            self.first.store(len, Ordering::Release);
        }
        self.bytes.fetch_add(len, Ordering::AcqRel);
        self.files.fetch_add(message.files.as_ref().map_or(0, Vec::len), Ordering::AcqRel);
        self.sockets.fetch_add(message.sockets, Ordering::AcqRel);
        // Room was reserved.
        let _ = queue.push_back(message);
        drop(sending);
        drop(queue);
        self.changed();
        Ok(())
    }

    /// Whether `message` from the socket whose inbox is `from` may go in
    /// now, its queue locked as `queue`. Fails with refused once its socket
    /// has gone; not_permitted if that socket is connected to another;
    /// gone once it was shut for reading; would_block while it is full;
    /// too_many with FILES_IN_FLIGHT_MAX files on their way, or against
    /// the rule for Unix sockets on their way. Makes room for the message
    /// in the queue, and holds the rule's lock until it is in when Unix
    /// sockets ride with it.
    fn admit(
        &self,
        queue: &mut Fifo<Box<Message, Kmalloc>>,
        message: &Message,
        from: &Inbox,
    ) -> Result<Option<MutexGuard<'static, ()>>, Status> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Kernel::Refused.status(0));
        }
        if !self.takes_from(from) {
            return Err(Kernel::NotPermitted.status(0));
        }
        if self.shut_read.load(Ordering::Acquire) {
            return Err(Kernel::Gone.status(0));
        }
        if self.full(message.data.len()) {
            return Err(Kernel::WouldBlock.status(0));
        }
        let files = message.files.as_ref().map_or(0, Vec::len);
        if self.files.load(Ordering::Acquire) + files > FILES_IN_FLIGHT_MAX as usize {
            return Err(Kernel::TooMany.status(0));
        }
        if !queue.reserve_one() {
            return Err(Kernel::NoMemory.status(0));
        }
        let Some(files) = message.files.as_ref().filter(|_| message.sockets > 0) else {
            return Ok(None);
        };
        let sending = flight::hold();
        flight::check(files, Queue::inbox(self))?;
        Ok(Some(sending))
    }

    /// Waits until a datagram of `len` bytes would go in, or would fail at
    /// once.
    fn wait_room(&self, len: usize) -> Result<(), Status> {
        wait::wait_until(self.key(), "datagram send", || {
            !self.full(len)
                || self.closed.load(Ordering::Acquire)
                || self.shut_read.load(Ordering::Acquire)
                || sched::interrupted()
        });
        if sched::interrupted() {
            return Err(interrupted(Restart::Always));
        }
        Ok(())
    }

    /// Takes the next datagram into `out`, waiting for one unless
    /// `non_blocking`, with the files that came with it: what `out` has no
    /// room for is lost. With `peek` it stays, and copies of its files
    /// come. Once its socket was shut for reading, a receive that would
    /// wait returns nothing at once, from no one, and one that may not
    /// wait fails with would_block, as on Linux.
    fn take(&self, out: &mut dyn Out, non_blocking: bool, peek: bool) -> Result<Received, Status> {
        loop {
            {
                let mut queue = self.queue.lock();
                if peek {
                    if let Some(message) = queue.iter().next() {
                        // Copied with the queue locked, so that it stays.
                        let count = message.data.len().min(out.len());
                        if count > 0 {
                            out.put(0, &message.data[..count]).map_err(crate::fs::status)?;
                        }
                        let files = message.files.as_ref().map(pipe::copies).transpose()?;
                        let (from, sender) = (message.from, message.sender);
                        return Ok(Received {
                            count,
                            size: message.data.len(),
                            files,
                            from,
                            sender,
                        });
                    }
                } else if let Some(message) = queue.pop_front() {
                    let next = queue.iter().next().map_or(0, |next| next.data.len());
                    self.first.store(next, Ordering::Release);
                    self.count.fetch_sub(1, Ordering::AcqRel);
                    self.bytes.fetch_sub(message.data.len(), Ordering::AcqRel);
                    let files = message.files.as_ref().map_or(0, Vec::len);
                    self.files.fetch_sub(files, Ordering::AcqRel);
                    self.sockets.fetch_sub(message.sockets, Ordering::AcqRel);
                    drop(queue);
                    self.room_changed();
                    // Copied once it is out, as Linux does: if the copy
                    // fails, it is lost, and its files are closed.
                    let received = message.receive(out);
                    if received.is_err() {
                        crate::fs::drop_deferred();
                    }
                    return received;
                }
                if self.shut_read.load(Ordering::Acquire) && !non_blocking {
                    return Ok(Received::NOTHING);
                }
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            wait::wait_until(self.key(), "datagram receive", || {
                self.count.load(Ordering::Acquire) > 0
                    || self.shut_read.load(Ordering::Acquire)
                    || sched::interrupted()
            });
            if sched::interrupted() && self.count.load(Ordering::Acquire) == 0 {
                return Err(interrupted(Restart::Always));
            }
        }
    }

    /// Its socket is shut for reading: what waits may still be received,
    /// and what is sent to it fails with gone.
    fn shut_read(&self) {
        if !self.shut_read.swap(true, Ordering::AcqRel) {
            self.room_changed();
        }
    }

    /// Takes every datagram that waits, with the counts, its queue locked
    /// as `queue`, for the caller to drop once it is unlocked.
    fn take_all(&self, queue: &mut Fifo<Box<Message, Kmalloc>>) -> Fifo<Box<Message, Kmalloc>> {
        for count in [&self.count, &self.bytes, &self.first, &self.files, &self.sockets] {
            count.store(0, Ordering::Release);
        }
        queue.take()
    }

    /// Throws away the datagrams that wait, as Linux does when a socket
    /// connects to another or disconnects.
    fn purge(&self) {
        let gone = self.take_all(&mut self.queue.lock());
        drop(gone);
        // A Unix socket among their files goes now, not at the store's
        // next change (`pipe::Passed`).
        crate::fs::drop_deferred();
        self.room_changed();
    }

    /// Its socket has gone: what waits goes, and so does what it was
    /// connected to; sends to it fail with refused from here.
    fn close(&self) {
        let (gone, peer) = {
            let mut queue = self.queue.lock();
            self.closed.store(true, Ordering::Release);
            (self.take_all(&mut queue), self.connected.lock().take())
        };
        drop(gone);
        drop(peer);
        self.room_changed();
    }
}

/// A datagram socket's part of its socket.
pub(super) struct Datagram {
    inbox: KArc<Inbox>,
    /// Whether it was shut for writing.
    shut_write: AtomicBool,
    /// For one of a pair, the process that made the pair, which
    /// `socket_state` gives (SO_PEERCRED); otherwise none.
    pair_maker: Peer,
}

impl Datagram {
    /// A datagram socket's part, neither named nor connected.
    pub(super) fn new() -> Result<Datagram, Status> {
        Datagram::with_maker(Peer::NONE)
    }

    fn with_maker(pair_maker: Peer) -> Result<Datagram, Status> {
        let inbox = mem::arc(Inbox::new()).map_err(|_| Kernel::NoMemory.status(0))?;
        Ok(Datagram { inbox, shut_write: AtomicBool::new(false), pair_maker })
    }

    /// The parts of a pair of datagram sockets that `maker` makes, each
    /// connected to the other.
    pub(super) fn pair(maker: Peer) -> Result<(Datagram, Datagram), Status> {
        let (one, other) = (Datagram::with_maker(maker)?, Datagram::with_maker(maker)?);
        for (from, to) in [(&one, &other), (&other, &one)] {
            let inbox = KArc::downgrade(&to.inbox);
            *from.inbox.connected.lock() = Some(Connected { inbox, name: Name::NONE });
        }
        Ok((one, other))
    }

    /// Its inbox.
    pub(super) fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    /// A weak reference to its inbox, which its name keeps.
    pub(super) fn downgrade(&self) -> Weak<Inbox, Kmalloc> {
        KArc::downgrade(&self.inbox)
    }

    /// The process its pair's maker was, or none.
    pub(super) fn pair_maker(&self) -> Peer {
        self.pair_maker
    }

    /// The inbox of the socket it is connected to, if it is.
    fn peer(&self) -> Option<Weak<Inbox, Kmalloc>> {
        self.inbox.connected.lock().as_ref().map(|peer| peer.inbox.clone())
    }

    /// Whether it is connected.
    pub(super) fn is_connected(&self) -> bool {
        self.inbox.connected.lock().is_some()
    }

    /// The name of the socket it is connected to; not_connected if it is
    /// not.
    pub(super) fn peer_name(&self) -> Result<Name, Status> {
        let connected = self.inbox.connected.lock();
        connected.as_ref().map(|peer| peer.name).ok_or(Kernel::NotConnected.status(0))
    }

    /// Connects it to `target`, the inbox of the socket named `name`: its
    /// sends go there and it takes datagrams from there alone. Fails with
    /// refused if that socket has gone, and with not_permitted if it is
    /// connected to another. Connected to another before, it throws away
    /// what waits, as Linux does.
    pub(super) fn connect(&self, target: &KArc<Inbox>, name: Name) -> Result<(), Status> {
        if target.closed.load(Ordering::Acquire) {
            return Err(Kernel::Refused.status(0));
        }
        if !target.takes_from(&self.inbox) {
            return Err(Kernel::NotPermitted.status(0));
        }
        let peer = Connected { inbox: KArc::downgrade(target), name };
        let old = self.inbox.connected.lock().replace(peer);
        let other = old
            .as_ref()
            .is_some_and(|old| !core::ptr::eq(old.inbox.as_ptr(), KArc::as_ptr(target)));
        // Outside the lock: it may be the last reference to the old one.
        drop(old);
        if other {
            self.inbox.purge();
        } else {
            self.inbox.polled.changed();
        }
        Ok(())
    }

    /// Has it forget the socket it is connected to, if it is, throwing
    /// away what waits, as Linux does.
    pub(super) fn disconnect(&self) {
        let old = self.inbox.connected.lock().take();
        if old.is_some() {
            drop(old);
            self.inbox.purge();
        }
    }

    /// Forgets `peer` if it is still what it is connected to: the socket
    /// has gone, which a send found.
    fn forget(&self, peer: &Weak<Inbox, Kmalloc>) {
        let old = {
            let mut connected = self.inbox.connected.lock();
            let same = connected.as_ref().is_some_and(|now| Weak::ptr_eq(&now.inbox, peer));
            if same { connected.take() } else { None }
        };
        if old.is_some() {
            drop(old);
            self.inbox.purge();
        }
    }

    /// Whether a datagram of `len` bytes may go from it at all, before
    /// anything is copied or looked up: message_too_long past
    /// DATAGRAM_MAX, gone once it was shut for writing, without SIGPIPE,
    /// as Linux has it for datagrams.
    pub(super) fn may_send(&self, len: usize) -> Result<(), Status> {
        if len > DATAGRAM_MAX as usize {
            return Err(Kernel::MessageTooLong.status(0));
        }
        if self.shut_write.load(Ordering::Acquire) {
            return Err(Kernel::Gone.status(0));
        }
        Ok(())
    }

    /// Sends `data`, with `files`, as one datagram to the socket it is
    /// connected to, from it as `from` for `sender`, waiting for room
    /// unless `non_blocking`; returns how many bytes went, all of them.
    /// Fails with not_connected if it is not, and with refused once that
    /// socket has gone, when it forgets it and throws away what waits, as
    /// Linux does.
    pub(super) fn send_to_peer(
        &self,
        data: &dyn In,
        files: Option<Files>,
        from: Name,
        sender: Peer,
        non_blocking: bool,
    ) -> Result<usize, Status> {
        let peer = self.peer().ok_or(Kernel::NotConnected.status(0))?;
        self.may_send(data.len())?;
        let message = Message::new(data, files, from, sender)?;
        let refused = Kernel::Refused.status(0);
        let sent = match peer.upgrade() {
            Some(target) => self.deliver(&target, message, non_blocking),
            None => Err(refused),
        };
        if sent == Err(refused) {
            self.forget(&peer);
        }
        sent
    }

    /// Sends `data`, with `files`, as one datagram from it as `from` for
    /// `sender` to the inbox `target` finds, of a socket named, as
    /// [`Datagram::send_to_peer`] does; `data` is copied first, as Linux
    /// does. The caller has asked [`Datagram::may_send`].
    pub(super) fn send_to(
        &self,
        target: &mut dyn FnMut() -> Result<KArc<Inbox>, Status>,
        data: &dyn In,
        files: Option<Files>,
        from: Name,
        sender: Peer,
        non_blocking: bool,
    ) -> Result<usize, Status> {
        let message = Message::new(data, files, from, sender)?;
        let target = target()?;
        self.deliver(&target, message, non_blocking)
    }

    /// Puts `message` in `target`, waiting for room unless
    /// `non_blocking`; returns how many bytes it holds.
    fn deliver(
        &self,
        target: &Inbox,
        message: Box<Message, Kmalloc>,
        non_blocking: bool,
    ) -> Result<usize, Status> {
        let len = message.data.len();
        let mut message = message;
        loop {
            match target.try_put(message, &self.inbox) {
                Ok(()) => return Ok(len),
                Err((err, back)) if err == Kernel::WouldBlock.status(0) && !non_blocking => {
                    message = back;
                    target.wait_room(len)?;
                }
                Err((err, _)) => return Err(err),
            }
        }
    }

    /// Receives as [`Inbox::take`] does.
    pub(super) fn receive(
        &self,
        out: &mut dyn Out,
        non_blocking: bool,
        peek: bool,
    ) -> Result<Received, Status> {
        self.inbox.take(out, non_blocking, peek)
    }

    /// The size of the next datagram, 0 if none waits.
    pub(super) fn pending(&self) -> usize {
        self.inbox.first.load(Ordering::Acquire)
    }

    /// Shuts it for reading, for writing, or both: its own sends fail
    /// with gone once it is shut for writing, and those to it once it is
    /// shut for reading. Its peer is not told, as on Linux.
    pub(super) fn shutdown(&self, read: bool, write: bool) {
        if read {
            self.inbox.shut_read();
        }
        if write && !self.shut_write.swap(true, Ordering::AcqRel) {
            self.inbox.changed();
        }
    }

    /// The events `poll` finds on it, as Linux's: readable while a
    /// datagram waits or once it is shut for reading; writable unless its
    /// peer's inbox is crowded ([`Inbox::crowded`]); hung up once shut
    /// both ways.
    pub(super) fn events(&self) -> u16 {
        let read_done = self.inbox.shut_read.load(Ordering::Acquire);
        let write_done = self.shut_write.load(Ordering::Acquire);
        let mut events = 0;
        if self.inbox.count.load(Ordering::Acquire) > 0 || read_done {
            events |= poll::READABLE;
        }
        if write_done || !self.peer_crowded() {
            events |= poll::WRITABLE;
        }
        if read_done && write_done {
            events |= poll::HANG_UP;
        }
        events
    }

    /// Whether its peer's inbox is crowded: never when it is not
    /// connected, as a send then needs a name, and never when a send would
    /// fail at once.
    fn peer_crowded(&self) -> bool {
        let Some(peer) = self.peer().and_then(|peer| peer.upgrade()) else { return false };
        !peer.closed.load(Ordering::Acquire)
            && !peer.shut_read.load(Ordering::Acquire)
            && peer.takes_from(&self.inbox)
            && peer.crowded()
    }

    /// Calls `each` with what its pollers count themselves in: its inbox,
    /// and for room in its peer's, [`ROOM`].
    pub(super) fn polled(&self, each: &mut dyn FnMut(&Polled)) {
        each(&self.inbox.polled);
        each(&ROOM);
    }
}

impl Drop for Datagram {
    fn drop(&mut self) {
        self.inbox.close();
    }
}
