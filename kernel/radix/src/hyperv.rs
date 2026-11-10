// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hyper-V's VMBus (ADR 0423): the devices a virtual machine under
//! Microsoft's hypervisor, or in Azure, has in place of PCI devices.
//!
//! [`start`] looks for Hyper-V and starts its synthetic interrupt
//! controller on the boot CPU, then the `vmbus` thread, pinned to that
//! CPU, whose message slot and end-of-message register are its own. The
//! thread connects to the host (CONNECT, the newest version it takes),
//! asks for its channels (REQUEST_OFFERS) and sets each up in turn:
//!
//! - the synthetic SCSI controller's disks, through the block queue
//!   (`fs::disk::storvsc`);
//! - the synthetic network card, through the network stack
//!   (`net::attach_other`);
//! - the synthetic keyboard, whose keys go to `/devices/kbd` as a PS/2
//!   keyboard's do;
//! - the shutdown service, which presses the power button for the host;
//!   the time service, which sets the clock; and the heartbeat, which
//!   tells the host the guest is alive.
//!
//! Other channels (the mouse, the video card, the backup and key-value
//! services, dynamic memory) are logged and left. Opening a channel names
//! its ring buffers to the host as a GPADL (GPADL_HEADER and GPADL_BODY,
//! answered by GPADL_CREATED) and opens it on them (OPEN_CHANNEL,
//! answered by OPENCHANNEL_RESULT); the thread waits for each answer in
//! the message slot, which only it reads.
//!
//! The interrupt handler takes the event flags: a disk controller's
//! completions are collected there, a network card's thread is woken, and
//! a service's packets are left for the `vmbus` thread, which it wakes,
//! as it does for a message. The boot waits for the disks the host
//! offered ([`wait_for_disks`]) before it looks for the root file system.

use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use radix_hal::{Hyperv, Synic, Timer};
use radix_hw::Dma;
use radix_hyperv::ic::{Action, Ic, Service};
use radix_hyperv::ring::{self, INBAND, Rings};
use radix_hyperv::vmbus::{self, FromHost, Offer, Request};
use radix_hyperv::{Guid, PAGE, guid, keyboard, network};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Once, SpinLock};

use crate::Arch;
use crate::sched::{self, Options, Policy, ThreadRef};

/// The pages of the thread's stack: 64 KiB, as it builds a network
/// card's stack in `net::attach_other`'s frame (ADR 0058).
pub const STACK_PAGES: usize = 16;
/// The channels whose events are routed: their relids are below this.
const CHANNELS: usize = 256;
/// The most channels the host's offers at boot are kept for.
const OFFERS: usize = 64;
/// The most services (the keyboard and the integration services) set up.
const SERVICES: usize = 8;
/// How long the host has to answer a message.
const ANSWER_NS: u64 = 5_000_000_000;
/// How long the boot waits for the disks the host offered.
const DISKS_NS: u64 = 60_000_000_000;
/// How often the thread looks at its slot and its services without being
/// woken, in case an interrupt was missed.
const LOOK_NS: u64 = 1_000_000_000;
/// The status of a message the host's queue had no room for, which goes
/// again.
const INSUFFICIENT_BUFFERS: u16 = 0x13;
/// How far off the clock may be before a sample of the host's sets it:
/// closer, the network time service keeps it.
const SAMPLE_STEP_NS: u64 = 5_000_000_000;

/// The pages of each channel's two rings, outbound then inbound.
const STORAGE_RINGS: (usize, usize) = (16, 16);
const NETWORK_RINGS: (usize, usize) = (32, 32);
const SERVICE_RINGS: (usize, usize) = (4, 4);

/// Where a channel's events go: the kind in the high byte, its number
/// among those of its kind below.
const ROUTE_DISKS: u32 = 1 << 24;
const ROUTE_NETWORK: u32 = 2 << 24;
const ROUTE_SERVICE: u32 = 3 << 24;

static SYNIC: Once<Synic> = Once::new();
static THREAD: Once<ThreadRef> = Once::new();
/// HvPostMessage's input block, which one message at a time is written
/// in.
static INPUT: Once<SpinLock<Dma>> = Once::new();
/// The interrupt page and the two monitor pages CONNECT gives the host.
static CONNECTION_PAGES: Once<Dma> = Once::new();
/// Whether the machine runs under Hyper-V and the thread is setting up
/// its channels.
static STARTED: AtomicBool = AtomicBool::new(false);
/// Whether the channels offered at boot are set up, or given up on.
static SETTLED: AtomicBool = AtomicBool::new(false);
/// Where each channel's events go, by relid ([`ROUTE_DISKS`] and the
/// others); 0 for nowhere.
static ROUTES: [AtomicU32; CHANNELS] = [const { AtomicU32::new(0) }; CHANNELS];
/// The services with packets waiting, a bit each.
static PENDING: AtomicU64 = AtomicU64::new(0);
/// The next GPADL's number: any the guest chooses, FreeBSD's first.
static NEXT_GPADL: AtomicU32 = AtomicU32::new(0xe1e10);

/// Starts VMBus, if the machine runs under Hyper-V: its synthetic
/// interrupt controller, then the thread that sets up its channels.
pub fn start() {
    let Some(synic) = <Arch as Hyperv>::hyperv_start(vmbus::SINT, on_interrupt, 0) else { return };
    let (major, minor, build) = synic.version;
    info!(
        "Hyper-V: version {major}.{minor} build {build}; VMBus on vector {:#x}, virtual \
         processor {}",
        synic.vector, synic.vp_index
    );
    let _ = SYNIC.set(synic);
    let Some(input) = <Arch as Hyperv>::hyperv_alloc(1) else {
        warn!("Hyper-V: out of memory; VMBus is not used");
        return;
    };
    let _ = INPUT.set(SpinLock::new(input));
    // Pinned to the boot CPU, whose message slot and end-of-message
    // register the thread uses.
    let options =
        Options::new().pinned(0).policy(Policy::Fair { nice: -5 }).stack_pages(STACK_PAGES);
    match sched::spawn("vmbus", run, 0, options) {
        Ok(thread) => {
            let _ = THREAD.set(thread);
            STARTED.store(true, Ordering::Release);
        }
        Err(err) => warn!("Hyper-V: could not start the VMBus thread: {err}"),
    }
}

/// Waits until the channels the host offered at boot are set up, the
/// disks among them, for a minute at most: before the boot looks for its
/// root file system.
pub fn wait_for_disks() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let deadline = sched::now_ns().saturating_add(DISKS_NS);
    while !SETTLED.load(Ordering::Acquire) {
        if sched::now_ns() >= deadline {
            info!("Hyper-V: VMBus's devices are not all set up yet; going on");
            return;
        }
        sched::sleep_ns(10_000_000);
    }
}

/// VMBus's interrupt, on the boot CPU: the channels with events, and the
/// message slot.
fn on_interrupt(_: usize) {
    let Some(synic) = SYNIC.get() else { return };
    let mut wake = false;
    vmbus::take_events(&synic.events, vmbus::SINT, CHANNELS as u32, &mut |relid| {
        let Some(route) = ROUTES.get(relid as usize) else { return };
        let route = route.load(Ordering::Acquire);
        let number = (route & 0xff_ffff) as usize;
        match route & 0xff00_0000 {
            ROUTE_DISKS => crate::fs::disk::storvsc::on_event(number),
            ROUTE_NETWORK => crate::net::wake_card(number),
            ROUTE_SERVICE => {
                PENDING.fetch_or(1 << number, Ordering::AcqRel);
                wake = true;
            }
            _ => {}
        }
    });
    if (wake || vmbus::peek(&synic.messages, vmbus::SINT).is_some())
        && let Some(thread) = THREAD.get()
    {
        sched::wake(thread);
    }
}

/// Signals the host's end of a channel through `connection`; for the
/// drivers, as `radix_hyperv::Signal`.
pub fn signal(connection: u32) -> u16 {
    <Arch as Hyperv>::hyperv_signal_event(connection)
}

/// An open channel: its number, the connection that signals the host,
/// and its rings.
pub struct Channel {
    /// Its relid.
    pub relid: u32,
    /// The connection the guest signals it through.
    pub connection: u32,
    /// Its rings, open.
    pub rings: Rings,
    /// The device's instance, which names it in the log.
    pub instance: Guid,
}

/// Routes `channel`'s events to the disk controller numbered `number`.
pub fn route_disks(channel: &Channel, number: usize) {
    route(channel.relid, ROUTE_DISKS | number as u32);
}

fn route(relid: u32, route: u32) {
    if let Some(slot) = ROUTES.get(relid as usize) {
        slot.store(route, Ordering::Release);
    }
}

/// Posts `request` to the host; false if the hypervisor refused it.
#[inline(never)]
fn post(request: &Request) -> bool {
    let Some(input) = INPUT.get() else { return false };
    for _ in 0..100 {
        let status = {
            let block = input.lock_irq();
            request.write_input(&block, 0);
            <Arch as Hyperv>::hyperv_post_message(block.phys())
        };
        match status {
            0 => return true,
            // The host's queue is full: it takes it a little later.
            INSUFFICIENT_BUFFERS => sched::sleep_ns(1_000_000),
            status => {
                warn!("Hyper-V: the hypervisor refused a VMBus message: status {status:#x}");
                return false;
            }
        }
    }
    warn!("Hyper-V: the host took no VMBus message for 100 ms");
    false
}

/// What the thread keeps: the services it set up, and a buffer their
/// packets are read into.
struct State {
    services: Vec<ServiceChannel, Kmalloc>,
    buffer: Vec<u8, Kmalloc>,
}

/// A keyboard or integration service's channel.
struct ServiceChannel {
    channel: Channel,
    kind: Kind,
}

enum Kind {
    Keyboard,
    Ic(Ic),
}

/// The bytes the thread reads a service's packet into: a shutdown request
/// carries a 2 KiB message after its fields.
const SERVICE_BUFFER: usize = 4096;

/// The thread: connects, sets up the channels offered, then serves its
/// services and the host's messages.
fn run(_: usize) {
    let Some(synic) = SYNIC.get() else { return };
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut services = Vec::new_in(heap);
    let mut buffer = Vec::new_in(heap);
    if services.try_reserve_exact(SERVICES).is_err()
        || buffer.try_reserve_exact(SERVICE_BUFFER).is_err()
    {
        warn!("Hyper-V: out of memory; VMBus is not used");
        SETTLED.store(true, Ordering::Release);
        return;
    }
    for _ in 0..SERVICE_BUFFER {
        let _ = buffer.push_within_capacity(0);
    }
    let mut state = State { services, buffer };
    match connect(synic) {
        Some(version) => {
            info!("Hyper-V: VMBus {}.{} connected", version >> 16, version & 0xffff);
            set_up_offers(synic, &mut state);
        }
        None => {
            warn!("Hyper-V: the host speaks no VMBus version the kernel does; VMBus is not used");
        }
    }
    SETTLED.store(true, Ordering::Release);
    loop {
        while let Some(message) = next_message(synic) {
            note(&message);
        }
        serve(&mut state);
        sched::block_until(sched::now_ns().saturating_add(LOOK_NS), "vmbus");
    }
}

/// Takes the next message from VMBus's slot, if one is there: the slot
/// is emptied, and the hypervisor told to deliver the next it holds.
#[inline(never)]
fn next_message(synic: &Synic) -> Option<FromHost> {
    let slot = vmbus::peek(&synic.messages, vmbus::SINT)?;
    let message =
        if slot.kind == vmbus::MESSAGE_CHANNEL { vmbus::parse(slot.bytes()) } else { None };
    if vmbus::empty(&synic.messages, vmbus::SINT) {
        <Arch as Hyperv>::hyperv_end_of_message();
    }
    Some(message.unwrap_or(FromHost::Other(slot.kind)))
}

/// Waits for the message `want` takes, for [`ANSWER_NS`] at most; others
/// that come meanwhile are noted.
#[inline(never)]
fn wait_message(synic: &Synic, want: &mut dyn FnMut(&FromHost) -> bool) -> Option<FromHost> {
    let deadline = sched::now_ns().saturating_add(ANSWER_NS);
    loop {
        while let Some(message) = next_message(synic) {
            if want(&message) {
                return Some(message);
            }
            note(&message);
        }
        let now = sched::now_ns();
        if now >= deadline {
            return None;
        }
        sched::block_until(deadline.min(now + 10_000_000), "vmbus");
    }
}

/// A message no one waits for. An offer once the boot's are set up is of
/// a device added while the machine runs.
fn note(message: &FromHost) {
    match message {
        FromHost::Offer(offer) if SETTLED.load(Ordering::Acquire) => info!(
            "Hyper-V: the host added {} ({}); devices added while the machine runs are not \
             used yet",
            describe(&offer.kind),
            offer.instance
        ),
        FromHost::Rescinded { channel } => {
            info!("Hyper-V: the host took channel {channel} away");
            route(*channel, 0);
            post(&vmbus::release(*channel));
        }
        _ => {}
    }
}

/// What the log calls a kind of device.
fn describe(kind: &Guid) -> &'static str {
    guid::name(kind).unwrap_or("a device of a kind the kernel does not know")
}

/// Connects at the newest version the host takes; returns it.
fn connect(synic: &Synic) -> Option<u32> {
    let pages = match CONNECTION_PAGES.get() {
        Some(pages) => pages,
        None => {
            let pages = <Arch as Hyperv>::hyperv_alloc(3)?;
            let _ = CONNECTION_PAGES.set(pages);
            CONNECTION_PAGES.get()?
        }
    };
    let phys = pages.phys();
    for version in vmbus::VERSIONS {
        let monitor = [phys + PAGE as u64, phys + 2 * PAGE as u64];
        if !post(&vmbus::connect(version, phys, monitor, synic.vp_index)) {
            return None;
        }
        let answer = wait_message(synic, &mut |m| matches!(m, FromHost::Connected { .. }));
        match answer {
            Some(FromHost::Connected { accepted: true }) => return Some(version),
            Some(_) => {}
            None => {
                warn!("Hyper-V: the host did not answer CONNECT");
                return None;
            }
        }
    }
    None
}

/// Asks for the host's channels and sets each up.
fn set_up_offers(synic: &Synic, state: &mut State) {
    let mut offers = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    if offers.try_reserve_exact(OFFERS).is_err() || !post(&vmbus::request_offers()) {
        warn!("Hyper-V: could not ask for VMBus's channels");
        return;
    }
    let mut done = false;
    while !done {
        let answer =
            wait_message(synic, &mut |m| matches!(m, FromHost::Offer(_) | FromHost::OffersDone));
        match answer {
            Some(FromHost::Offer(offer)) => {
                if offers.push_within_capacity(offer).is_err() {
                    warn!("Hyper-V: more than {OFFERS} channels; {} is not used", offer.instance);
                }
            }
            Some(_) => done = true,
            None => {
                warn!("Hyper-V: the host did not finish offering its channels");
                done = true;
            }
        }
    }
    let (mut controllers, mut disks) = (0, 0);
    for offer in &offers {
        if offer.channel as usize >= CHANNELS {
            warn!("Hyper-V: channel {} is past those the kernel routes; not used", offer.channel);
            continue;
        }
        match offer.kind {
            guid::STORAGE => {
                if let Some(channel) = open(synic, offer, STORAGE_RINGS) {
                    route_disks(&channel, controllers);
                    match crate::fs::disk::storvsc::bind(channel, controllers, &mut disks) {
                        Ok(()) => controllers += 1,
                        Err(why) => warn!(
                            "Hyper-V: the synthetic SCSI controller {}: {why}",
                            offer.instance
                        ),
                    }
                }
            }
            guid::NETWORK => set_up_network(synic, offer),
            guid::KEYBOARD => set_up_service(synic, state, offer, None),
            guid::SHUTDOWN => set_up_service(synic, state, offer, Some(Service::Shutdown)),
            guid::TIMESYNC => set_up_service(synic, state, offer, Some(Service::TimeSync)),
            guid::HEARTBEAT => set_up_service(synic, state, offer, Some(Service::Heartbeat)),
            kind => info!("Hyper-V: {} ({}) is not used", describe(&kind), offer.instance),
        }
    }
    info!(
        "Hyper-V: {} channel(s) offered; {controllers} SCSI controller(s), {disks} disk(s)",
        offers.len()
    );
}

/// Opens `offer`'s channel on rings of `pages` (outbound, inbound).
#[inline(never)]
fn open(synic: &Synic, offer: &Offer, (out, inbound): (usize, usize)) -> Option<Channel> {
    let what = describe(&offer.kind);
    // Never given back: the host may write into the rings for as long as
    // the channel is open, and the kernel does not close it.
    let Some(memory) = <Arch as Hyperv>::hyperv_alloc(out + inbound).map(Dma::kept) else {
        warn!("Hyper-V: {what} ({}): out of memory", offer.instance);
        return None;
    };
    let first = memory.phys() / PAGE as u64;
    let Some(gpadl) = make_gpadl(synic, offer.channel, out + inbound, &|i| first + i as u64) else {
        warn!("Hyper-V: {what} ({}): the host did not take its rings", offer.instance);
        return None;
    };
    let opened = post(&vmbus::open(offer.channel, gpadl, out as u32, synic.vp_index, &[]))
        && matches!(
            wait_message(synic, &mut |m| {
                matches!(m, FromHost::Opened { channel, .. } if *channel == offer.channel)
            }),
            Some(FromHost::Opened { status: 0, .. })
        );
    if !opened {
        warn!("Hyper-V: {what} ({}): the host did not open its channel", offer.instance);
        tear_down(synic, offer.channel, gpadl);
        return None;
    }
    Some(Channel {
        relid: offer.channel,
        connection: offer.connection,
        rings: Rings::new(memory, out),
        instance: offer.instance,
    })
}

/// Names `count` pages to the host for channel `relid`, the frame number
/// of the `i`th being `page(i)`: returns the GPADL's number.
#[inline(never)]
fn make_gpadl(synic: &Synic, relid: u32, count: usize, page: &dyn Fn(usize) -> u64) -> Option<u32> {
    let number = NEXT_GPADL.fetch_add(1, Ordering::Relaxed);
    if !vmbus::gpadl(relid, number, count, page, &mut |request| post(request)) {
        return None;
    }
    let answer = wait_message(
        synic,
        &mut |m| matches!(m, FromHost::GpadlCreated { gpadl, .. } if *gpadl == number),
    );
    matches!(answer, Some(FromHost::GpadlCreated { status: 0, .. })).then_some(number)
}

/// Takes GPADL `gpadl` of channel `relid` back; whether the host let go
/// of it.
#[inline(never)]
fn tear_down(synic: &Synic, relid: u32, gpadl: u32) -> bool {
    post(&vmbus::gpadl_teardown(relid, gpadl))
        && wait_message(
            synic,
            &mut |m| matches!(m, FromHost::GpadlTornDown { gpadl: torn } if *torn == gpadl),
        )
        .is_some()
}

/// Sets up the synthetic network card `offer` names.
fn set_up_network(synic: &Synic, offer: &Offer) {
    let Some(channel) = open(synic, offer, NETWORK_RINGS) else { return };
    let Channel { relid, connection, rings, instance } = channel;
    // The receive buffer and the frames' slots are the host's to reach
    // for as long as the channel is open: never given back.
    let mut alloc = |pages: usize| <Arch as Hyperv>::hyperv_alloc(pages).map(Dma::kept);
    let mut gpadl = |count: usize, page: network::Pages<'_>| make_gpadl(synic, relid, count, page);
    let mut delay = delay;
    let kernel = network::Kernel { alloc: &mut alloc, gpadl: &mut gpadl, delay: &mut delay };
    match network::start(rings, connection, signal, kernel) {
        Ok(card) => {
            let version = card.version();
            let mut bus = TextBuf::<48>::new();
            let _ = write!(bus, "VMBus {instance}");
            if let Some(number) = crate::net::attach_other(card, "netvsc", bus.as_str()) {
                route(relid, ROUTE_NETWORK | number as u32);
                info!(
                    "Hyper-V: the synthetic network card {instance} speaks NVSP {}.{}",
                    version >> 16,
                    version & 0xffff
                );
            }
        }
        Err(err) => warn!("Hyper-V: the synthetic network card {instance}: {err}"),
    }
}

/// Sets up the keyboard (`service` `None`) or the integration service
/// `offer` names.
fn set_up_service(synic: &Synic, state: &mut State, offer: &Offer, service: Option<Service>) {
    if state.services.len() >= SERVICES {
        return;
    }
    let Some(channel) = open(synic, offer, SERVICE_RINGS) else { return };
    let number = state.services.len();
    route(channel.relid, ROUTE_SERVICE | number as u32);
    let kind = match service {
        Some(service) => Kind::Ic(Ic::new(service)),
        None => {
            match ring::send(&channel.rings, &keyboard::protocol_request(), 1, true) {
                Ok(true) => {
                    signal(channel.connection);
                }
                Ok(false) => {}
                Err(_) => warn!("Hyper-V: the synthetic keyboard's ring is full"),
            }
            Kind::Keyboard
        }
    };
    let _ = state.services.push_within_capacity(ServiceChannel { channel, kind });
    PENDING.fetch_or(1 << number, Ordering::AcqRel);
}

/// Reads the packets waiting on every service's channel, and answers
/// them.
fn serve(state: &mut State) {
    let State { services, buffer, .. } = state;
    PENDING.store(0, Ordering::Release);
    for service in services.iter_mut() {
        let rings = &service.channel.rings;
        let connection = service.channel.connection;
        while let Some((header, len, wake)) = rings.read(buffer) {
            if wake {
                signal(connection);
            }
            if header.kind != INBAND {
                continue;
            }
            match &mut service.kind {
                Kind::Keyboard => key(&buffer[..len]),
                Kind::Ic(ic) => {
                    let Some((answer, action)) = ic.handle(buffer, len) else { continue };
                    match ring::send(rings, &buffer[..answer], header.transaction, false) {
                        Ok(true) => {
                            signal(connection);
                        }
                        Ok(false) => {}
                        Err(_) => {
                            warn!("Hyper-V: the {} service's ring is full", ic.service().name());
                        }
                    }
                    act(ic.service(), action);
                }
            }
        }
    }
}

/// A message from the synthetic keyboard.
fn key(data: &[u8]) {
    match keyboard::parse(data) {
        Some(keyboard::Message::Key { bytes, len }) => {
            for &byte in &bytes[..len] {
                crate::fs::keyboard::set1_byte(byte);
            }
        }
        Some(keyboard::Message::Protocol { accepted: true }) => {
            info!("Keyboard: Hyper-V's synthetic keyboard reads into /devices/kbd");
        }
        Some(keyboard::Message::Protocol { accepted: false }) => {
            warn!("Keyboard: Hyper-V's synthetic keyboard refused protocol 1.0");
        }
        _ => {}
    }
}

/// Does what a request of `service` asked.
fn act(service: Service, action: Action) {
    match action {
        Action::None => {}
        Action::Negotiated { framework, message } => info!(
            "Hyper-V: the {} service speaks framework {}.{}, messages {}.{}",
            service.name(),
            framework.0,
            framework.1,
            message.0,
            message.1
        ),
        Action::NoVersion => {
            warn!("Hyper-V: the {} service speaks no version the kernel does", service.name());
        }
        Action::PowerOff => crate::user::power::host_asked(),
        Action::Time { unix_ns, sync } => set_clock(unix_ns, sync),
    }
}

/// The host's clock: taken as it is when the host says to, as the guest
/// starts or resumes; a sample only when the clock is well off, as after
/// the host paused the machine, since the network time service keeps it
/// closer.
fn set_clock(unix_ns: u64, sync: bool) {
    let before = <Arch as Timer>::wall_clock_ns();
    let off = before.map(|before| unix_ns.abs_diff(before));
    if !sync && off.is_some_and(|off| off <= SAMPLE_STEP_NS) {
        return;
    }
    <Arch as Timer>::set_wall_clock_ns(unix_ns);
    match before {
        Some(before) => info!(
            "Time: Hyper-V set the clock ({:+} ms)",
            (i128::from(unix_ns) - i128::from(before)) / 1_000_000
        ),
        None => info!("Time: Hyper-V set the clock"),
    }
}

/// Waits `us` microseconds while a card starts: asleep, for a
/// millisecond or more.
fn delay(us: u32) {
    let us = u64::from(us);
    if us >= 1_000 {
        sched::sleep_ns(us * 1_000);
    } else {
        Arch::delay_us(us);
    }
}
