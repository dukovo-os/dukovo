// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bridges and VLANs (ADR 0333): interfaces whose frames go through other
//! interfaces' links, as Linux's `bridge` and `vlan` links do.
//!
//! A VLAN interface, `net0.7`, is on a parent's link: what it sends goes
//! out of its parent tagged for its VLAN, and the frames its parent
//! receives tagged for it come to it untagged. It has its parent's MAC
//! address. A bridge, `br0`, joins its ports into one network: a frame a
//! port receives goes on to the port its destination was last heard on,
//! or to every other port, and to the bridge's own stack when it is for
//! the bridge or for every machine; what the bridge's stack sends goes out
//! of the port its destination was heard on, or of every port. A port
//! has no addresses of its own while it is one, as on Linux; the bridge
//! takes the first port's MAC address, as Linux's takes its lowest.
//!
//! Each interface's frames pass through [`hooks`] (radix-net's
//! [`Hooks`]): its link's frames for the interfaces above it are taken
//! out of what it receives into those interfaces' queues, and theirs to
//! send come from its queue. The queues are taken under one lock, which
//! a stack's lock may be held under but which never holds one: the
//! interfaces never wait for one another's stacks. An interface given a
//! frame is woken once the poll that gave it has let go of its stack
//! ([`wake_given`]).
//!
//! A bridge or VLAN is made from a program, through the network
//! capability's control right ([`add`], radix-network(2)'s `add_link`),
//! and its stack on a thread of its own with a stack large enough to make
//! one on. One taken away (`remove_link`) is gone from every listing and
//! takes no more frames; its place among the interfaces is not used
//! again until the machine restarts.
//!
//! A tunnel (ADR 0339) is made and taken away the same way, its stack on
//! a thread of its own; its packets go through its program
//! ([`super::tunnel`]), not through other interfaces.

use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use radix_klog::{TextBuf, info};
use radix_mm::Kmalloc;
use radix_net::layer2::{self, FDB_ENTRIES, Fdb, Heard};
use radix_net::{Hooks, TAGGED_FRAME};
use radix_sync::Mutex;

use super::{Card, MAX_INTERFACES, kmalloc, numbered};
use crate::sched::{self, Options, Policy, wait};

/// The frames each queue holds: a TCP window's worth of full ones, as
/// Linux's default transmit queue holds a thousand.
const QUEUE_FRAMES: usize = 64;

/// What a bridge, VLAN or tunnel interface is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A bridge, which its ports join.
    Bridge,
    /// A VLAN on the interface numbered `parent`, with the VLAN number
    /// `id`.
    Vlan { parent: usize, id: u16 },
    /// A tunnel, whose packets a program carries (ADR 0339).
    Tunnel,
}

/// The link of a bridge or VLAN interface: frames from its queue, and to
/// those of the interfaces beneath it.
pub struct Virtual {
    /// Its interface's number.
    pub number: usize,
    /// Its MAC address.
    pub mac: [u8; 6],
}

impl Virtual {
    /// The next frame for its stack, copied into `frame`: its length.
    pub fn receive(&self, frame: &mut [u8]) -> Option<usize> {
        let mut state = STATE.lock();
        state.up.get_mut(self.number)?.as_mut()?.pop(frame)
    }

    /// Sends `frame`, its stack's, through the interfaces beneath it:
    /// false if none took it.
    pub fn send(&self, frame: &[u8]) -> bool {
        let now = sched::now_ns();
        let mut state = STATE.lock();
        match state.roles.get(self.number).copied().flatten() {
            Some(Role::Vlan { parent, id }) => {
                state.give_out(parent, |out| layer2::tag(frame, id, out))
            }
            Some(Role::Bridge) => state.bridge_out(self.number, frame, None, now),
            Some(Role::Tunnel) | None => false,
        }
    }
}

/// A ring of frames, each up to [`TAGGED_FRAME`] bytes.
pub(super) struct Queue {
    bytes: Vec<u8, Kmalloc>,
    lens: [u16; QUEUE_FRAMES],
    first: usize,
    count: usize,
}

impl Queue {
    pub(super) fn new() -> Option<Queue> {
        let bytes = radix_mm::bytes::zeroed(QUEUE_FRAMES * TAGGED_FRAME, kmalloc()).ok()?;
        Some(Queue { bytes, lens: [0; QUEUE_FRAMES], first: 0, count: 0 })
    }

    /// Puts the frame `write` writes into the next free slot, given the
    /// slot and returning the frame's length: false if there is no free
    /// slot, or `write` wrote nothing.
    pub(super) fn push(&mut self, write: impl FnOnce(&mut [u8]) -> Option<usize>) -> bool {
        if self.count == QUEUE_FRAMES {
            return false;
        }
        let slot = (self.first + self.count) % QUEUE_FRAMES;
        let room = &mut self.bytes[slot * TAGGED_FRAME..(slot + 1) * TAGGED_FRAME];
        let Some(len) = write(room).filter(|&len| len <= TAGGED_FRAME) else { return false };
        // At most TAGGED_FRAME, which fits.
        self.lens[slot] = len as u16;
        self.count += 1;
        true
    }

    /// Copies the oldest frame into `out`, cut to its length, and lets it
    /// go: its length.
    pub(super) fn pop(&mut self, out: &mut [u8]) -> Option<usize> {
        let len = self.oldest()?.len().min(out.len());
        let slot = self.first;
        out[..len].copy_from_slice(&self.bytes[slot * TAGGED_FRAME..slot * TAGGED_FRAME + len]);
        self.drop_oldest();
        Some(len)
    }

    /// The oldest frame, if one waits.
    pub(super) fn oldest(&self) -> Option<&[u8]> {
        if self.count == 0 {
            return None;
        }
        let at = self.first * TAGGED_FRAME;
        Some(&self.bytes[at..at + usize::from(self.lens[self.first])])
    }

    /// Lets the oldest frame go.
    pub(super) fn drop_oldest(&mut self) {
        if self.count > 0 {
            self.first = (self.first + 1) % QUEUE_FRAMES;
            self.count -= 1;
        }
    }

    /// Whether it has no room for another frame.
    pub(super) fn is_full(&self) -> bool {
        self.count == QUEUE_FRAMES
    }

    /// Lets every frame go.
    pub(super) fn clear(&mut self) {
        self.count = 0;
    }
}

/// A bridge's table of where it heard each machine.
type Table = Fdb<Vec<Option<Heard>, Kmalloc>>;

/// The bridges and VLANs, their queues and what the bridges heard.
struct State {
    roles: [Option<Role>; MAX_INTERFACES],
    /// The bridge each interface is a port of.
    master: [Option<usize>; MAX_INTERFACES],
    /// Each bridge's and VLAN's own address.
    macs: [[u8; 6]; MAX_INTERFACES],
    /// The frames for each bridge's and VLAN's stack.
    up: [Option<Queue>; MAX_INTERFACES],
    /// The frames for each interface's link to send, of the interfaces
    /// above it.
    out: [Option<Queue>; MAX_INTERFACES],
    fdb: [Option<Table>; MAX_INTERFACES],
    /// Frames lost because the interface's queue was full.
    lost: [u64; MAX_INTERFACES],
}

static STATE: Mutex<State> = Mutex::new(State {
    roles: [None; MAX_INTERFACES],
    master: [None; MAX_INTERFACES],
    macs: [[0; 6]; MAX_INTERFACES],
    up: [const { None }; MAX_INTERFACES],
    out: [const { None }; MAX_INTERFACES],
    fdb: [const { None }; MAX_INTERFACES],
    lost: [0; MAX_INTERFACES],
});

/// The interfaces given frames since they were last woken, a bit each.
static GIVEN: AtomicU32 = AtomicU32::new(0);

/// Whether any bridge or VLAN was made: until one is, no frame is looked
/// at on its way.
static ANY: AtomicBool = AtomicBool::new(false);

impl State {
    /// Puts the frame `write` writes in interface `number`'s queue of
    /// frames to send, and marks it to be woken: false if it has none
    /// or it is full.
    fn give_out(&mut self, number: usize, write: impl FnOnce(&mut [u8]) -> Option<usize>) -> bool {
        let Some(queue) = self.out.get_mut(number).and_then(Option::as_mut) else { return false };
        let given = queue.push(write);
        if given {
            GIVEN.fetch_or(1 << number, Ordering::Relaxed);
        } else {
            self.lost[number] += 1;
        }
        given
    }

    /// Puts `frame` in the queue of interface `number`'s stack, likewise.
    fn give_up(&mut self, number: usize, write: impl FnOnce(&mut [u8]) -> Option<usize>) -> bool {
        let Some(queue) = self.up.get_mut(number).and_then(Option::as_mut) else { return false };
        let given = queue.push(write);
        if given {
            GIVEN.fetch_or(1 << number, Ordering::Relaxed);
        } else {
            self.lost[number] += 1;
        }
        given
    }

    /// Sends `frame` on through bridge `bridge`, which came in on port
    /// `from` or from the bridge's own stack: up, out of one port, or of
    /// every other. False if it went nowhere.
    fn bridge_out(&mut self, bridge: usize, frame: &[u8], from: Option<usize>, now: u64) -> bool {
        let Some((to, source)) = layer2::addresses(frame) else { return false };
        let own = self.macs[bridge];
        let Some(fdb) = self.fdb[bridge].as_mut() else { return false };
        if let Some(port) = from {
            fdb.learn(source, port, now);
        }
        let forward = layer2::forward(fdb, own, to, from, now);
        let copy = |out: &mut [u8]| {
            let out = out.get_mut(..frame.len())?;
            out.copy_from_slice(frame);
            Some(frame.len())
        };
        let mut went = false;
        if forward.up {
            went |= self.give_up(bridge, copy);
        }
        if let Some(port) = forward.port {
            went |= self.give_out(port, copy);
        }
        if forward.flood {
            for port in 0..MAX_INTERFACES {
                if self.master[port] == Some(bridge) && Some(port) != from {
                    went |= self.give_out(port, copy);
                }
            }
        }
        went
    }
}

/// What interface `number`'s frames pass through.
pub fn hooks(number: usize) -> Hooks {
    Hooks { number, sift, next, admit: super::firewall::admit }
}

/// Takes `frame`, which interface `number`'s link received, if it is for
/// a VLAN on it or it is a bridge's port.
fn sift(number: usize, frame: &[u8]) -> bool {
    if !ANY.load(Ordering::Relaxed) {
        return false;
    }
    let now = sched::now_ns();
    let mut state = STATE.lock();
    if let Some(id) = layer2::vlan_of(frame) {
        let vlan =
            state.roles.iter().position(|role| *role == Some(Role::Vlan { parent: number, id }));
        if let Some(vlan) = vlan {
            state.give_up(vlan, |out| layer2::untag(frame, out));
            return true;
        }
    }
    match state.master.get(number).copied().flatten() {
        Some(bridge) => {
            state.bridge_out(bridge, frame, Some(number), now);
            true
        }
        None => false,
    }
}

/// The next frame of the interfaces above interface `number` for its link
/// to send, copied into `frame`.
fn next(number: usize, frame: &mut [u8]) -> Option<usize> {
    if !ANY.load(Ordering::Relaxed) {
        return None;
    }
    STATE.lock().out.get_mut(number)?.as_mut()?.pop(frame)
}

/// Wakes the interfaces given frames since the last call: after a poll,
/// once its stack's lock is let go.
pub fn wake_given() {
    let given = GIVEN.swap(0, Ordering::Relaxed);
    if given == 0 {
        return;
    }
    for number in 0..MAX_INTERFACES {
        if given & (1 << number) != 0
            && let Some(interface) = numbered(number)
        {
            interface.wake();
        }
    }
}

/// The frames lost to interface `number`'s full queues since the
/// machine started.
pub fn lost(number: usize) -> u64 {
    STATE.lock().lost.get(number).copied().unwrap_or(0)
}

/// What interface `number` is, if a bridge or VLAN.
pub fn role(number: usize) -> Option<Role> {
    STATE.lock().roles.get(number).copied().flatten()
}

/// The bridge interface `number` is a port of, if one.
pub fn master(number: usize) -> Option<usize> {
    STATE.lock().master.get(number).copied().flatten()
}

/// The ports of bridge `bridge`, by number, a bit each.
pub fn ports(bridge: usize) -> u32 {
    let state = STATE.lock();
    (0..MAX_INTERFACES)
        .filter(|&port| state.master[port] == Some(bridge))
        .fold(0, |all, port| all | 1 << port)
}

/// Calls `f` with each machine bridge `bridge` heard, and where.
pub fn heard(
    bridge: usize,
    mut f: impl FnMut(Heard, u64) -> core::fmt::Result,
) -> core::fmt::Result {
    let now = sched::now_ns();
    let state = STATE.lock();
    let Some(fdb) = state.fdb.get(bridge).and_then(Option::as_ref) else { return Ok(()) };
    for heard in fdb.heard(now) {
        f(heard, now)?;
    }
    Ok(())
}

/// Why a bridge or VLAN could not be made, joined or taken away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The name is empty, too long, or not one an interface can have.
    Name,
    /// An interface has the name, or the parent has a VLAN with the
    /// number.
    Taken,
    /// There is no such interface, or it is not one that can be a parent
    /// or a port or a bridge.
    NoDevice,
    /// The VLAN number is not 1 to 4094.
    Vlan,
    /// It cannot be done: a bridge in a bridge, `lo`, or taking a card
    /// away.
    NotSupported,
    /// No room for another interface.
    NoRoom,
    /// No memory for its stack or queues.
    NoMemory,
}

/// What a program asked the maker thread to make.
#[derive(Clone)]
struct Asked {
    role: Role,
    name: TextBuf<16>,
}

/// The ask the maker thread works on, and its answer.
static ASKED: Mutex<Option<Asked>> = Mutex::new(None);
static ANSWER: Mutex<Option<Result<usize, LinkError>>> = Mutex::new(None);
/// Whether an ask is under way: one at a time.
static ASKING: AtomicBool = AtomicBool::new(false);
static ANSWERED: AtomicUsize = AtomicUsize::new(0);

/// The pages of the maker thread's stack: a stack is built on it before
/// it goes in its place, as at boot (radix::net::add_interface).
const MAKER_STACK_PAGES: usize = 16;

/// Whether `name` is one an interface may have: 1 to 15 bytes, as Linux's
/// IFNAMSIZ allows, of letters, digits, `.`, `-` and `_`, and not `.` or
/// `..`.
pub fn name_ok(name: &[u8]) -> bool {
    (1..=15).contains(&name.len())
        && name != b"."
        && name != b".."
        && name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

/// Makes a bridge, or a VLAN on the interface numbered `parent`, named
/// `name`: its number.
///
/// # Errors
///
/// As [`LinkError`] says.
pub fn add(role: Role, name: &[u8]) -> Result<usize, LinkError> {
    if !name_ok(name) {
        return Err(LinkError::Name);
    }
    let text = core::str::from_utf8(name).map_err(|_| LinkError::Name)?;
    let mut buffer = TextBuf::<16>::new();
    let _ = buffer.write_str(text);
    // One ask at a time, which the maker thread answers, so that two
    // cannot both take a name or a VLAN number.
    wait::wait_until(key(), "netlink", || !ASKING.load(Ordering::Acquire));
    while ASKING.swap(true, Ordering::AcqRel) {
        wait::wait_until(key(), "netlink", || !ASKING.load(Ordering::Acquire));
    }
    let result = free(role, text).and_then(|()| ask(Asked { role, name: buffer }));
    ASKING.store(false, Ordering::Release);
    wait::wake_all(key());
    result
}

/// Whether an interface named `name` with role `role` can be made.
fn free(role: Role, name: &str) -> Result<(), LinkError> {
    if super::interfaces().any(|interface| interface.name.as_str() == name) {
        return Err(LinkError::Taken);
    }
    if let Role::Vlan { parent, id } = role {
        let found =
            numbered(parent).filter(|interface| !interface.gone()).ok_or(LinkError::NoDevice)?;
        // A tunnel has no Ethernet frames to tag.
        if found.is_loopback() || self::role(parent) == Some(Role::Tunnel) {
            return Err(LinkError::NotSupported);
        }
        if !(layer2::VLAN_FIRST..=layer2::VLAN_LAST).contains(&id) {
            return Err(LinkError::Vlan);
        }
        if STATE.lock().roles.contains(&Some(role)) {
            return Err(LinkError::Taken);
        }
    }
    Ok(())
}

/// Has the maker thread make what `asked` asks for, while this thread
/// has the one ask: its number.
fn ask(asked: Asked) -> Result<usize, LinkError> {
    *ASKED.lock() = Some(asked);
    let before = ANSWERED.load(Ordering::Acquire);
    let options = Options::new().policy(Policy::Fair { nice: -5 }).stack_pages(MAKER_STACK_PAGES);
    let result = match sched::spawn("netlink", make, 0, options) {
        Ok(thread) => {
            sched::release(&thread);
            wait::wait_until(key(), "netlink", || ANSWERED.load(Ordering::Acquire) != before);
            ANSWER.lock().take().unwrap_or(Err(LinkError::NoMemory))
        }
        Err(_) => Err(LinkError::NoMemory),
    };
    ASKED.lock().take();
    result
}

/// The wait key of the asks.
fn key() -> usize {
    core::ptr::addr_of!(ASKING) as usize
}

/// The maker thread: makes what was asked, and answers.
fn make(_: usize) {
    let asked = ASKED.lock().clone();
    let answer = match asked {
        Some(asked) => make_link(&asked),
        None => Err(LinkError::NoMemory),
    };
    *ANSWER.lock() = Some(answer);
    ANSWERED.fetch_add(1, Ordering::AcqRel);
    wait::wake_all(key());
}

/// Makes the interface `asked` asks for, with its queue and its thread.
fn make_link(asked: &Asked) -> Result<usize, LinkError> {
    let (mac, parent_out) = match asked.role {
        // A tunnel has neither a MAC address nor frames.
        Role::Tunnel => return make_tunnel(&asked.name),
        Role::Bridge => (super::fallback_mac(), None),
        Role::Vlan { parent, .. } => {
            let parent_mac = numbered(parent)
                .and_then(|interface| interface.stack.lock().mac())
                .ok_or(LinkError::NoDevice)?;
            let out = if STATE.lock().out[parent].is_none() {
                Some(Queue::new().ok_or(LinkError::NoMemory)?)
            } else {
                None
            };
            (parent_mac.0, out)
        }
    };
    let up = Queue::new().ok_or(LinkError::NoMemory)?;
    let fdb = match asked.role {
        Role::Bridge => {
            let mut entries = Vec::new_in(kmalloc());
            entries.try_reserve_exact(FDB_ENTRIES).map_err(|_| LinkError::NoMemory)?;
            while entries.len() < FDB_ENTRIES && entries.push_within_capacity(None).is_ok() {}
            Some(Fdb::new(entries))
        }
        Role::Vlan { .. } | Role::Tunnel => None,
    };
    let interface = super::add_virtual(&asked.name, mac).map_err(|_| LinkError::NoRoom)?;
    let number = interface.number;
    {
        let mut state = STATE.lock();
        state.roles[number] = Some(asked.role);
        state.macs[number] = mac;
        state.up[number] = Some(up);
        state.fdb[number] = fdb;
        if let Role::Vlan { parent, .. } = asked.role
            && state.out[parent].is_none()
        {
            state.out[parent] = parent_out;
        }
    }
    ANY.store(true, Ordering::Relaxed);
    match asked.role {
        Role::Bridge => info!("Net: {} is a bridge", interface.name),
        Role::Vlan { parent, id } => info!(
            "Net: {} is VLAN {id} on {}",
            interface.name,
            numbered(parent).map_or("?", |parent| parent.name.as_str())
        ),
        // Made by make_tunnel.
        Role::Tunnel => {}
    }
    super::run_thread(interface).map_err(|_| LinkError::NoMemory)?;
    Ok(number)
}

/// Makes a tunnel named `name`, with its pipe and its thread.
fn make_tunnel(name: &TextBuf<16>) -> Result<usize, LinkError> {
    let pipe = super::tunnel::Pipe::new().ok_or(LinkError::NoMemory)?;
    let interface = super::add_tunnel(name).map_err(|_| LinkError::NoRoom)?;
    let number = interface.number;
    STATE.lock().roles[number] = Some(Role::Tunnel);
    super::tunnel::install(number, pipe);
    info!("Net: {} is a tunnel", interface.name);
    super::run_thread(interface).map_err(|_| LinkError::NoMemory)?;
    Ok(number)
}

/// Makes the interface numbered `port` a port of bridge `bridge`, or of
/// none for `None`.
///
/// # Errors
///
/// As [`LinkError`] says.
pub fn set_master(port: usize, bridge: Option<usize>) -> Result<(), LinkError> {
    let interface =
        numbered(port).filter(|interface| !interface.gone()).ok_or(LinkError::NoDevice)?;
    // A tunnel has no Ethernet frames for a bridge.
    if interface.is_loopback() || matches!(role(port), Some(Role::Bridge | Role::Tunnel)) {
        return Err(LinkError::NotSupported);
    }
    let queue = match bridge {
        Some(bridge) => {
            let found = numbered(bridge).filter(|bridge| !bridge.gone());
            if found.is_none() || role(bridge) != Some(Role::Bridge) {
                return Err(LinkError::NoDevice);
            }
            if STATE.lock().out[port].is_none() {
                Some(Queue::new().ok_or(LinkError::NoMemory)?)
            } else {
                None
            }
        }
        None => None,
    };
    let mac = interface.stack.lock().mac();
    let (was, first) = {
        let mut state = STATE.lock();
        if state.out[port].is_none() {
            state.out[port] = queue;
        }
        let was = core::mem::replace(&mut state.master[port], bridge);
        if let Some(was) = was
            && let Some(fdb) = state.fdb[was].as_mut()
        {
            fdb.forget(port);
        }
        let first = bridge.filter(|&bridge| {
            (0..MAX_INTERFACES).filter(|&other| state.master[other] == Some(bridge)).count() == 1
        });
        if let (Some(bridge), Some(mac)) = (first, mac) {
            state.macs[bridge] = mac.0;
        }
        (was, first)
    };
    // A port has no addresses: they are the bridge's.
    if bridge.is_some() {
        let mut stack = interface.stack.lock();
        stack.configure(&[], None);
        stack.configure_ipv6(None, &[], None);
    }
    // The bridge takes its first port's address.
    if let (Some(bridge), Some(mac)) = (first, mac)
        && let Some(bridge) = numbered(bridge)
    {
        let mut stack = bridge.stack.lock();
        if let Card::Virtual(link) = stack.link() {
            link.mac = mac.0;
        }
        stack.set_mac(mac.0);
    }
    match (was, bridge) {
        (_, Some(bridge)) => info!(
            "Net: {} is a port of {}",
            interface.name,
            numbered(bridge).map_or("?", |bridge| bridge.name.as_str())
        ),
        (Some(_), None) => info!("Net: {} is a port of no bridge", interface.name),
        (None, None) => {}
    }
    super::changed();
    Ok(())
}

/// Takes the bridge, VLAN or tunnel numbered `number` away: its ports
/// leave it, and so do the VLANs on it; a tunnel's program reads and
/// writes no more packets.
///
/// # Errors
///
/// As [`LinkError`] says: a card cannot be taken away.
pub fn remove(number: usize) -> Result<(), LinkError> {
    let interface =
        numbered(number).filter(|interface| !interface.gone()).ok_or(LinkError::NoDevice)?;
    if role(number).is_none() {
        return Err(LinkError::NotSupported);
    }
    // The VLANs on it go first.
    let vlans: Vec<usize, Kmalloc> = {
        let state = STATE.lock();
        let mut vlans = Vec::new_in(kmalloc());
        if vlans.try_reserve_exact(MAX_INTERFACES).is_err() {
            return Err(LinkError::NoMemory);
        }
        for (at, role) in state.roles.iter().enumerate() {
            if matches!(role, Some(Role::Vlan { parent, .. }) if *parent == number) {
                let _ = vlans.push_within_capacity(at);
            }
        }
        vlans
    };
    for vlan in vlans {
        remove(vlan)?;
    }
    if role(number) == Some(Role::Tunnel) {
        super::tunnel::remove(number);
    }
    {
        let mut state = STATE.lock();
        for master in &mut state.master {
            if *master == Some(number) {
                *master = None;
            }
        }
        state.roles[number] = None;
        state.up[number] = None;
        state.fdb[number] = None;
        if let Some(master) = state.master[number].take()
            && let Some(fdb) = state.fdb[master].as_mut()
        {
            fdb.forget(number);
        }
    }
    {
        let mut stack = interface.stack.lock();
        stack.configure(&[], None);
        stack.configure_ipv6(None, &[], None);
    }
    interface.go();
    info!("Net: {} was taken away", interface.name);
    super::changed();
    Ok(())
}
