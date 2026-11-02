// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tunnels (ADR 0339): interfaces whose packets a program carries, as
//! Linux's tun devices' are, WireGuard's among them, which wgd(8) seals
//! and sends to its peers over UDP.
//!
//! A tunnel, `wg0`, is an IP link: its stack sends and receives IP
//! packets alone, with no Ethernet header, MAC address or neighbours, at
//! most [`MTU`] bytes each. It is made as a bridge is, through the
//! network capability's control right (radix-network(2)'s `add_link`,
//! [`super::links`]), and a program with that right opens it
//! (`open_tunnel`, `crate::fs::tunnel`): what its stack sends goes into
//! a queue the program reads, a packet each, and what the program writes
//! into a queue its stack takes. One program at a time has it. While
//! none does, its link is down and what its stack sends is lost, as on
//! a wire with nothing at its other end; it stays, with its addresses,
//! so that its program can start again on it, until `remove_link` takes
//! it away.
//!
//! Each tunnel's queues are under a lock of their own, which a stack's
//! lock may be held under but which never holds one; the program's
//! thread is woken once the poll that gave it a packet has let go of its
//! stack ([`wake_given`]).

use core::sync::atomic::{AtomicU32, Ordering};

use radix_sync::Mutex;

use super::links::Queue;
use super::{MAX_INTERFACES, numbered};
use crate::sched::wait;

/// The longest packet a tunnel sends: WireGuard's 1420, which leaves room
/// in a 1500-byte Ethernet frame for its own header, a UDP header and an
/// IPv6 header, as Linux's and wg-quick's default does.
pub const MTU: usize = 1420;

/// A tunnel's two queues, and whether a program has it.
pub struct Pipe {
    /// The packets its stack sent, for its program.
    out: Queue,
    /// The packets its program wrote, for its stack.
    inbox: Queue,
    /// Whether a program has it open.
    open: bool,
    /// Packets lost to a full queue, either way.
    lost: u64,
}

impl Pipe {
    /// A tunnel's pipe, with nothing in it: `None` without the memory.
    pub fn new() -> Option<Pipe> {
        Some(Pipe { out: Queue::new()?, inbox: Queue::new()?, open: false, lost: 0 })
    }
}

/// Each interface's pipe, if it is a tunnel.
static PIPES: [Mutex<Option<Pipe>>; MAX_INTERFACES] = [const { Mutex::new(None) }; MAX_INTERFACES];

/// The tunnels whose stacks sent their programs packets since they were
/// last woken, a bit each.
static GIVEN: AtomicU32 = AtomicU32::new(0);

/// The link of a tunnel interface: its stack's ends of its queues.
pub struct Tunnel {
    /// Its interface's number.
    pub number: usize,
}

impl Tunnel {
    /// The next packet its program wrote, copied into `packet`: its
    /// length.
    pub fn receive(&self, packet: &mut [u8]) -> Option<usize> {
        PIPES.get(self.number)?.lock().as_mut()?.inbox.pop(packet)
    }

    /// Gives its program `packet`, its stack's: false if no program has
    /// it, or its queue is full, and the packet is lost.
    pub fn send(&self, packet: &[u8]) -> bool {
        let Some(pipe) = PIPES.get(self.number) else { return false };
        let mut pipe = pipe.lock();
        let Some(pipe) = pipe.as_mut().filter(|pipe| pipe.open) else { return false };
        let given = pipe.out.push(|room| {
            room.get_mut(..packet.len())?.copy_from_slice(packet);
            Some(packet.len())
        });
        if given {
            GIVEN.fetch_or(1 << self.number, Ordering::Relaxed);
        } else {
            pipe.lost += 1;
        }
        given
    }

    /// Whether a program has it open: its link is up.
    pub fn open(&self) -> bool {
        PIPES.get(self.number).is_some_and(|pipe| pipe.lock().as_ref().is_some_and(|p| p.open))
    }
}

/// Makes interface `number` a tunnel, with `pipe`.
pub fn install(number: usize, pipe: Pipe) {
    if let Some(slot) = PIPES.get(number) {
        *slot.lock() = Some(pipe);
    }
}

/// Takes tunnel `number` away: its program reads and writes no more,
/// and is woken to find so.
pub fn remove(number: usize) {
    if let Some(slot) = PIPES.get(number) {
        slot.lock().take();
    }
    wait::wake_all(key(number));
    crate::fs::tunnel::POLLED.changed();
}

/// Why a tunnel's program could not open it, or read or write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeError {
    /// It is not a tunnel, or it was taken away.
    Gone,
    /// Another program has it open.
    Taken,
}

/// Has tunnel `number` open, for a program: its link comes up.
///
/// # Errors
///
/// [`PipeError::Gone`] if it is not a tunnel, [`PipeError::Taken`] if a
/// program has it already.
pub fn open(number: usize) -> Result<(), PipeError> {
    {
        let slot = PIPES.get(number).ok_or(PipeError::Gone)?;
        let mut pipe = slot.lock();
        let pipe = pipe.as_mut().ok_or(PipeError::Gone)?;
        if pipe.open {
            return Err(PipeError::Taken);
        }
        pipe.open = true;
    }
    changed(number);
    Ok(())
}

/// Lets tunnel `number` go, as its program's last description of it
/// goes: its link is down, and what waited for the program is let go.
pub fn close(number: usize) {
    if let Some(slot) = PIPES.get(number)
        && let Some(pipe) = slot.lock().as_mut()
    {
        pipe.open = false;
        pipe.out.clear();
    }
    changed(number);
}

/// The link of tunnel `number` went up or down: its stack is told, and
/// whoever lists the interfaces.
fn changed(number: usize) {
    if let Some(interface) = numbered(number) {
        interface.kick();
    }
    super::changed();
}

/// Calls `take` with the oldest packet tunnel `number`'s stack sent its
/// program, if one waits, which goes if `take` says so: what `take`
/// returned, or `None` if none waits.
///
/// # Errors
///
/// [`PipeError::Gone`] if it is not a tunnel now.
pub fn take<R>(
    number: usize,
    take: impl FnOnce(&[u8]) -> (R, bool),
) -> Result<Option<R>, PipeError> {
    let slot = PIPES.get(number).ok_or(PipeError::Gone)?;
    let mut pipe = slot.lock();
    let pipe = pipe.as_mut().ok_or(PipeError::Gone)?;
    let Some(packet) = pipe.out.oldest() else { return Ok(None) };
    let (taken, goes) = take(packet);
    if goes {
        pipe.out.drop_oldest();
    }
    Ok(Some(taken))
}

/// Gives tunnel `number`'s stack the packet `write` writes, of `len`
/// bytes, and wakes the stack: whether `write` wrote it. A packet that
/// finds the queue full is lost, as a card loses a frame when its ring
/// is full.
///
/// # Errors
///
/// [`PipeError::Gone`] if it is not a tunnel now.
pub fn give(
    number: usize,
    len: usize,
    write: impl FnOnce(&mut [u8]) -> bool,
) -> Result<bool, PipeError> {
    let written = {
        let slot = PIPES.get(number).ok_or(PipeError::Gone)?;
        let mut pipe = slot.lock();
        let pipe = pipe.as_mut().ok_or(PipeError::Gone)?;
        if pipe.inbox.is_full() {
            pipe.lost += 1;
            return Ok(true);
        }
        let mut written = false;
        pipe.inbox.push(|room| {
            let room = room.get_mut(..len)?;
            written = write(room);
            written.then_some(len)
        });
        written
    };
    if written && let Some(interface) = numbered(number) {
        interface.kick();
    }
    Ok(written)
}

/// Whether a packet waits for tunnel `number`'s program, and whether the
/// tunnel is gone.
pub fn ready(number: usize) -> (bool, bool) {
    match PIPES.get(number).map(|pipe| pipe.lock().as_ref().map(|pipe| pipe.out.oldest().is_some()))
    {
        Some(Some(waiting)) => (waiting, false),
        _ => (false, true),
    }
}

/// The packets lost to tunnel `number`'s full queues.
pub fn lost(number: usize) -> u64 {
    PIPES.get(number).and_then(|pipe| pipe.lock().as_ref().map(|pipe| pipe.lost)).unwrap_or(0)
}

/// What a program waiting for tunnel `number`'s packets waits on.
pub fn key(number: usize) -> usize {
    PIPES.get(number).map_or(0, |pipe| core::ptr::from_ref(pipe).addr())
}

/// Wakes the programs of the tunnels whose stacks gave them packets since
/// the last call: after a poll, once its stack's lock is let go.
pub fn wake_given() {
    if GIVEN.load(Ordering::Relaxed) == 0 {
        return;
    }
    let given = GIVEN.swap(0, Ordering::Relaxed);
    for number in 0..MAX_INTERFACES {
        if given & (1 << number) != 0 {
            wait::wake_all(key(number));
        }
    }
    if given != 0 {
        crate::fs::tunnel::POLLED.changed();
    }
}
