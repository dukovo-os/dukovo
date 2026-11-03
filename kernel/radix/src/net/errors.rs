// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The ICMP and ICMPv6 errors interfaces hear about what datagram and
//! ping sockets sent, given to the sockets as Linux gives them (ADR
//! 0345): a router's "time exceeded", which traceroute(8) and mtr(8) find
//! the way by, a machine's "port unreachable", which makes a connected
//! socket's next call fail with ECONNREFUSED, and the rest.
//!
//! A socket that may hear one is watched: one that asked for errors
//! (IP_RECVERR, its receive_errors option), or is connected, once it has
//! a port or echo identifier. Each interface's stack hands [`hear`] the
//! errors that came to its addresses ([`radix_net::reports`]), which goes
//! to the watched socket the error's packet came from, as Linux's lookup
//! finds it: of its protocol and port, of the error's version of IP,
//! bound to the packet's source address or to none, to its interface or
//! to none, and connected to where the packet went or to nowhere, the
//! closest match winning. One that asked for errors keeps it in its
//! [`Sink`] for radix-file(2)'s `receive_error`, [`QUEUE`] at most; its
//! next send or receive fails with it, once. One that did not hears only
//! a hard error, and only while connected.
//!
//! Lock order: a socket's state, then a stack's, then the watches, then
//! a sink's queue.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_net::{About, IpAddress, Reason, Report};
use radix_sync::Mutex;

use super::ports::{Protocol, Versions};
use crate::user::Fifo;

/// The most errors that wait for one socket; those after are lost, as
/// Linux's are once they fill the socket's receive buffer.
pub const QUEUE: usize = 32;

/// An error that waits for a socket's `receive_error`.
pub struct Heard {
    /// The error its next send or receive fails with for it.
    pub error: Status,
    pub report: Report,
    /// The interface it came by, by its number (`ifindex`), for the scope
    /// of a link-local address.
    pub interface: u32,
}

/// What a watched socket heard.
pub struct Sink {
    /// The error its next send or receive fails with, as a status word:
    /// 0 for none.
    error: AtomicU64,
    /// How many errors wait in `queue`, to be looked at without its lock.
    waiting: AtomicUsize,
    queue: Mutex<Fifo<Heard>>,
}

impl Sink {
    /// A sink with nothing in it, on the heap.
    pub fn new() -> Result<Arc<Sink, Kmalloc>, Status> {
        let sink = Sink {
            error: AtomicU64::new(0),
            waiting: AtomicUsize::new(0),
            queue: Mutex::new(Fifo::new()),
        };
        Arc::try_new_in(sink, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| Kernel::NoMemory.status(0))
    }

    /// Takes the error the socket's next call fails with, if one: looked
    /// at first, so that a call with none, as nearly every one is, takes
    /// no locked instruction.
    #[inline]
    pub fn take_error(&self) -> Option<Status> {
        if self.error.load(Ordering::Acquire) == 0 {
            return None;
        }
        match self.error.swap(0, Ordering::AcqRel) {
            0 => None,
            word => Some(Status(word)),
        }
    }

    /// Whether `poll` finds an error on its socket: one its next call
    /// fails with, or one waiting.
    pub fn has_error(&self) -> bool {
        self.error.load(Ordering::Acquire) != 0 || self.waiting.load(Ordering::Acquire) > 0
    }

    /// Takes the oldest error waiting, if one, and has the socket's next
    /// call fail with the next's, or with none, as Linux's
    /// `sock_dequeue_err_skb` does.
    pub fn take(&self) -> Option<Heard> {
        let mut queue = self.queue.lock();
        let heard = queue.pop_front()?;
        self.waiting.store(queue.len(), Ordering::Release);
        let next = queue.iter().next().map_or(0, |next| next.error.0);
        self.error.store(next, Ordering::Release);
        Some(heard)
    }

    /// Throws away the errors that wait.
    pub fn clear(&self) {
        let mut queue = self.queue.lock();
        while queue.pop_front().is_some() {}
        self.waiting.store(0, Ordering::Release);
    }

    /// Keeps `heard`, unless [`QUEUE`] wait already or there is no memory.
    fn keep(&self, heard: Heard) {
        let mut queue = self.queue.lock();
        if queue.len() < QUEUE && queue.push_back(heard).is_ok() {
            self.waiting.store(queue.len(), Ordering::Release);
        }
    }
}

/// A watched socket: which errors are its, and where they go.
pub struct Watch {
    /// The socket's own number.
    pub socket: u64,
    pub protocol: Protocol,
    /// Its port, or its echo identifier.
    pub port: u16,
    pub versions: Versions,
    /// The address it is bound to: `None` for any.
    pub local: Option<IpAddress>,
    /// The interface it is bound to, by its number: 0 for none.
    pub device: u32,
    /// Where it is connected: a ping socket's port is 0.
    pub peer: Option<(IpAddress, u16)>,
    /// Whether it asked for errors.
    pub queue: bool,
    pub sink: Arc<Sink, Kmalloc>,
}

impl Watch {
    /// How closely `report`, which came by the interface numbered
    /// `index`, matches it: `None` for not at all.
    fn matches(&self, report: &Report, index: u32) -> Option<u8> {
        let protocol = match report.about {
            About::Udp => Protocol::Udp,
            About::Ping => Protocol::Ping,
        };
        let versions = matches!(
            (report.is_v6(), self.versions),
            (false, Versions::Ipv4 | Versions::Both) | (true, Versions::Ipv6 | Versions::Both)
        );
        if protocol != self.protocol || report.local_port != self.port || !versions {
            return None;
        }
        let mut score = 0;
        if let Some(local) = self.local {
            if local != report.local {
                return None;
            }
            score += 1;
        }
        if self.device != 0 {
            if self.device != index {
                return None;
            }
            score += 1;
        }
        // A ping socket's errors are its own wherever it is connected, as
        // on Linux.
        if let Some((ip, port)) = self.peer
            && self.protocol == Protocol::Udp
        {
            if ip != report.remote || port != report.remote_port {
                return None;
            }
            score += 2;
        }
        Some(score)
    }
}

/// The watched sockets.
static WATCHES: Mutex<Option<Vec<Watch, Kmalloc>>> = Mutex::new(None);

/// Watches the socket `watch` says, in place of what was watched for it.
///
/// # Errors
///
/// no_memory, and the socket is not watched.
pub fn watch(watch: Watch) -> Result<(), Status> {
    let mut watches = WATCHES.lock();
    let watches = watches.get_or_insert_with(|| Vec::new_in(HEAP.with(Flags::MAY_SLEEP)));
    if let Some(kept) = watches.iter_mut().find(|kept| kept.socket == watch.socket) {
        *kept = watch;
        return Ok(());
    }
    watches.try_reserve(1).map_err(|_| Kernel::NoMemory.status(0))?;
    watches.push_within_capacity(watch).map_err(|_| Kernel::NoMemory.status(0))?;
    Ok(())
}

/// Watches the socket numbered `socket` no more.
pub fn unwatch(socket: u64) {
    if let Some(watches) = WATCHES.lock().as_mut()
        && let Some(at) = watches.iter().position(|kept| kept.socket == socket)
    {
        watches.swap_remove(at);
    }
}

/// The status word of `reason`.
fn status(reason: Reason) -> Status {
    let kind = match reason {
        Reason::NetworkUnreachable => Kernel::Unreachable,
        Reason::HostUnreachable => Kernel::HostUnreachable,
        Reason::ProtocolUnreachable => Kernel::ProtocolUnreachable,
        Reason::Refused => Kernel::Refused,
        Reason::MessageTooLong => Kernel::MessageTooLong,
        Reason::NotSupported => Kernel::NotSupported,
        Reason::HostDown => Kernel::HostDown,
        Reason::NoNetwork => Kernel::NoNetwork,
        Reason::Prohibited => Kernel::Prohibited,
        Reason::ProtocolError => Kernel::ProtocolError,
    };
    kind.status(0)
}

/// Hears `report`, which the interface numbered `number` received: the
/// stacks' reporter, called with the stack's lock held.
pub fn hear(number: usize, report: &Report) {
    // At most MAX_INTERFACES.
    let index = number as u32 + 1;
    let (reason, hard) = report.reason();
    let error = status(reason);
    let watches = WATCHES.lock();
    let Some(watches) = watches.as_ref() else { return };
    let best = watches
        .iter()
        .filter_map(|watch| watch.matches(report, index).map(|score| (score, watch)))
        .max_by_key(|&(score, _)| score);
    let Some((_, watch)) = best else { return };
    if watch.queue {
        watch.sink.keep(Heard { error, report: *report, interface: index });
    } else if !hard || watch.peer.is_none() {
        return;
    }
    watch.sink.error.store(error.0, Ordering::Release);
}
