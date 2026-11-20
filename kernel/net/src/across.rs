// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! TCP connections whose two ends are sockets of one stack, on the
//! loopback link, pass their bytes straight across (ADR 0186): from the
//! sender's system call into the other end's receive buffer, with no
//! packet between them, as Solaris's TCP fusion does. A byte is copied
//! twice, into the kernel and out of it, as on Linux, where it was copied
//! four times in packets: into the sender's buffer, into a packet, out of
//! it into the receiver's buffer, and out of that.
//!
//! The kernel's patch to smoltcp counts what goes across as a segment
//! sent, received and acknowledged at once, at both ends, so that their
//! sequence numbers, windows and timers are as they would have been, and
//! packets can go between the two again at any time: bytes go across
//! only while none wait, at either end or in the link's ring, to go the
//! other way. A connection that cannot pass its bytes across, or whose
//! other end is somewhere else, sends them in packets, as before.
//!
//! A UDP datagram to another socket of the loopback interface goes
//! straight into that socket's receive buffer the same way (ADR 0553):
//! the socket a packet would reach, found as smoltcp finds it, takes it
//! with the addresses and ports the packet would carry, and the
//! interface counts it as a packet sent and received. A datagram that
//! would reach no socket goes as a packet, which brings back the ICMP
//! error that tells its sender.

use smoltcp::iface::SocketHandle;
use smoltcp::socket::{Socket, tcp, udp};
use smoltcp::wire::{IpAddress, IpEndpoint};

use crate::{Link, Stack, instant};

/// Where a TCP connection's other end was found among its stack's
/// sockets, kept with the connection so that it is looked at first the
/// next time.
#[derive(Clone, Copy, Debug, Default)]
pub struct OtherEnd(Option<SocketHandle>);

/// What [`Stack::send_across`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Across<R> {
    /// Nothing: the bytes go in packets, through the stack.
    Packets,
    /// Nothing: the other end has no room for them, until it reads.
    Full,
    /// Passed the bytes the second says, and the writer returned the
    /// first.
    Passed(R, usize),
}

/// Whether `socket` is the other end of the connection from `local` to
/// `remote`, and is still connected.
fn ends(socket: &tcp::Socket<'_>, local: IpEndpoint, remote: IpEndpoint) -> bool {
    socket.local_endpoint() == Some(remote)
        && socket.remote_endpoint() == Some(local)
        && !matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait)
}

impl<L: Link> Stack<L> {
    /// The other end of TCP connection `handle`, if it is another of this
    /// stack's sockets and bytes can go across to it now: the stack is on
    /// the loopback link, and no packet waits in its ring. `other` is where
    /// it was last found.
    fn other_end(&mut self, handle: SocketHandle, other: &mut OtherEnd) -> Option<SocketHandle> {
        // A capture of the loopback interface sees its packets (ADR 0346).
        if self.link.pending() || self.link.loopback().is_none() || self.watched() {
            return None;
        }
        let socket = self.sockets.try_get::<tcp::Socket<'static>>(handle)?;
        let (local, remote) = (socket.local_endpoint()?, socket.remote_endpoint()?);
        if let Some(last) = other.0
            && last != handle
            && self
                .sockets
                .try_get::<tcp::Socket<'static>>(last)
                .is_some_and(|socket| ends(socket, local, remote))
        {
            return Some(last);
        }
        // A connection to itself, from a port to the same port, is not
        // its own other end.
        let found = self.sockets.iter().find_map(|(found, socket)| match socket {
            Socket::Tcp(socket) if found != handle && ends(socket, local, remote) => Some(found),
            _ => None,
        });
        other.0 = found;
        found
    }

    /// How many bytes TCP connection `handle` can pass straight across to
    /// its other end now, 0 when the other end must read first; `None`
    /// when they go in packets.
    pub fn room_across(&mut self, handle: SocketHandle, other: &mut OtherEnd) -> Option<usize> {
        let found = self.other_end(handle, other)?;
        let next = self.sockets.try_get::<tcp::Socket<'static>>(handle)?.next_across()?;
        self.sockets.try_get::<tcp::Socket<'static>>(found)?.room_across(next)
    }

    /// Passes bytes from TCP connection `handle` straight across into its
    /// other end's receive buffer, at `now_ns`, if it can: `write` writes
    /// them into the room there and returns how many it wrote, and what to
    /// return. `other` is where the other end was last found.
    pub fn send_across<R>(
        &mut self,
        handle: SocketHandle,
        other: &mut OtherEnd,
        now_ns: u64,
        write: impl FnOnce(&mut [u8]) -> (usize, R),
    ) -> Across<R> {
        match self.room_across(handle, other) {
            None => return Across::Packets,
            Some(0) => return Across::Full,
            Some(_) => {}
        }
        // `room_across` found both, just now.
        let (Some(found), now) = (other.0, instant(now_ns)) else { return Across::Packets };
        let Some(end) = self.sockets.try_get_mut::<tcp::Socket<'static>>(found) else {
            return Across::Packets;
        };
        let (result, size, window) = end.receive_across(now, write);
        if size > 0
            && let Some(socket) = self.sockets.try_get_mut::<tcp::Socket<'static>>(handle)
        {
            socket.sent_across(size, window, now);
        }
        Across::Passed(result, size)
    }

    /// Passes `payload`, a datagram that UDP socket `handle` sends with
    /// `meta`, straight into the receive buffer of the socket of this
    /// stack that its packet would reach, if one would and nothing it could
    /// overtake waits: the stack is on the loopback link, with no packet in
    /// its ring and no capture watching it, and `handle` has no datagram
    /// queued. Returns whether it did; if not, the datagram goes in a
    /// packet.
    pub fn send_datagram_across(
        &mut self,
        handle: SocketHandle,
        meta: &udp::UdpMetadata,
        payload: &[u8],
    ) -> bool {
        let to = meta.endpoint;
        // Only one of the interface's own addresses: never a group's, a
        // broadcast address or one the interface does not have.
        if self.link.pending()
            || self.link.loopback().is_none()
            || self.watched()
            || !self.has_address(to.addr)
        {
            return false;
        }
        let Some(socket) = self.sockets.try_get::<udp::Socket<'static>>(handle) else {
            return false;
        };
        if socket.send_queue() > 0 {
            return false;
        }
        // From the address smoltcp would send it from.
        let bound = socket.endpoint();
        let from = match meta.local_address.or(bound.addr) {
            Some(address) => address,
            None => match self.iface.get_source_address(&to.addr) {
                Some(address) => address,
                None => return false,
            },
        };
        if from.is_unspecified() || bound.port == 0 {
            return false;
        }
        // The first socket that takes it, as `process_udp` looks: the
        // sender itself, when it sends to its own port.
        let Some(end) = self.sockets.iter_mut().find_map(|(_, socket)| match socket {
            Socket::Udp(socket) if socket.takes_across(to) => Some(socket),
            _ => None,
        }) else {
            return false;
        };
        end.receive_across(IpEndpoint::new(from, bound.port), to.addr, payload);
        let len = datagram_len(to.addr, payload.len());
        self.counted.sent(len);
        self.counted.received(len);
        true
    }

    /// Tells TCP connection `handle`'s other end straight of the window it
    /// offers, at `now_ns`, after a read opened it, so that no packet need
    /// carry it. Returns whether the other end has bytes waiting to send,
    /// which a poll sends, or `None` if it could not tell it: a packet
    /// will.
    pub fn window_across(
        &mut self,
        handle: SocketHandle,
        other: &mut OtherEnd,
        now_ns: u64,
    ) -> Option<bool> {
        let found = self.other_end(handle, other)?;
        let (ack, window) =
            self.sockets.try_get::<tcp::Socket<'static>>(handle)?.window_across()?;
        let end = self.sockets.try_get_mut::<tcp::Socket<'static>>(found)?;
        if !end.heard_window_across(ack, window, instant(now_ns)) {
            return None;
        }
        let waiting = end.send_queue() > 0;
        self.sockets.try_get_mut::<tcp::Socket<'static>>(handle)?.told_window_across(window);
        Some(waiting)
    }
}

/// The length of the packet that would carry a UDP datagram of `payload`
/// bytes to `to`: its IP and UDP headers too, as the interface counts it.
fn datagram_len(to: IpAddress, payload: usize) -> usize {
    let ip = match to {
        IpAddress::Ipv4(_) => 20,
        IpAddress::Ipv6(_) => 40,
    };
    ip + 8 + payload
}
