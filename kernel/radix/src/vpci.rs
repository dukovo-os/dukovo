// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hyper-V's virtual PCI bus, and Azure's network adapter on it:
//! accelerated networking (ADR 0425).
//!
//! The host passes a PCI function through to the guest on a bus of its
//! own, which it offers as a VMBus channel ([`radix_hyperv::pci`]). The
//! `vmbus` thread ([`crate::hyperv`]) opens the channel and hands it to
//! [`start`], which sets the bus up and, for each function the kernel has
//! a driver for, gives its BARs addresses from the ranges the firmware
//! sets aside for VMBus (the `_CRS` of VMBus's ACPI device, or of the
//! device it is in), asks the host for its interrupt (CREATE_INTERRUPT)
//! and starts the driver. The only such function is Microsoft's Azure
//! Network Adapter (`radix-mana`).
//!
//! A MANA port has the MAC address of one of the synthetic network
//! cards, and joins it: the card's stack holds a
//! [`radix_hyperv::bond::Bond`] of the two, which takes the port through
//! the card's [`Port`] once the host says the card has a virtual
//! function, and sends through it from then on. When the host ejects the
//! function, [`serve`] asks the bond for the port back, which turns the
//! path back to the synthetic card; the thread then stops the port,
//! gives back its interrupt and addresses, and tells the host the
//! ejection is complete. A bus the host offers while the machine runs,
//! as after its maintenance, is set up the same way.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use radix_acpi::resources::{Resource, Resources};
use radix_hal::Devices;
use radix_hw::pci::{Address as PciAddress, Bar, Function, MsiX, command};
use radix_hw::{Mmio, MsiMessage};
use radix_hyperv::Guid;
use radix_hyperv::bond::Handoff;
use radix_hyperv::pci::{self as vpci, Bus, CONFIG_PAGE, CONFIG_WINDOW, Child, Event, Translated};
use radix_klog::{info, warn};
use radix_mana::Mana;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_nic::{Interrupt, Nic};
use radix_sync::SpinLock;

use crate::Arch;
use crate::hyperv::{self, Channel};
use crate::sched;

/// The most synthetic network cards a port can join.
const CARDS: usize = 8;
/// The most functions passed through at once, on all the buses.
const FUNCTIONS: usize = 8;
/// The most virtual PCI buses set up.
const BUSES: usize = 4;
/// The most ranges of addresses the firmware sets aside for VMBus.
const RANGES: usize = 4;
/// The most blocks of addresses given back and kept for the next
/// function.
const FREED: usize = 16;
/// How long a bond has to give its port back.
const GIVE_BACK_NS: u64 = 10_000_000_000;
/// How long the thread waits for uACPI to load the namespace, which
/// holds VMBus's ranges.
const ACPI_NS: u64 = 30_000_000_000;
/// Where the first BAR is in a function's configuration space.
const BAR0: usize = 0x10;
/// Addresses from 4 GiB up.
const FOUR_GIB: u64 = 1 << 32;
/// The low addresses never given: the legacy video memory and BIOS.
const LOW_START: u64 = 1 << 20;
/// The addresses kept clear past the start of the framebuffer, in case
/// the video card's memory is larger than what the screen shows.
const FRAMEBUFFER_ROOM: u64 = 64 << 20;
/// No card: what [`WAKES`] holds for a function that joined none, and
/// the index of a [`Port`] that joins nothing.
const NONE: usize = usize::MAX;

/// What a card's slot holds, for the bond and the thread to hand the
/// port over by.
mod state {
    /// No port.
    pub const EMPTY: u8 = 0;
    /// A port waits for the bond to take it.
    pub const OFFERED: u8 = 1;
    /// The bond has the port.
    pub const IN_USE: u8 = 2;
    /// The thread asked for the port back.
    pub const ASKED_BACK: u8 = 3;
    /// The bond gave the port back, still running.
    pub const RETURNED: u8 = 4;
}

/// A synthetic network card a port can join.
struct Slot {
    /// Whether a synthetic card holds the slot.
    used: AtomicBool,
    /// The card's MAC address.
    mac: SpinLock<[u8; 6]>,
    /// The card's number among the network cards, for
    /// `net::wake_card`: [`NONE`] until it has one.
    card: AtomicUsize,
    /// What the slot holds ([`state`]).
    state: AtomicU8,
    /// The port, while it is offered or given back.
    port: SpinLock<Option<Mana>>,
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            used: AtomicBool::new(false),
            mac: SpinLock::new([0; 6]),
            card: AtomicUsize::new(NONE),
            state: AtomicU8::new(state::EMPTY),
            port: SpinLock::new(None),
        }
    }
}

static SLOTS: [Slot; CARDS] = [const { Slot::new() }; CARDS];

/// The network card each passed-through function's interrupt wakes, by
/// the function's number: [`NONE`] until its port joins a card.
static WAKES: [AtomicUsize; FUNCTIONS] = [const { AtomicUsize::new(NONE) }; FUNCTIONS];
/// Which of the functions' numbers are in use.
static NUMBERS: [AtomicBool; FUNCTIONS] = [const { AtomicBool::new(false) }; FUNCTIONS];

/// A synthetic network card's slot, through which its bond takes a port
/// and gives it back.
pub struct Port(usize);

impl Port {
    /// Its slot's index, for [`attached`] and [`forget`].
    pub fn index(&self) -> usize {
        self.0
    }
}

/// Gives the synthetic network card whose MAC address is `mac` a slot,
/// for a port to join it: its [`Port`], for its bond. One that joins
/// nothing if every slot is taken.
pub fn port_for(mac: [u8; 6]) -> Port {
    for (index, slot) in SLOTS.iter().enumerate() {
        if slot.used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            *slot.mac.lock() = mac;
            slot.card.store(NONE, Ordering::Release);
            slot.state.store(state::EMPTY, Ordering::Release);
            return Port(index);
        }
    }
    warn!("Hyper-V: more than {CARDS} synthetic network cards; no more can be accelerated");
    Port(NONE)
}

/// Says that the card of slot `index` is network card `card`, which a
/// port's interrupt then wakes.
pub fn attached(index: usize, card: usize) {
    if let Some(slot) = SLOTS.get(index) {
        slot.card.store(card, Ordering::Release);
    }
}

/// Frees slot `index`, whose card could not be set up.
pub fn forget(index: usize) {
    if let Some(slot) = SLOTS.get(index) {
        slot.used.store(false, Ordering::Release);
    }
}

impl Handoff<Mana> for Port {
    fn take(&mut self) -> Option<Mana> {
        let slot = SLOTS.get(self.0)?;
        slot.state
            .compare_exchange(state::OFFERED, state::IN_USE, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let port = slot.port.lock().take();
        if port.is_none() {
            slot.state.store(state::EMPTY, Ordering::Release);
        }
        port
    }

    fn asked_back(&self) -> bool {
        SLOTS
            .get(self.0)
            .is_some_and(|slot| slot.state.load(Ordering::Acquire) == state::ASKED_BACK)
    }

    fn give_back(&mut self, vf: Mana) {
        // A port with no slot is dropped, which stops it.
        let Some(slot) = SLOTS.get(self.0) else { return };
        *slot.port.lock() = Some(vf);
        slot.state.store(state::RETURNED, Ordering::Release);
    }
}

/// A function's interrupt: wakes the network card its port joined.
fn on_interrupt(number: usize) {
    let card = WAKES.get(number).map_or(NONE, |card| card.load(Ordering::Acquire));
    if card != NONE {
        crate::net::wake_card(card);
    }
}

/// Why a function could not be used.
enum Failure {
    /// The kernel's side.
    Kernel(&'static str),
    /// The bus's protocol.
    Bus(vpci::Error),
    /// The driver.
    Card(radix_nic::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Kernel(why) => f.write_str(why),
            Failure::Bus(err) => write!(f, "{err}"),
            Failure::Card(err) => write!(f, "{err}"),
        }
    }
}

/// The addresses the firmware sets aside for VMBus's devices, which the
/// buses' configuration windows and the functions' BARs are given from.
struct Space {
    /// Each range: its next free address and its end.
    ranges: [(u64, u64); RANGES],
    count: usize,
    /// Blocks given back: their address and size.
    freed: [Option<(u64, u64)>; FREED],
}

impl Space {
    /// VMBus's ranges, from its ACPI device's `_CRS` or that of the
    /// nearest device it is in that has one with memory: the ranges
    /// above 4 GiB, and those below past the first megabyte and clear of
    /// the framebuffer.
    fn find() -> Space {
        let mut space = Space { ranges: [(0, 0); RANGES], count: 0, freed: [None; FREED] };
        let mut device = None;
        for id in [c"VMBUS", c"VMBus"] {
            radix_uacpi::find_devices(id, &mut |node| {
                if device.is_none() {
                    device = Some(node);
                }
            });
            if device.is_some() {
                break;
            }
        }
        let framebuffer = crate::console::framebuffer_start();
        let mut node = device;
        let mut depth = 0;
        while let Some(at) = node
            && space.count == 0
            && depth < 8
        {
            let _ = at.bytes(c"_CRS", |bytes| {
                for resource in Resources::new(bytes) {
                    if let Ok(Resource::Memory(memory)) = resource {
                        space.add(
                            memory.base,
                            memory.base.saturating_add(memory.length),
                            framebuffer,
                        );
                    }
                }
            });
            node = at.parent();
            depth += 1;
        }
        space
    }

    /// Adds the range from `start` to `end`, less what is never given.
    fn add(&mut self, start: u64, end: u64, framebuffer: Option<u64>) {
        let mut start = start.max(LOW_START);
        if let Some(framebuffer) = framebuffer
            && (start..end).contains(&framebuffer)
        {
            start = framebuffer.saturating_add(FRAMEBUFFER_ROOM);
        }
        if start < end && self.count < RANGES {
            self.ranges[self.count] = (start, end);
            self.count += 1;
        }
    }

    /// Whether it has any range.
    fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// `size` bytes (a power of two), aligned to their size, below 4 GiB
    /// if `below_4g`: a block given back first, then a range above 4 GiB,
    /// then one below.
    fn alloc(&mut self, size: u64, below_4g: bool) -> Option<u64> {
        let fits = |address: u64| !below_4g || address.saturating_add(size) <= FOUR_GIB;
        if let Some(block) = self
            .freed
            .iter_mut()
            .find(|block| block.is_some_and(|(address, held)| held == size && fits(address)))
        {
            return block.take().map(|(address, _)| address);
        }
        for high in [true, false] {
            for range in &mut self.ranges[..self.count] {
                if (range.0 >= FOUR_GIB) != high {
                    continue;
                }
                let Some(start) = range.0.checked_next_multiple_of(size) else { continue };
                let Some(end) = start.checked_add(size) else { continue };
                if end <= range.1 && fits(start) {
                    range.0 = end;
                    return Some(start);
                }
            }
        }
        None
    }

    /// Gives back the block at `address` of `size` bytes, for the next
    /// function of that size; lost if no room is left to keep it.
    fn free(&mut self, address: u64, size: u64) {
        if let Some(block) = self.freed.iter_mut().find(|block| block.is_none()) {
            *block = Some((address, size));
        }
    }
}

/// A function passed through and set up, as far as it got.
struct Passed {
    /// Its slot on its bus.
    slot: u32,
    /// Its number, for its interrupt ([`WAKES`]).
    number: usize,
    function: Option<Function>,
    msix: Option<(MsiX, Mmio)>,
    /// Its interrupt's vector.
    vector: Option<u32>,
    /// Its interrupt, as the host translated it.
    interrupt: Option<Translated>,
    /// Its BARs' addresses and sizes.
    bars: [(u64, u64); 6],
    /// Its port, while the thread holds it.
    mana: Option<Mana>,
    /// The slot of the card its port joined, while the bond may hold it.
    card: Option<usize>,
}

/// A virtual PCI bus, set up.
struct PciBus {
    /// Its channel's relid.
    relid: u32,
    /// Its channel's instance, which names it in the log.
    instance: Guid,
    bus: Bus,
    /// Its configuration window, which shows one function at a time.
    window: Mmio,
    window_phys: u64,
    /// The PCI segment its functions are given.
    domain: u16,
    functions: Vec<Passed, Kmalloc>,
}

impl PciBus {
    /// Shows the function in `slot` in the configuration window.
    fn select(&self, slot: u32) {
        self.window.write32(0, slot);
    }
}

/// What the `vmbus` thread keeps of the virtual PCI buses.
pub struct Buses {
    /// VMBus's addresses, found when the first bus is set up.
    space: Option<Space>,
    list: Vec<PciBus, Kmalloc>,
}

impl Buses {
    /// None yet.
    pub fn new() -> Buses {
        Buses { space: None, list: Vec::new_in(HEAP.with(Flags::MAY_SLEEP)) }
    }
}

/// Waits `us` microseconds, as `hyperv` does while a card starts.
fn delay(us: u32) {
    hyperv::delay(us);
}

/// Sets up the virtual PCI bus on `channel`, open, and the functions on
/// it the kernel has a driver for. `vp_index` names the boot CPU to the
/// host, which the functions' interrupts go to.
#[inline(never)]
pub fn start(buses: &mut Buses, vp_index: u32, channel: Channel) {
    let Channel { relid, connection, rings, instance } = channel;
    if buses.list.len() >= BUSES || buses.list.try_reserve(1).is_err() {
        warn!("Hyper-V: the virtual PCI bus {instance} is past the {BUSES} the kernel sets up");
        return;
    }
    let space = buses.space.get_or_insert_with(|| {
        if !crate::acpi::wait_started(ACPI_NS) {
            warn!("Hyper-V: no ACPI namespace, which says where PCI devices may go");
        }
        let space = Space::find();
        if space.is_empty() {
            warn!("Hyper-V: the firmware sets aside no addresses for VMBus's PCI devices");
        }
        space
    });
    let Some(window_phys) = space.alloc(CONFIG_WINDOW as u64, false) else {
        warn!("Hyper-V: the virtual PCI bus {instance}: no addresses for its configuration window");
        return;
    };
    let Some(window) = <Arch as Devices>::map_device(window_phys, CONFIG_WINDOW) else {
        warn!(
            "Hyper-V: the virtual PCI bus {instance}: its configuration window could not be mapped"
        );
        space.free(window_phys, CONFIG_WINDOW as u64);
        return;
    };
    let mut wait = delay;
    let bus = match vpci::start(rings, connection, hyperv::signal, window_phys, &mut wait) {
        Ok(bus) => bus,
        Err(err) => {
            warn!("Hyper-V: the virtual PCI bus {instance}: {err}");
            space.free(window_phys, CONFIG_WINDOW as u64);
            return;
        }
    };
    let version = bus.version();
    let mut children = [None; vpci::FUNCTIONS];
    for (kept, child) in children.iter_mut().zip(bus.children()) {
        *kept = Some(*child);
    }
    info!(
        "Hyper-V: virtual PCI bus {instance} speaks vPCI {}.{}; {} function(s)",
        version >> 16,
        version & 0xffff,
        children.iter().flatten().count()
    );
    let domain = u16::from_le_bytes([instance.0[8], instance.0[9]]);
    let mut pci = PciBus {
        relid,
        instance,
        bus,
        window,
        window_phys,
        domain,
        functions: Vec::new_in(HEAP.with(Flags::MAY_SLEEP)),
    };
    if pci.functions.try_reserve_exact(vpci::FUNCTIONS).is_err() {
        warn!("Hyper-V: the virtual PCI bus {instance}: out of memory");
        space.free(window_phys, CONFIG_WINDOW as u64);
        return;
    }
    for child in children.iter().flatten() {
        let descriptor = child.descriptor;
        if !radix_mana::identify(descriptor.vendor, descriptor.device) {
            info!(
                "Hyper-V: the virtual PCI bus {instance} passes through {:04x}:{:04x}, which the \
                 kernel has no driver for",
                descriptor.vendor, descriptor.device
            );
            continue;
        }
        if let Some(passed) = pass(&mut pci, space, child, vp_index) {
            let _ = pci.functions.push_within_capacity(passed);
        }
    }
    let _ = buses.list.push_within_capacity(pci);
}

/// Sets up `child`, Azure's network adapter, and offers its port to the
/// synthetic card with its MAC address. `None`, logged, if it could not
/// be set up.
#[inline(never)]
fn pass(pci: &mut PciBus, space: &mut Space, child: &Child, vp_index: u32) -> Option<Passed> {
    let Some(number) = NUMBERS.iter().position(|used| {
        used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }) else {
        warn!("Hyper-V: more than {FUNCTIONS} PCI functions passed through; no more are used");
        return None;
    };
    let mut passed = Passed {
        slot: child.descriptor.slot,
        number,
        function: None,
        msix: None,
        vector: None,
        interrupt: None,
        bars: [(0, 0); 6],
        mana: None,
        card: None,
    };
    if let Err(why) = set_up(pci, space, child, vp_index, &mut passed) {
        warn!("Hyper-V: Azure's network adapter on the virtual PCI bus {}: {why}", pci.instance);
        release(pci, space, &mut passed);
        NUMBERS[number].store(false, Ordering::Release);
        return None;
    }
    let Some(mana) = passed.mana.take() else { return Some(passed) };
    let mac = mana.info().mac;
    let [a, b, c, d, e, f] = mac;
    let joined = SLOTS.iter().position(|slot| {
        slot.used.load(Ordering::Acquire)
            && *slot.mac.lock() == mac
            && slot.state.load(Ordering::Acquire) == state::EMPTY
    });
    let Some(index) = joined else {
        info!(
            "Hyper-V: Azure's network adapter has port {a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:\
             {f:02x}, which no synthetic network card has; not used"
        );
        passed.mana = Some(mana);
        release(pci, space, &mut passed);
        // Kept, stopped, so that the host's ejection is answered.
        return Some(passed);
    };
    // Its interrupt wakes the card before the card can take it.
    let slot = &SLOTS[index];
    let card = slot.card.load(Ordering::Acquire);
    WAKES[number].store(card, Ordering::Release);
    passed.card = Some(index);
    *slot.port.lock() = Some(mana);
    slot.state.store(state::OFFERED, Ordering::Release);
    info!(
        "Hyper-V: Azure's network adapter at {} accelerates the synthetic network card \
         {a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}",
        PciAddress {
            segment: pci.domain,
            bus: 0,
            device: child.descriptor.device_number(),
            function: child.descriptor.function_number()
        }
    );
    if card != NONE {
        crate::net::wake_card(card);
    }
    Some(passed)
}

/// Gives `child`'s BARs addresses, finds it, maps its registers, asks the
/// host for its interrupt and starts its driver, keeping each part in
/// `passed` as it goes, for [`release`].
fn set_up(
    pci: &mut PciBus,
    space: &mut Space,
    child: &Child,
    vp_index: u32,
    passed: &mut Passed,
) -> Result<(), Failure> {
    let descriptor = child.descriptor;
    pci.select(descriptor.slot);
    let config = pci
        .window
        .window(CONFIG_PAGE, CONFIG_PAGE)
        .ok_or(Failure::Kernel("its configuration space could not be reached"))?;
    place_bars(&config, &child.probed, space, &mut passed.bars)?;
    let address = PciAddress {
        segment: pci.domain,
        bus: 0,
        device: descriptor.device_number(),
        function: descriptor.function_number(),
    };
    let function = Function::probe_given(address, config, descriptor.given())
        .ok_or(Failure::Kernel("its configuration space is too small"))?;
    function.enable_memory_and_dma();
    let Some(Bar::Memory { address: base, size, .. }) = function.bar(0) else {
        passed.function = Some(function);
        return Err(Failure::Kernel("it has no registers"));
    };
    let msix = MsiX::find(&function);
    passed.function = Some(function);
    let regs = usize::try_from(size)
        .ok()
        .and_then(|size| <Arch as Devices>::map_device(base, size))
        .ok_or(Failure::Kernel("its registers could not be mapped"))?;
    let function = passed.function.as_ref().ok_or(Failure::Kernel("it went away"))?;
    let msix = msix.ok_or(Failure::Kernel("it has no MSI-X"))?;
    let (bar, offset) = msix.table_location();
    let Some(Bar::Memory { address: table_base, .. }) = function.bar(bar) else {
        return Err(Failure::Kernel("its MSI-X table is in no BAR"));
    };
    let table = <Arch as Devices>::map_device(table_base + offset, msix.table_len())
        .ok_or(Failure::Kernel("its MSI-X table could not be mapped"))?;
    let (msix, table) = passed.msix.insert((msix, table));
    msix.enable_masked(function);
    for entry in 0..msix.entries() {
        msix.mask_entry(table, entry);
    }
    let (vector, _) =
        <Arch as Devices>::allocate_interrupt_at(address, 0, on_interrupt, passed.number)
            .ok_or(Failure::Kernel("no interrupt vector is free"))?;
    passed.vector = Some(vector);
    let vector = u8::try_from(vector).map_err(|_| Failure::Kernel("its vector is past 255"))?;
    let mut wait = delay;
    let interrupt = pci
        .bus
        .create_interrupt(descriptor.slot, vector, vp_index, &mut wait)
        .map_err(Failure::Bus)?;
    passed.interrupt = Some(interrupt);
    let message = MsiMessage { address: interrupt.address, data: interrupt.data };
    msix.set_entry(table, radix_mana::QUEUE_VECTOR, message);
    msix.unmask_all(function);
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(address, pages, false);
    let mana =
        radix_mana::start(regs, Interrupt::Msi, &mut alloc, &mut wait).map_err(Failure::Card)?;
    passed.mana = Some(mana);
    Ok(())
}

/// Gives each memory BAR of a function whose BARs read `probed` with all
/// ones written an address, in `config`, and keeps where and how large
/// in `bars`.
fn place_bars(
    config: &Mmio,
    probed: &[u32; 6],
    space: &mut Space,
    bars: &mut [(u64, u64); 6],
) -> Result<(), Failure> {
    let mut index = 0;
    while index < 6 {
        let low = probed[index];
        // None, or I/O ports, which a function passed through has not.
        if low == 0 || low & 1 == 1 {
            index += 1;
            continue;
        }
        let wide = (low >> 1) & 0x3 == 0x2 && index + 1 < 6;
        let mask = if wide {
            u64::from(probed[index + 1]) << 32 | u64::from(low & !0xf)
        } else {
            0xffff_ffff_0000_0000 | u64::from(low & !0xf)
        };
        let size = (!mask).wrapping_add(1);
        if size == 0 || !size.is_power_of_two() {
            index += if wide { 2 } else { 1 };
            continue;
        }
        let address = space
            .alloc(size, !wide)
            .ok_or(Failure::Kernel("no addresses are left for its registers"))?;
        bars[index] = (address, size);
        let offset = BAR0 + 4 * index;
        config.write32(offset, address as u32);
        if wide {
            config.write32(offset + 4, (address >> 32) as u32);
        }
        index += if wide { 2 } else { 1 };
    }
    Ok(())
}

/// Stops what `passed` holds of a function, in the order it was set up
/// backwards: its port, its interrupt, its decoding, its addresses.
fn release(pci: &mut PciBus, space: &mut Space, passed: &mut Passed) {
    let mut wait = delay;
    if let Some(mana) = passed.mana.take()
        && !mana.shut_down(&mut wait)
    {
        warn!(
            "Hyper-V: Azure's network adapter on the virtual PCI bus {} did not stop cleanly; \
             its memory is kept",
            pci.instance
        );
    }
    pci.select(passed.slot);
    if let (Some(function), Some((msix, table))) = (&passed.function, &passed.msix) {
        for entry in 0..msix.entries() {
            msix.mask_entry(table, entry);
        }
        msix.disable(function);
    }
    passed.msix = None;
    if let Some(interrupt) = passed.interrupt.take() {
        let _ = pci.bus.delete_interrupt(passed.slot, &interrupt);
    }
    if let Some(vector) = passed.vector.take() {
        <Arch as Devices>::free_interrupt(vector);
    }
    if let Some(function) = passed.function.take() {
        function.set_command(command::INTX_DISABLE);
    }
    for (address, size) in &mut passed.bars {
        if *size != 0 {
            space.free(*address, *size);
            *size = 0;
        }
    }
    WAKES[passed.number].store(NONE, Ordering::Release);
}

/// Takes `passed`'s port back from the bond that may hold it: false if
/// the bond did not give it back in time.
fn take_back(passed: &mut Passed) -> bool {
    let Some(index) = passed.card else { return true };
    let slot = &SLOTS[index];
    let swap = |from, to| {
        slot.state.compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire).is_ok()
    };
    if !swap(state::OFFERED, state::EMPTY) {
        if swap(state::IN_USE, state::ASKED_BACK) {
            let card = slot.card.load(Ordering::Acquire);
            if card != NONE {
                crate::net::wake_card(card);
            }
        }
        let deadline = sched::now_ns().saturating_add(GIVE_BACK_NS);
        while slot.state.load(Ordering::Acquire) != state::RETURNED {
            if sched::now_ns() >= deadline {
                return false;
            }
            sched::sleep_ns(1_000_000);
        }
    }
    passed.mana = slot.port.lock().take();
    slot.state.store(state::EMPTY, Ordering::Release);
    WAKES[passed.number].store(NONE, Ordering::Release);
    passed.card = None;
    true
}

/// Stops the function in `slot` of `pci`, which the host ejects, and
/// says so to the host. A function whose port its bond did not give back
/// is left as it is: the host takes it away in its own time.
fn eject(pci: &mut PciBus, space: &mut Space, slot: u32) {
    let Some(index) = pci.functions.iter().position(|passed| passed.slot == slot) else {
        let _ = pci.bus.ejection_complete(slot);
        return;
    };
    let passed = &mut pci.functions[index];
    if !take_back(passed) {
        warn!(
            "Hyper-V: the host ejects Azure's network adapter on the virtual PCI bus {}, and its \
             network card did not give it back",
            pci.instance
        );
        return;
    }
    let mut passed = pci.functions.swap_remove(index);
    release(pci, space, &mut passed);
    NUMBERS[passed.number].store(false, Ordering::Release);
    let _ = pci.bus.ejection_complete(slot);
    info!(
        "Hyper-V: the host took Azure's network adapter on the virtual PCI bus {} away; its \
         network card goes on through the synthetic card",
        pci.instance
    );
}

/// Stops the functions of the bus on channel `relid`, which the host
/// took away, and forgets it: the host has taken its functions away too.
pub fn rescinded(buses: &mut Buses, relid: u32) {
    let Buses { space, list } = buses;
    let Some(space) = space.as_mut() else { return };
    let Some(index) = list.iter().position(|pci| pci.relid == relid) else { return };
    let mut pci = list.swap_remove(index);
    while let Some(mut passed) = pci.functions.pop() {
        if take_back(&mut passed) {
            release(&mut pci, space, &mut passed);
            NUMBERS[passed.number].store(false, Ordering::Release);
        } else {
            warn!(
                "Hyper-V: the host took the virtual PCI bus {} away, and the network card of \
                 Azure's network adapter on it did not give it back",
                pci.instance
            );
        }
    }
    space.free(pci.window_phys, CONFIG_WINDOW as u64);
    info!("Hyper-V: the virtual PCI bus {} is gone", pci.instance);
}

/// Answers what the virtual PCI buses' hosts said: ejects the functions
/// they take away, and stops the ports bonds gave back on their own, as
/// when their card stopped.
pub fn serve(buses: &mut Buses) {
    let Buses { space, list } = buses;
    let Some(space) = space.as_mut() else { return };
    for pci in list.iter_mut() {
        loop {
            while let Some(event) = pci.bus.next_event() {
                match event {
                    Event::Eject { slot } => eject(pci, space, slot),
                    Event::Relations(relations) => {
                        let added = relations
                            .iter()
                            .filter(|listed| {
                                !pci.functions.iter().any(|passed| passed.slot == listed.slot)
                            })
                            .count();
                        if added > 0 {
                            info!(
                                "Hyper-V: the virtual PCI bus {} lists {added} new function(s), \
                                 which are used once the host offers the bus again",
                                pci.instance
                            );
                        }
                    }
                }
            }
            if !pci.bus.arm() {
                break;
            }
        }
        // Ports their bonds gave back unasked: stopped, and their
        // functions kept, so that the host's ejection is answered.
        for index in 0..pci.functions.len() {
            let returned = pci.functions[index]
                .card
                .is_some_and(|card| SLOTS[card].state.load(Ordering::Acquire) == state::RETURNED);
            if !returned {
                continue;
            }
            // Out of the list while it is stopped, then back in its place.
            let mut passed = pci.functions.swap_remove(index);
            if take_back(&mut passed) {
                release(pci, space, &mut passed);
            }
            let _ = pci.functions.push_within_capacity(passed);
            let last = pci.functions.len() - 1;
            pci.functions.swap(index, last);
        }
    }
}
