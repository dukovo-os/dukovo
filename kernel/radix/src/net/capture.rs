// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Packet captures (ADR 0346): what tcpdump(8) and the rest of libpcap's
//! programs read the frames an interface receives and sends from, as
//! Linux's packet sockets and the BSDs' BPF devices give them.
//!
//! A program with the network capability's control right opens one
//! (radix-network(2)'s `open_capture`, `crate::fs::capture`), of one
//! interface or of all of them. From then on each interface it looks at
//! has its stack's tap ([`radix_net::Tap`]) call [`see`] with every frame,
//! as it is received, before anything takes it, and as it is sent. The
//! capture's filter, classic BPF ([`radix_net::bpf`]), picks those it
//! keeps and how much of each, its snap length cuts them, and they wait
//! in its ring, each as radix-network(2)'s CaptureHeader and its bytes,
//! until its program reads them; one that finds the ring full is lost,
//! and counted, as on Linux. A loopback packet has an Ethernet header of
//! zero addresses put before it, as Linux's `lo` gives it; a tunnel's
//! packets are IP packets alone.
//!
//! An interface no capture looks at costs its stack one load a frame:
//! [`WATCHED`] says which are looked at. The captures and their rings
//! are under locks of their own, which a stack's lock may be held under
//! but which never hold one; a program is woken once the poll that gave
//! it a frame has let go of its stack ([`wake_seen`]).

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::network::{
    CAPTURE_BUFFER_DEFAULT, CAPTURE_BUFFER_MAX, CAPTURE_BUFFER_MIN, CAPTURE_LINK_ETHERNET,
    CAPTURE_LINK_RAW, CAPTURE_SNAP_MAX, CAPTURES_MAX, CaptureHeader,
};
use radix_hal::Timer;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_net::bpf::{self, Instruction, Packet};
use radix_net::{Medium, Tap};
use radix_sync::Mutex;

use super::{MAX_INTERFACES, numbered};
use crate::Arch;
use crate::sched::{self, wait};

/// Whether a capture looks at each interface, by its number: what its
/// stack's tap looks at for each frame.
static WATCHED: [AtomicBool; MAX_INTERFACES] = [const { AtomicBool::new(false) }; MAX_INTERFACES];

/// The captures open, of every program's.
static CAPTURES: Mutex<Option<Vec<Arc<Capture, Kmalloc>, Kmalloc>>> = Mutex::new(None);

/// Whether a frame went to a capture since its programs were last woken.
static SEEN: AtomicBool = AtomicBool::new(false);

/// The tap of interface `number`'s stack.
pub fn tap(number: usize) -> Tap {
    Tap { number, watched: &WATCHED[number.min(MAX_INTERFACES - 1)], see }
}

/// The bytes a frame of `captured` bytes takes in a ring, its header and
/// all, to a multiple of 8.
fn record(captured: usize) -> usize {
    (CaptureHeader::SIZE + captured).next_multiple_of(8)
}

/// The bytes of a ring are kept in pieces of this many, 64 KiB: a block
/// of frames is at most 4 MiB, and a ring of 64 MiB is many blocks, as
/// Linux's packet rings are.
const PIECE: usize = CAPTURE_BUFFER_MIN as usize;

/// Bytes in a circle, in pieces: the frames a capture keeps, oldest first.
struct Ring {
    pieces: Vec<Vec<u8, Kmalloc>, Kmalloc>,
    /// The bytes it has, in all.
    size: usize,
    /// Where the oldest starts.
    start: usize,
    /// How many bytes are kept.
    len: usize,
}

impl Ring {
    /// A ring of `size` bytes, at least one, zeroed.
    fn new(size: usize) -> Option<Ring> {
        let flags = HEAP.with(Flags::MAY_SLEEP);
        let mut pieces = Vec::new_in(flags);
        pieces.try_reserve_exact(size.div_ceil(PIECE)).ok()?;
        for at in (0..size).step_by(PIECE) {
            let piece = radix_mm::bytes::zeroed(PIECE.min(size - at), flags).ok()?;
            pieces.push_within_capacity(piece).ok()?;
        }
        Some(Ring { pieces, size, start: 0, len: 0 })
    }

    fn room(&self) -> usize {
        self.size - self.len
    }

    /// Writes `data` at `at` bytes past the oldest, round the end.
    fn put(&mut self, at: usize, mut data: &[u8]) {
        let mut from = (self.start + at) % self.size;
        while !data.is_empty() {
            let piece = &mut self.pieces[from / PIECE];
            let offset = from % PIECE;
            let len = data.len().min(piece.len() - offset);
            piece[offset..offset + len].copy_from_slice(&data[..len]);
            data = &data[len..];
            from = (from + len) % self.size;
        }
    }

    /// Hands `each` the bytes from `at` past the oldest, `len` of them,
    /// in runs, round the end.
    fn runs<E>(
        &self,
        at: usize,
        mut len: usize,
        mut each: impl FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut from = (self.start + at) % self.size;
        while len > 0 {
            let piece = &self.pieces[from / PIECE];
            let offset = from % PIECE;
            let run = len.min(piece.len() - offset);
            each(&piece[offset..offset + run])?;
            len -= run;
            from = (from + run) % self.size;
        }
        Ok(())
    }

    /// The header of the oldest frame, if one is kept.
    fn oldest(&self) -> Option<[u8; CaptureHeader::SIZE]> {
        if self.len == 0 {
            return None;
        }
        let mut header = [0; CaptureHeader::SIZE];
        let mut filled = 0;
        let _ = self.runs(0, CaptureHeader::SIZE, |run| {
            header[filled..filled + run.len()].copy_from_slice(run);
            filled += run.len();
            Ok::<(), ()>(())
        });
        Some(header)
    }

    /// Forgets the oldest `len` bytes.
    fn drop_oldest(&mut self, len: usize) {
        self.start = (self.start + len) % self.size;
        self.len -= len;
        if self.len == 0 {
            self.start = 0;
        }
    }

    fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }
}

/// What a capture keeps, under its lock.
struct State {
    ring: Ring,
    /// Its filter, which [`bpf::check`] took.
    filter: Option<Vec<Instruction, Kmalloc>>,
    /// The frames it kept.
    received: u64,
    /// Of those, the frames lost as its ring was full.
    dropped: u64,
    /// Whether its interface was taken away.
    gone: bool,
}

/// A packet capture.
pub struct Capture {
    /// The interface it looks at, by its number: `None` for every one.
    interface: Option<usize>,
    /// The most bytes of a frame it keeps.
    snap: u32,
    state: Mutex<State>,
    /// Whether a frame came for it since its program was last woken.
    seen: AtomicBool,
    /// Whether a frame waits, and whether its interface was taken away:
    /// what a waiting reader looks at with the scheduler's table locked,
    /// where its lock may not be taken. Each is set under it.
    waiting: AtomicBool,
    ended: AtomicBool,
}

/// A frame as a capture sees it.
struct Frame<'a> {
    packet: Packet<'a>,
    /// Its CaptureHeader's `interface`, `link` and `outgoing`.
    index: u32,
    link: u16,
    outgoing: bool,
    /// When it was seen.
    time: u64,
}

impl Capture {
    /// Whether it looks at interface `number`.
    fn looks_at(&self, number: usize) -> bool {
        self.interface.is_none_or(|interface| interface == number)
    }

    /// Keeps `frame`, if its filter wants it and its ring has room: whether
    /// it kept it.
    fn keep(&self, frame: &Frame<'_>) -> bool {
        let mut state = self.state.lock();
        if state.gone {
            return false;
        }
        let wanted = match &state.filter {
            Some(program) => bpf::run(program, frame.packet),
            None => u32::MAX,
        };
        if wanted == 0 {
            return false;
        }
        state.received += 1;
        let length = frame.packet.len();
        let captured = length.min(self.snap as usize).min(wanted as usize);
        let size = record(captured);
        if state.ring.room() < size {
            state.dropped += 1;
            return false;
        }
        let mut header = [0u8; CaptureHeader::SIZE];
        let field = |header: &mut [u8; CaptureHeader::SIZE], at: usize, bytes: &[u8]| {
            header[at..at + bytes.len()].copy_from_slice(bytes);
        };
        field(&mut header, CaptureHeader::OFFSET_TIME, &frame.time.to_le_bytes());
        // A frame is shorter than 4 GiB.
        field(&mut header, CaptureHeader::OFFSET_LENGTH, &(length as u32).to_le_bytes());
        field(&mut header, CaptureHeader::OFFSET_CAPTURED, &(captured as u32).to_le_bytes());
        field(&mut header, CaptureHeader::OFFSET_INTERFACE, &frame.index.to_le_bytes());
        field(&mut header, CaptureHeader::OFFSET_LINK, &frame.link.to_le_bytes());
        header[CaptureHeader::OFFSET_OUTGOING] = u8::from(frame.outgoing);
        let ring = &mut state.ring;
        let at = ring.len;
        ring.put(at, &header);
        let head = &frame.packet.head[..captured.min(frame.packet.head.len())];
        ring.put(at + header.len(), head);
        let body = &frame.packet.body[..captured - head.len()];
        ring.put(at + header.len() + head.len(), body);
        let padding = size - CaptureHeader::SIZE - captured;
        ring.put(at + CaptureHeader::SIZE + captured, &[0; 8][..padding]);
        ring.len += size;
        self.waiting.store(true, Ordering::Release);
        true
    }

    /// Whether a frame waits, and whether its interface was taken away:
    /// atomics alone, read without its lock.
    pub fn ready(&self) -> (bool, bool) {
        (self.waiting.load(Ordering::Acquire), self.ended.load(Ordering::Acquire))
    }

    /// The frames it kept and those it lost, since it opened or its
    /// filter was last set.
    pub fn counts(&self) -> (u64, u64) {
        let state = self.state.lock();
        (state.received, state.dropped)
    }

    /// Gives it `filter`, or none, in place of the one it had: the frames
    /// waiting are thrown away, and the counts start again.
    pub fn set_filter(&self, filter: Option<Vec<Instruction, Kmalloc>>) {
        let old = {
            let mut state = self.state.lock();
            state.ring.clear();
            self.waiting.store(false, Ordering::Release);
            state.received = 0;
            state.dropped = 0;
            core::mem::replace(&mut state.filter, filter)
        };
        drop(old);
    }

    /// Hands `put` the frames waiting, oldest first, while they fit in
    /// `room` bytes, each its header, bytes and padding at the offset it
    /// goes at, as runs; the first is cut to `room` if it alone does not
    /// fit, as long as `room` holds its header. Each frame `put` takes all
    /// of goes. Returns the bytes taken, or `None` if none were; `put`
    /// says, by its error, why it stopped.
    ///
    /// # Errors
    ///
    /// `put`'s, when it took no frame; else the bytes it took.
    pub fn take<E>(
        &self,
        room: usize,
        mut put: impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<Option<usize>, E> {
        let mut state = self.state.lock();
        let mut taken = 0;
        while let Some(mut header) = state.ring.oldest() {
            let word = |at: usize| {
                u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
            };
            let captured = word(CaptureHeader::OFFSET_CAPTURED) as usize;
            let size = record(captured);
            // What of it goes in the buffer: all of it, or the first of it
            // alone, cut.
            let len = if taken + size <= room {
                size
            } else if taken == 0 && room >= CaptureHeader::SIZE {
                let kept = (room - CaptureHeader::SIZE).min(captured);
                let at = CaptureHeader::OFFSET_CAPTURED;
                header[at..at + 4].copy_from_slice(&(kept as u32).to_le_bytes());
                CaptureHeader::SIZE + kept
            } else {
                break;
            };
            let copied = (|| {
                put(taken, &header)?;
                let mut offset = taken + header.len();
                state.ring.runs(CaptureHeader::SIZE, len - header.len(), |run| {
                    put(offset, run)?;
                    offset += run.len();
                    Ok(())
                })
            })();
            if let Err(err) = copied {
                return if taken == 0 { Err(err) } else { Ok(Some(taken)) };
            }
            state.ring.drop_oldest(size);
            self.waiting.store(state.ring.len > 0, Ordering::Release);
            taken += len;
        }
        Ok((taken > 0).then_some(taken))
    }

    /// What a program waiting for its frames waits on.
    pub fn key(&self) -> usize {
        core::ptr::from_ref(self).addr()
    }
}

/// Why a capture could not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Its interface is not there.
    NoDevice,
    /// Its snap length or buffer is out of range.
    Invalid,
    /// [`CAPTURES_MAX`] are open.
    Busy,
    NoMemory,
}

impl OpenError {
    /// Its status word.
    pub fn status(self) -> Status {
        match self {
            OpenError::NoDevice => FileError::NoDevice.status(0),
            OpenError::Invalid => Kernel::InvalidArgument.status(0),
            OpenError::Busy => Kernel::Busy.status(0),
            OpenError::NoMemory => Kernel::NoMemory.status(0),
        }
    }
}

/// Opens a capture of interface `interface`, by its number, or of every
/// one for `None`, keeping `snap` bytes of each frame and `buffer` bytes
/// of frames (0 for [`CAPTURE_BUFFER_DEFAULT`]).
///
/// # Errors
///
/// Why it could not.
pub fn open(
    interface: Option<usize>,
    snap: u32,
    buffer: u32,
) -> Result<Arc<Capture, Kmalloc>, OpenError> {
    if snap == 0 || u64::from(snap) > CAPTURE_SNAP_MAX {
        return Err(OpenError::Invalid);
    }
    let buffer = match u64::from(buffer) {
        0 => CAPTURE_BUFFER_DEFAULT,
        buffer if (CAPTURE_BUFFER_MIN..=CAPTURE_BUFFER_MAX).contains(&buffer) => buffer,
        _ => return Err(OpenError::Invalid),
    };
    if interface.is_some_and(|number| numbered(number).is_none_or(super::Interface::gone)) {
        return Err(OpenError::NoDevice);
    }
    // At most CAPTURE_BUFFER_MAX, to a multiple of 8.
    let ring = Ring::new((buffer as usize) & !7).ok_or(OpenError::NoMemory)?;
    let state = State { ring, filter: None, received: 0, dropped: 0, gone: false };
    let capture = Capture {
        interface,
        snap,
        state: Mutex::new(state),
        seen: AtomicBool::new(false),
        waiting: AtomicBool::new(false),
        ended: AtomicBool::new(false),
    };
    let capture =
        Arc::try_new_in(capture, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| OpenError::NoMemory)?;
    let mut captures = CAPTURES.lock();
    let captures = captures.get_or_insert_with(|| Vec::new_in(HEAP.with(Flags::MAY_SLEEP)));
    if captures.len() as u64 >= CAPTURES_MAX {
        return Err(OpenError::Busy);
    }
    captures.try_reserve(1).map_err(|_| OpenError::NoMemory)?;
    captures.push_within_capacity(Arc::clone(&capture)).map_err(|_| OpenError::NoMemory)?;
    watch(captures);
    Ok(capture)
}

/// Closes `capture`, as its file's last description goes.
pub fn close(capture: &Capture) {
    let mut captures = CAPTURES.lock();
    let Some(captures) = captures.as_mut() else { return };
    if let Some(at) = captures.iter().position(|kept| core::ptr::eq(&raw const **kept, capture)) {
        captures.swap_remove(at);
    }
    watch(captures);
}

/// The keys of the captures `pick` picks, which [`wake`] wakes once the
/// captures' lock is let go.
fn keys(pick: impl Fn(&Capture) -> bool) -> [usize; CAPTURES_MAX as usize] {
    let mut keys = [0; CAPTURES_MAX as usize];
    if let Some(captures) = CAPTURES.lock().as_ref() {
        let picked = captures.iter().filter(|capture| pick(capture));
        for (key, capture) in keys.iter_mut().zip(picked) {
            *key = capture.key();
        }
    }
    keys
}

/// Wakes the programs waiting on `keys`, and the polls of captures.
fn wake(keys: &[usize]) {
    for &key in keys.iter().filter(|&&key| key != 0) {
        wait::wake_all(key);
    }
    crate::fs::capture::POLLED.changed();
}

/// Sets [`WATCHED`] from `captures`.
fn watch(captures: &[Arc<Capture, Kmalloc>]) {
    for (number, watched) in WATCHED.iter().enumerate() {
        let looked =
            captures.iter().any(|capture| capture.looks_at(number) && !capture.state.lock().gone);
        watched.store(looked, Ordering::Relaxed);
    }
}

/// Interface `number` was taken away: the captures of it alone find so,
/// once they have read what waits.
pub fn gone(number: usize) {
    let of = |capture: &Capture| capture.interface == Some(number);
    if let Some(captures) = CAPTURES.lock().as_ref() {
        for capture in captures.iter().filter(|capture| of(capture)) {
            capture.state.lock().gone = true;
            capture.ended.store(true, Ordering::Release);
        }
        watch(captures);
    }
    wake(&keys(of));
}

/// The Ethernet header Linux's `lo` gives a loopback packet: zero
/// addresses, and the type of its IP version.
fn loopback_header(packet: &[u8]) -> [u8; 14] {
    let mut header = [0; 14];
    let kind: u16 = match packet.first().map(|byte| byte >> 4) {
        Some(4) => 0x0800,
        Some(6) => 0x86dd,
        _ => 0,
    };
    header[12..].copy_from_slice(&kind.to_be_bytes());
    header
}

/// The time of day now, or the time since the machine started while it
/// is not known.
fn now() -> u64 {
    <Arch as Timer>::wall_clock_ns().unwrap_or_else(sched::now_ns)
}

/// The tap: interface `number` received `frame`, of `medium`, or sent it
/// if `outgoing`. Called with its stack's lock held.
fn see(number: usize, frame: &[u8], medium: Medium, outgoing: bool) {
    let lo = loopback_header(frame);
    let (head, link): (&[u8], u64) = match medium {
        Medium::Ethernet => (&[], CAPTURE_LINK_ETHERNET),
        Medium::Ip if number == 0 => (&lo, CAPTURE_LINK_ETHERNET),
        Medium::Ip => (&[], CAPTURE_LINK_RAW),
    };
    let frame = Frame {
        packet: Packet { head, body: frame },
        // At most MAX_INTERFACES; the links' numbers are pcap's.
        index: number as u32 + 1,
        link: link as u16,
        outgoing,
        time: now(),
    };
    let captures = CAPTURES.lock();
    let Some(captures) = captures.as_ref() else { return };
    for capture in captures.iter().filter(|capture| capture.looks_at(number)) {
        if capture.keep(&frame) {
            capture.seen.store(true, Ordering::Relaxed);
            SEEN.store(true, Ordering::Relaxed);
        }
    }
}

/// Wakes the programs of the captures that were given frames since the
/// last call: after a poll, once its stack's lock is let go.
pub fn wake_seen() {
    if !SEEN.load(Ordering::Relaxed) || !SEEN.swap(false, Ordering::Relaxed) {
        return;
    }
    wake(&keys(|capture| capture.seen.swap(false, Ordering::Relaxed)));
}
