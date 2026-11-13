// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! virtio console ports (`radix_virtio::console`, ADR 0327): the channels
//! a program on the host reaches the machine through, as QEMU's guest
//! agent and cloud consoles do. Each port is a terminal in `/devices`,
//! `vport<d>p<n>` as Linux names it (console `d`, port `n`), that carries
//! bytes as they are (`fs::tty::make_raw_line`): Linux's are no terminals
//! at all, and the programs that use them speak protocols. A named port
//! is linked from `/devices/virtio-ports/<name>`, as udev links Linux's,
//! so that a program finds its channel by name:
//! `/devices/virtio-ports/org.qemu.guest_agent.0`.
//!
//! [`bind`] sets the device up with one MSI-X interrupt for all its queues
//! and every port it may have (up to `MAX_PORTS`), and starts its thread,
//! `vport<d>`, which the interrupt and the ports' terminals wake. The
//! thread reads the device's control messages: a port the host adds gets
//! its terminal and is said ready, one it takes away hangs up, and a name
//! links its port. Then it carries each port, as a serial line's thread
//! does: what came from the host is typed on the terminal, and dropped
//! while no program has it open, as Linux drops it; what programs write
//! goes to the host a buffer at a time. A program opening the terminal,
//! or the last one closing it, is told to the host, as Linux's driver
//! does, so that the host knows whether anyone listens.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

use radix_hal::Devices;
use radix_hw::pci::{Bar, Function, MsiX};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Once, SpinLock};
use radix_tty::line::{B115200, cflag};
use radix_virtio::Transport;
use radix_virtio::console::{BUFFER, Console, Event, MAX_PORTS, NAME_MAX, Port};

use crate::Arch;
use crate::fs::devfs;
use crate::fs::tty::{self, TtyRef};
use crate::sched::{self, Options, Policy, ThreadRef};

/// The most consoles.
const MAX_CONSOLES: usize = 4;
/// Their threads' names.
const NAMES: [&str; MAX_CONSOLES] = ["vport0", "vport1", "vport2", "vport3"];
/// The control flags a port's terminal starts with: CS8, CREAD and
/// CLOCAL, at a speed it has none of, 115200 baud as a serial console's.
const CFLAG: u32 = B115200 | cflag::CSIZE | 0o200 | cflag::CLOCAL;
/// How many control messages the thread takes at once.
const EVENTS: usize = 8;

/// A port as its console's thread carries it.
struct Line {
    port: Port,
    /// Its terminal, while the host has it.
    tty: Option<TtyRef>,
    /// Its name, if the host gave one: its link's in `virtio-ports`.
    name: Option<TextBuf<NAME_MAX>>,
    /// Whether a program has its terminal open, as last told to the host.
    open: bool,
    /// Bytes from the host that the terminal has not taken yet.
    held: [u8; BUFFER],
    held_at: usize,
    held_len: usize,
    /// Bytes from the terminal that the device has not taken yet.
    sending: [u8; BUFFER],
    sending_at: usize,
    sending_len: usize,
}

/// A console and its ports, which its thread owns.
struct Carrier {
    console: Console,
    /// Its ports, by number, each on the heap.
    lines: Vec<Box<Line, Kmalloc>, Kmalloc>,
    /// Which console it is.
    index: usize,
}

/// How many consoles were found.
static FOUND: AtomicUsize = AtomicUsize::new(0);
/// Each console's thread, which its interrupt and its terminals wake.
static THREADS: [Once<ThreadRef>; MAX_CONSOLES] = [const { Once::new() }; MAX_CONSOLES];
/// Each console's carrier, until its thread takes it.
static CARRIERS: [SpinLock<Option<Box<Carrier, Kmalloc>>>; MAX_CONSOLES] =
    [const { SpinLock::new(None) }; MAX_CONSOLES];

/// Wakes console `index`'s thread: its device or a terminal changed.
fn wake(index: usize) {
    if let Some(thread) = THREADS.get(index).and_then(Once::get) {
        sched::wake(thread);
    }
}

/// Sets up the virtio console `function`, its ports and its thread.
pub fn bind(function: &Function) {
    if let Err(why) = try_bind(function) {
        warn!("Ports: could not set up the virtio console at {}: {why}", function.address());
    }
}

/// [`bind`], saying why it failed. Kept out of the PCI scan's frame: the
/// console's queues are built on the stack before they move.
#[inline(never)]
fn try_bind(function: &Function) -> Result<(), &'static str> {
    let index = FOUND.fetch_add(1, Ordering::AcqRel);
    if index >= MAX_CONSOLES {
        return Err("too many consoles");
    }
    function.enable_memory_and_dma();
    let mut map_bar = |_: usize, bar: Bar| match bar {
        Bar::Memory { address, size, .. } => {
            <Arch as Devices>::map_device(address, usize::try_from(size).ok()?)
        }
        Bar::Io { .. } => None,
    };
    let transport = Transport::find(function, &mut map_bar).map_err(|_| "no modern transport")?;
    // One MSI-X entry for every queue: they all wake the thread.
    let msix = MsiX::find(function).ok_or("it has no MSI-X")?;
    let (bar, offset) = msix.table_location();
    let Some(Bar::Memory { address, .. }) = function.bar(bar) else {
        return Err("its MSI-X table is not in memory");
    };
    let table = <Arch as Devices>::map_device(address + offset, msix.table_len())
        .ok_or("its MSI-X table could not be mapped")?;
    let (vector, message) = <Arch as Devices>::allocate_interrupt(function.address(), wake, index)
        .ok_or("no interrupt vector is free")?;
    msix.enable_masked(function);
    msix.set_entry(&table, 0, message);
    msix.unmask_all(function);
    let carrier = new_carrier(function, transport, index).inspect_err(|_| {
        <Arch as Devices>::free_interrupt(vector);
    })?;
    let ports = carrier.lines.len();
    *CARRIERS[index].lock() = Some(carrier);
    // Held until it is kept for the interrupt to wake: run at once, it
    // could say a port is ready and sleep before then, and the host's
    // answer, the port's name, would wake no one.
    let options = Options::new().held().policy(Policy::Fair { nice: -5 });
    match sched::spawn(NAMES[index], run, index, options) {
        Ok(thread) => {
            let _ = THREADS[index].set(thread.clone());
            sched::release(&thread);
        }
        Err(err) => {
            warn!("Ports: could not start the thread of the virtio console: {err}");
            // Dropping the console resets the device.
            CARRIERS[index].lock().take();
            <Arch as Devices>::free_interrupt(vector);
            return Err("no thread");
        }
    }
    info!(
        "Ports: the virtio console at {} has room for {ports} ports, MSI-X vector {vector:#x}",
        function.address()
    );
    Ok(())
}

/// The console behind `transport`, set up with all its ports' queues and
/// started, on the heap.
#[inline(never)]
fn new_carrier(
    function: &Function,
    transport: Transport,
    index: usize,
) -> Result<Box<Carrier, Kmalloc>, &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(function.address(), pages, false);
    let console = Console::new(transport, 0, &mut alloc).map_err(|err| match err {
        radix_virtio::Error::MissingFeature(_) => "it has only one port, which is not used",
        _ => "the device refused to be set up",
    })?;
    let count = console.info().ports.min(MAX_PORTS) as usize;
    let mut lines = Vec::new_in(heap);
    lines.try_reserve_exact(count).map_err(|_| "out of memory")?;
    for number in 0..count {
        let port = console.port(number as u32, 0, &mut alloc).map_err(|_| "a port's queues")?;
        let line = Box::try_new_in(
            Line {
                port,
                tty: None,
                name: None,
                open: false,
                held: [0; BUFFER],
                held_at: 0,
                held_len: 0,
                sending: [0; BUFFER],
                sending_at: 0,
                sending_len: 0,
            },
            heap,
        )
        .map_err(|_| "out of memory")?;
        let _ = lines.push_within_capacity(line);
    }
    let mut carrier =
        Box::try_new_in(Carrier { console, lines, index }, heap).map_err(|_| "out of memory")?;
    carrier.console.start().map_err(|_| "the device took no control message")?;
    Ok(carrier)
}

/// A console's thread: reads its control messages and carries its ports,
/// then sleeps until the device or a terminal wakes it.
fn run(index: usize) {
    let Some(mut carrier) = CARRIERS[index].lock().take() else { return };
    loop {
        carrier.control();
        for number in 0..carrier.lines.len() {
            carrier.carry(number);
        }
        sched::block(NAMES[index]);
    }
}

/// Whether `name` makes a good link's name in `virtio-ports`: printable,
/// no `/`, not `.` or `..`.
fn good_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name != b"."
        && name != b".."
        && name.iter().all(|&b| (0x21..0x7f).contains(&b) && b != b'/')
}

impl Carrier {
    /// The terminal's name of port `number`: `vport<d>p<n>`.
    fn tty_name(&self, number: usize) -> TextBuf<16> {
        let mut name = TextBuf::new();
        let _ = write!(name, "vport{}p{number}", self.index);
        name
    }

    /// Reads the device's control messages and does what they say.
    fn control(&mut self) {
        loop {
            let mut events: [Option<Event>; EVENTS] = [None; EVENTS];
            let mut count = 0;
            // Taken a few at a time: the console answers some of them.
            self.console.events(&mut |event| {
                if count < EVENTS {
                    events[count] = Some(event);
                    count += 1;
                }
            });
            if count == 0 {
                return;
            }
            for event in events.iter().flatten() {
                self.event(*event);
            }
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Added(number) => self.added(number as usize),
            Event::Removed(number) => self.removed(number as usize),
            Event::Named(number, name, len) => self.named(number as usize, &name[..len]),
            Event::Console(number) => {
                info!("Ports: {} is the host's console", self.tty_name(number as usize));
            }
            // The host's end is not watched: what goes while no program
            // there listens is the host's to drop.
            Event::HostOpen(..) => {}
        }
    }

    /// Port `number` was added: gives it a terminal and its buffers, and
    /// tells the device it is ready, or that it is not taken.
    fn added(&mut self, number: usize) {
        let name = self.tty_name(number);
        let index = self.index;
        let Some(line) = self.lines.get_mut(number) else {
            let _ = self.console.port_ready(number as u32, false);
            warn!("Ports: the virtio console's port {number} is past the {MAX_PORTS} kept");
            return;
        };
        if line.tty.is_none() {
            match tty::make_raw_line(name.as_str(), CFLAG, wake, index) {
                Ok(tty) => line.tty = Some(tty),
                Err(err) => {
                    let _ = self.console.port_ready(number as u32, false);
                    warn!("Ports: /devices/{name} could not be made: {err:?}");
                    return;
                }
            }
            line.port.fill();
        }
        let _ = self.console.port_ready(number as u32, true);
    }

    /// Port `number` was taken away: its terminal hangs up, and its link
    /// goes.
    fn removed(&mut self, number: usize) {
        let name = self.tty_name(number);
        let Some(line) = self.lines.get_mut(number) else { return };
        info!("Ports: {name} was taken away");
        if let Some(tty) = line.tty.take() {
            tty.line_gone();
        }
        if let Some(name) = line.name.take() {
            devfs::remove_port_link(name.as_str());
        }
        line.open = false;
        (line.held_at, line.held_len, line.sending_at, line.sending_len) = (0, 0, 0, 0);
    }

    /// Port `number` is called `name`: links it from `virtio-ports`.
    fn named(&mut self, number: usize, name: &[u8]) {
        let target = self.tty_name(number);
        let Some(line) = self.lines.get_mut(number) else { return };
        let Ok(text) = core::str::from_utf8(name) else { return };
        if !good_name(name) {
            warn!("Ports: {target}'s name is not one a link can have");
            return;
        }
        if let Some(old) = line.name.take() {
            devfs::remove_port_link(old.as_str());
        }
        let mut kept = TextBuf::new();
        let _ = kept.write_str(text);
        let mut path = TextBuf::<24>::new();
        let _ = write!(path, "../{target}");
        match devfs::add_port_link(text, path.as_str()) {
            Ok(()) => {
                info!("Ports: {target} is /devices/virtio-ports/{text}");
                line.name = Some(kept);
            }
            Err(err) => warn!("Ports: /devices/virtio-ports/{text} could not be made: {err:?}"),
        }
    }

    /// Carries port `number`: tells the host when its terminal opens or
    /// closes, types what came, and sends what programs wrote.
    fn carry(&mut self, number: usize) {
        let Self { console, lines, .. } = self;
        let Some(line) = lines.get_mut(number) else { return };
        let Some(tty) = line.tty.clone() else { return };
        let open = tty.line_open();
        if open != line.open {
            line.open = open;
            let _ = console.port_open(number as u32, open);
            if !open {
                // What nobody read goes, as on Linux.
                (line.held_at, line.held_len) = (0, 0);
            }
        }
        // From the host: typed, as much as the terminal takes; the rest
        // waits until a program reads, which wakes the thread.
        loop {
            if line.held_at == line.held_len {
                line.held_at = 0;
                line.held_len = line.port.read(&mut line.held);
                if line.held_len == 0 {
                    break;
                }
            }
            let taken = tty.line_give(&line.held[line.held_at..line.held_len]);
            line.held_at += taken;
            if line.held_at < line.held_len {
                break;
            }
        }
        // To the host: a buffer at a time, while the device has room; it
        // interrupts as it takes them, which wakes the thread for more.
        loop {
            if line.sending_at == line.sending_len {
                line.sending_at = 0;
                line.sending_len = tty.line_take(&mut line.sending);
                if line.sending_len == 0 {
                    break;
                }
            }
            let sent = line.port.write(&line.sending[line.sending_at..line.sending_len]);
            line.sending_at += sent;
            if sent == 0 {
                break;
            }
        }
    }
}
