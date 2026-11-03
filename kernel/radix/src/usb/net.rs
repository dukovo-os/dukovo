// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Ethernet adapters on USB (Phase 12, ADR 0199): CDC-ECM and CDC-NCM
//! (`radix_usb::net`), Realtek's RTL8153 and QEMU's `usb-net` among them,
//! in the configuration that has the adapter when the first is their
//! maker's own or RNDIS (`controller::choose_configuration`); and ASIX's
//! AX88179 and its kin in their own protocol (ADR 0362,
//! `radix_usb::net::asix`).
//!
//! Once a device is configured, [`Controller::start_net`] reads its MAC
//! address, asks an NCM adapter for the rules of its transfer blocks and
//! has it send none longer than a page, puts its data interface in the
//! setting with the bulk pair, has it pass on the frames to its own
//! address, broadcasts and every multicast frame, and gives it a network
//! interface (`net::program::attach_usb`), whose link the controller's
//! thread drives as e1000d drives a card's:
//!
//! - What comes on the bulk IN endpoint is a frame (ECM) or a block of
//!   them (NCM), put on the link for the stack, whose thread is woken.
//!   While the link is full the endpoint is not read, and the adapter
//!   holds or drops the frames, as a card would; the stack wakes the
//!   thread when it takes one.
//! - What the stack sends wakes the thread, which sends each frame on
//!   the bulk OUT endpoint, a transfer at a time: alone (ECM), or in a
//!   block of its own (NCM); a transfer that would end on a packet's
//!   boundary gets a byte more, as Linux's usbnet pads it, rather than a
//!   zero-length packet after it.
//! - The interrupt endpoint says when the link comes and goes, and its
//!   speed. An adapter is taken to have its link up, at 100 Mb/s, until
//!   it says otherwise: QEMU's says nothing.
//!
//! An ASIX adapter is brought up with its vendor's requests and polled
//! for its PHY's link each second, as FreeBSD's axge(4) does; it sends
//! its frames in bursts that may be longer than a transfer here, which
//! are joined until a transfer comes short, and takes each frame sent
//! after a header of its own.
//!
//! An adapter unplugged takes its link away: its interface goes down,
//! keeping its addresses, and is its own again when it comes back. Only
//! xHCI controllers carry the bulk transfers an adapter needs.

use alloc::sync::Arc;
use alloc::vec::Vec;

use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_nic::link::{self, Driver};
use radix_nic::{Link, MAX_FRAME, MIN_FRAME};
use radix_usb::net::asix::{self, mii, register};
use radix_usb::net::{self as usbnet, Notification, Port, Protocol, filter, ncm};
use radix_usb::{Setup, descriptor};
use radix_xhci::{Device, Host, Xhci, code, endpoint_index};

use super::controller::{Controller, Name, Why, alloc};
use crate::net::program::{self, Pages};
use crate::sched;

/// Errors in a row on an endpoint before the adapter is given up.
const PIPE_ERRORS: u8 = 3;
/// The bytes each read of an ECM adapter asks for: the longest frame,
/// in whole packets at every speed.
const ECM_READ: usize = 2048;
/// The longest block an NCM adapter is asked to send: a page, the most a
/// transfer here carries.
const NCM_READ: usize = 4096;
/// The bytes each read of an ASIX adapter asks for: a page, the most a
/// transfer here carries; its bursts are joined from them.
const ASIX_READ: usize = 4096;
/// The bytes a notification has at most that are read: a speed change's.
const NOTIFICATION: usize = 16;
/// The link an adapter is taken to have until it says.
const ASSUMED: Link = Link { up: true, mbps: 100, full_duplex: true };
/// The longest burst of an ASIX adapter that is kept, as FreeBSD's
/// axge(4) reads them: longer ones are dropped.
const ASIX_BURST: usize = 65536;
/// How often an ASIX adapter's PHY is asked for its link.
const ASIX_POLL_NS: u64 = 1_000_000_000;
/// How long an ASIX adapter's PHY has to come out of its reset.
const ASIX_PHY_RESET_TRIES: u32 = 50;

/// What the thread keeps of an adapter it carries.
pub(super) struct Ethernet {
    port: Port,
    /// Its interface's number among the cards.
    number: usize,
    /// The link to its interface's stack, and the thread's side of it.
    pages: Arc<Pages, Kmalloc>,
    side: Driver,
    /// Its endpoints' device context indices.
    input: u8,
    output: u8,
    notify: Option<u8>,
    /// The transfers in flight on them, 0 for none.
    reading: u64,
    writing: u64,
    listening: u64,
    /// The bytes each read asks for.
    read_length: usize,
    /// An NCM adapter's rules for the blocks it takes, and the next
    /// block's sequence number.
    blocks: Option<ncm::Parameters>,
    sequence: u16,
    /// Its link, as it last said.
    link: Link,
    /// Errors in a row on its endpoints.
    errors: u8,
    /// An ASIX adapter's own state.
    asix: Option<Asix>,
}

/// What the thread keeps of an ASIX adapter.
struct Asix {
    /// The burst it is sending, joined from transfers: its first `len`
    /// bytes, and whether it outgrew [`ASIX_BURST`] and is dropped.
    burst: Vec<u8, Kmalloc>,
    len: usize,
    overflowed: bool,
    /// Its USB speed, as its PLSR register says.
    plsr: u8,
    /// When its PHY was last asked for its link.
    polled_at: u64,
}

/// Runs `step` of an ASIX adapter's bring-up on `device`, and waits as it
/// asks.
fn asix_step(
    device: &mut Device,
    xhci: &mut Xhci,
    host: &mut dyn Host,
    step: asix::Step,
) -> Result<(), Why> {
    device.control_out(xhci, host, step.setup, &step.data)?;
    if step.then_ms > 0 {
        sched::sleep_ns(u64::from(step.then_ms) * 1_000_000);
    }
    Ok(())
}

/// Reads the PHY's register `register` of the ASIX adapter `device`.
fn asix_phy(
    device: &mut Device,
    xhci: &mut Xhci,
    host: &mut dyn Host,
    buffer: &mut [u8],
    register: u8,
) -> Option<u16> {
    let came = device.control_in(xhci, host, asix::phy_read(register), &mut buffer[..2]).ok()?;
    (came == 2).then(|| u16::from_le_bytes([buffer[0], buffer[1]]))
}

/// Writes `value` to the PHY's register `register` of the ASIX adapter
/// `device`.
fn asix_phy_write(
    device: &mut Device,
    xhci: &mut Xhci,
    host: &mut dyn Host,
    register: u8,
    value: u16,
) -> Result<(), Why> {
    device.control_out(xhci, host, asix::phy_write(register), &value.to_le_bytes())?;
    Ok(())
}

/// Brings up the ASIX adapter `model` on `device`, as FreeBSD's axge(4)
/// starts one: its PHY powered and its clocks on, its MAC address read
/// (or a random one given it), its filter and medium set, and its PHY
/// reset and negotiating. Says its MAC address and its USB speed.
#[inline(never)]
fn asix_start(
    device: &mut Device,
    xhci: &mut Xhci,
    host: &mut dyn Host,
    buffer: &mut [u8],
    model: asix::Model,
) -> Result<([u8; 6], u8), Why> {
    for step in asix::power_up(model) {
        asix_step(device, xhci, host, step)?;
    }
    let came = device.control_in(xhci, host, asix::read(register::NIDR, 6), &mut buffer[..6])?;
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&buffer[..6]);
    if came != 6 || !asix::usable(&mac) {
        mac = random_mac();
        device.control_out(xhci, host, asix::write(register::NIDR, 6), &mac)?;
    }
    for step in asix::configure() {
        asix_step(device, xhci, host, step)?;
    }
    let came = device.control_in(xhci, host, asix::read(register::PLSR, 1), &mut buffer[..1])?;
    let plsr = if came == 1 { buffer[0] } else { 0 };
    device.control_out(
        xhci,
        host,
        asix::write(register::BULK_IN, 5),
        &asix::bulk_in(1000, plsr),
    )?;
    // The PHY: reset, then every speed advertised and negotiation begun.
    asix_phy_write(device, xhci, host, mii::BMCR, mii::BMCR_RESET)?;
    for _ in 0..ASIX_PHY_RESET_TRIES {
        sched::sleep_ns(10_000_000);
        let bmcr = asix_phy(device, xhci, host, buffer, mii::BMCR);
        if bmcr.is_some_and(|bmcr| bmcr & mii::BMCR_RESET == 0) {
            break;
        }
    }
    asix_phy_write(device, xhci, host, mii::ANAR, mii::ANAR_ALL)?;
    asix_phy_write(device, xhci, host, mii::GTCR, mii::GTCR_ALL)?;
    asix_phy_write(device, xhci, host, mii::BMCR, mii::BMCR_AUTONEGOTIATE)?;
    Ok((mac, plsr))
}

/// The adapter `ethernet`, whose device is gone or stopped: its
/// interface goes down.
pub(super) fn gone(ethernet: &mut Option<Ethernet>) {
    if let Some(ethernet) = ethernet.take() {
        ethernet.side.set_link(&*ethernet.pages, Link::default());
        program::detach_usb(ethernet.number);
    }
}

/// A random, locally administered, unicast address, for an adapter whose
/// own cannot be read.
fn random_mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    radix_random::fill(&mut mac);
    radix_nic::local_mac(mac)
}

/// Gives the first `len` bytes of `out` to send a byte more, a zero, if
/// they would end on a boundary of `packet`-byte packets and `out` has
/// room for it: says how many to send.
fn padded(out: &mut [u8], len: usize, packet: u16) -> usize {
    let packet = usize::from(packet).max(1);
    if len.is_multiple_of(packet) && len < out.len() {
        out[len] = 0;
        len + 1
    } else {
        len
    }
}

impl Controller {
    /// Starts the Ethernet adapter `port` of the configured device in
    /// slot `slot`: gives it its setting and its interface, and starts
    /// reading it.
    #[inline(never)]
    pub(super) fn start_net(&mut self, slot: u8, port: Port) -> Result<(), Why> {
        let at = self.find(slot).ok_or(Why::Device("it is gone"))?;
        let Controller { devices, xhci, host, buffer, source, bus, .. } = self;
        let attached = &mut devices[at];
        let path = attached.path;
        let device = &mut attached.device;
        let mut asix = None;
        let mac = match (port.protocol, port.mac_string) {
            (Protocol::Asix(model), _) => {
                let (mac, plsr) = asix_start(device, xhci, host, buffer, model)?;
                let mut burst = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
                burst.try_reserve_exact(ASIX_BURST).map_err(|_| Why::Device("out of memory"))?;
                while burst.push_within_capacity(0).is_ok() {}
                asix = Some(Asix { burst, len: 0, overflowed: false, plsr, polled_at: 0 });
                Some(mac)
            }
            (_, 0) => None,
            (_, index) => {
                let setup = Setup::get_descriptor(descriptor::STRING, index, 0x0409, 64);
                device
                    .control_in(xhci, host, setup, &mut buffer[..64])
                    .ok()
                    .and_then(|came| usbnet::mac_from_string(&buffer[..came]))
            }
        };
        let mac = mac.unwrap_or_else(random_mac);
        let (read_length, blocks) = match port.protocol {
            Protocol::Ecm => (ECM_READ, None),
            Protocol::Asix(_) => (ASIX_READ, None),
            Protocol::Ncm => {
                let setup = usbnet::get_ntb_parameters(port.control);
                let came = device.control_in(xhci, host, setup, &mut buffer[..ncm::PARAMETERS])?;
                let parameters = ncm::Parameters::parse(&buffer[..came])
                    .ok_or(Why::Device("its transfer blocks have rules it cannot keep"))?;
                if parameters.in_max as usize > NCM_READ {
                    let size = usbnet::ntb_input_size(NCM_READ as u32);
                    let setup = usbnet::set_ntb_input_size(port.control);
                    device.control_out(xhci, host, setup, &size)?;
                }
                (NCM_READ.min(parameters.in_max as usize), Some(parameters))
            }
        };
        // Its second setting has the bulk pair, as a rule; an adapter
        // whose first has them was given them as it was configured.
        if port.alternate != 0 {
            let mut alloc = alloc(*source);
            let setting = (port.data, port.alternate);
            let pair = [port.bulk_in, port.bulk_out];
            device.set_interface(xhci, host, &mut alloc, setting, &[], &pair)?;
        }
        // An adapter that passes every frame may refuse the request, as
        // QEMU's does; Linux goes on too. An ASIX adapter's filter is its
        // own register, set as it was brought up.
        if asix.is_none() {
            let passed = filter::DIRECTED | filter::BROADCAST | filter::ALL_MULTICAST;
            let setup = usbnet::set_packet_filter(port.control, passed);
            let _ = device.control_out(xhci, host, setup, &[]);
        }
        let place = Name(*bus, path);
        let (number, pages) = program::attach_usb(*bus - 1, mac, port.protocol.name(), &place)
            .ok_or(Why::Device("it could not be given an interface"))?;
        let side = Driver::new();
        // An ASIX adapter's link is down until its PHY says otherwise.
        let link = if asix.is_some() { Link::default() } else { ASSUMED };
        side.set_link(&*pages, link);
        program::wake(number);
        attached.ethernet = Some(Ethernet {
            port,
            number,
            pages,
            side,
            input: endpoint_index(port.bulk_in.address),
            output: endpoint_index(port.bulk_out.address),
            notify: port.notify.map(|endpoint| endpoint_index(endpoint.address)),
            reading: 0,
            writing: 0,
            listening: 0,
            read_length,
            blocks,
            sequence: 0,
            link,
            errors: 0,
            asix,
        });
        self.net_listen(at);
        Ok(())
    }

    /// Starts the reads of the adapter of the device at `at` that are not
    /// running: its frames', while its link has room, and its
    /// notifications'.
    fn net_listen(&mut self, at: usize) {
        let attached = &mut self.devices[at];
        let Some(ethernet) = attached.ethernet.as_mut() else { return };
        let memory = &*ethernet.pages;
        let mut room = ethernet.side.can_receive(memory);
        if ethernet.reading == 0 && !room {
            // Asked before looking again, so that a frame taken between
            // the two still wakes the thread.
            memory.want_room();
            room = ethernet.side.can_receive(memory);
        }
        if ethernet.reading == 0 && room {
            match attached.device.submit_in(&self.xhci, ethernet.input, ethernet.read_length) {
                Ok(trb) => ethernet.reading = trb,
                Err(err) => {
                    warn!(
                        "USB: {}: its network adapter cannot be read: {err}",
                        Name(self.bus, attached.path)
                    );
                    gone(&mut attached.ethernet);
                    return;
                }
            }
        }
        if let (Some(notify), 0) = (ethernet.notify, ethernet.listening) {
            match attached.device.submit_in(&self.xhci, notify, NOTIFICATION) {
                Ok(trb) => ethernet.listening = trb,
                // Its link is left as it is.
                Err(_) => ethernet.notify = None,
            }
        }
    }

    /// Carries every adapter: sends what its stack has for it, and reads
    /// it again once its link has room. Says whether anything moved.
    pub(super) fn net(&mut self) -> bool {
        let mut any = false;
        let now = sched::now_ns();
        for at in 0..self.devices.len() {
            if self.devices[at].ethernet.is_some() {
                any |= self.net_send(at);
                self.net_poll(at, now);
                self.net_listen(at);
            }
        }
        any
    }

    /// Asks the PHY of the ASIX adapter of the device at `at` for its link,
    /// if it was last asked a second or more before `now`, and sets the
    /// adapter's medium and bursts to the link it has.
    #[inline(never)]
    fn net_poll(&mut self, at: usize, now: u64) {
        let Controller { devices, buffer, xhci, host, bus, .. } = self;
        let attached = &mut devices[at];
        let Some(ethernet) = attached.ethernet.as_mut() else { return };
        let Some(asix) = ethernet.asix.as_mut() else { return };
        if asix.polled_at != 0 && now.saturating_sub(asix.polled_at) < ASIX_POLL_NS {
            return;
        }
        asix.polled_at = now.max(1);
        let device = &mut attached.device;
        let mut phy = |register| asix_phy(device, xhci, host, buffer, register);
        // BMSR keeps a loss of link until it is read: the second read says
        // how it is now.
        let _ = phy(mii::BMSR);
        let (Some(bmsr), Some(anar), Some(anlpar), Some(gtcr), Some(gtsr)) =
            (phy(mii::BMSR), phy(mii::ANAR), phy(mii::ANLPAR), phy(mii::GTCR), phy(mii::GTSR))
        else {
            return;
        };
        let found = asix::resolve(bmsr, anar, anlpar, gtcr, gtsr);
        let link = found.map_or(Link::default(), |medium| Link {
            up: true,
            mbps: medium.mbps,
            full_duplex: medium.full_duplex,
        });
        if link == ethernet.link {
            return;
        }
        if let Some(medium) = found {
            let bursts = asix::bulk_in(medium.mbps, asix.plsr);
            let _ = device.control_out(xhci, host, asix::write(register::BULK_IN, 5), &bursts);
            let value = asix::medium(medium).to_le_bytes();
            let _ = device.control_out(xhci, host, asix::write(register::MSR, 2), &value);
        }
        if link.up != ethernet.link.up {
            let state = if link.up { "up" } else { "down" };
            info!("USB: {}: its network adapter's link is {state}", Name(*bus, attached.path));
        }
        ethernet.link = link;
        ethernet.side.set_link(&*ethernet.pages, link);
        program::wake(ethernet.number);
    }

    /// Sends the next frame the stack has for the adapter of the device
    /// at `at`, unless one is being sent. Says whether it took one.
    fn net_send(&mut self, at: usize) -> bool {
        let Controller { devices, buffer, xhci, bus, .. } = self;
        let attached = &mut devices[at];
        let Some(ethernet) = attached.ethernet.as_mut() else { return false };
        if ethernet.writing != 0 {
            return false;
        }
        let (frame, block) = buffer.split_at_mut(link::SLOT);
        let Some(len) = ethernet.side.take(&*ethernet.pages, frame) else { return false };
        if !(MIN_FRAME..=MAX_FRAME).contains(&len) {
            return true;
        }
        let packet = ethernet.port.bulk_out.max_packet();
        let out: &[u8] = match ethernet.blocks {
            // After a header, and a second one if the transfer would end
            // on a packet's boundary.
            None if ethernet.asix.is_some() => {
                let Some(built) = asix::build(&frame[..len], packet, block) else { return true };
                &block[..built]
            }
            None => {
                let len = padded(frame, len, packet);
                &frame[..len]
            }
            Some(parameters) => {
                let sequence = ethernet.sequence;
                ethernet.sequence = sequence.wrapping_add(1);
                let Some(built) = ncm::build(&parameters, sequence, &frame[..len], block) else {
                    return true;
                };
                // The pad byte is past the block's own length, and only
                // where the adapter takes it.
                let room = block.len().min(parameters.out_max as usize);
                let len = padded(&mut block[..room], built, packet);
                &block[..len]
            }
        };
        match attached.device.submit_out(xhci, ethernet.output, out) {
            Ok(trb) => ethernet.writing = trb,
            Err(err) => warn!(
                "USB: {}: a frame its network adapter was to send is lost: {err}",
                Name(*bus, attached.path)
            ),
        }
        true
    }

    /// Whether the transfer at `trb` on `endpoint` of the device at `at`
    /// is its adapter's; if so, takes what it brought.
    pub(super) fn net_transfer(
        &mut self,
        at: usize,
        trb: u64,
        endpoint: u8,
        code: u8,
        residual: u32,
    ) -> bool {
        let Some(ethernet) = self.devices[at].ethernet.as_ref() else { return false };
        if ethernet.input == endpoint && ethernet.reading == trb {
            self.net_received(at, code, residual);
        } else if ethernet.output == endpoint && ethernet.writing == trb {
            self.net_sent(at, code);
        } else if ethernet.notify == Some(endpoint) && ethernet.listening == trb {
            self.net_notified(at, code, residual);
        } else {
            return false;
        }
        self.net_send(at);
        self.net_listen(at);
        true
    }

    /// The read on the adapter of the device at `at` ended with `code`,
    /// `residual` bytes short: its frames go on the link.
    #[inline(never)]
    fn net_received(&mut self, at: usize, code: u8, residual: u32) {
        let Controller { devices, buffer, xhci, host, bus, .. } = self;
        let attached = &mut devices[at];
        let Some(ethernet) = attached.ethernet.as_mut() else { return };
        ethernet.reading = 0;
        match code {
            code::SUCCESS | code::SHORT_PACKET => {
                ethernet.errors = 0;
                let came = ethernet.read_length.saturating_sub(residual as usize);
                if attached.device.received(ethernet.input, &mut buffer[..came]).is_err() {
                    return;
                }
                let memory = &*ethernet.pages;
                let side = &mut ethernet.side;
                let mut give = |frame: &[u8]| {
                    if (MIN_FRAME..=MAX_FRAME).contains(&frame.len()) {
                        side.received(memory, frame);
                    }
                };
                match (ethernet.blocks, ethernet.asix.as_mut()) {
                    (_, Some(asix)) => {
                        // A burst goes on until a transfer comes short.
                        let room = asix.burst.len() - asix.len;
                        if asix.overflowed || came > room {
                            asix.overflowed = true;
                        } else {
                            asix.burst[asix.len..asix.len + came].copy_from_slice(&buffer[..came]);
                            asix.len += came;
                        }
                        if came == ethernet.read_length {
                            return;
                        }
                        if !asix.overflowed {
                            let burst = &asix.burst[..asix.len];
                            for (offset, len) in asix::frames(burst) {
                                give(&burst[offset..offset + len]);
                            }
                        }
                        asix.len = 0;
                        asix.overflowed = false;
                    }
                    (None, None) => give(&buffer[..came]),
                    (Some(_), None) => {
                        for (offset, len) in ncm::frames(&buffer[..came]) {
                            give(&buffer[offset..offset + len]);
                        }
                    }
                }
                program::wake(ethernet.number);
            }
            code::STOPPED => {}
            failed => {
                ethernet.errors += 1;
                let input = ethernet.input;
                if ethernet.errors > PIPE_ERRORS
                    || attached.device.recover(xhci, host, input, failed).is_err()
                {
                    warn!(
                        "USB: {}: its network adapter stopped: {}",
                        Name(*bus, attached.path),
                        code::name(failed)
                    );
                    gone(&mut attached.ethernet);
                }
            }
        }
    }

    /// The write on the adapter of the device at `at` ended with `code`.
    fn net_sent(&mut self, at: usize, code: u8) {
        let Controller { devices, xhci, host, bus, .. } = self;
        let attached = &mut devices[at];
        let Some(ethernet) = attached.ethernet.as_mut() else { return };
        ethernet.writing = 0;
        match code {
            code::SUCCESS | code::SHORT_PACKET => ethernet.errors = 0,
            code::STOPPED => {}
            failed => {
                ethernet.errors += 1;
                let output = ethernet.output;
                if ethernet.errors > PIPE_ERRORS
                    || attached.device.recover(xhci, host, output, failed).is_err()
                {
                    warn!(
                        "USB: {}: its network adapter stopped sending: {}",
                        Name(*bus, attached.path),
                        code::name(failed)
                    );
                    gone(&mut attached.ethernet);
                }
            }
        }
    }

    /// The adapter of the device at `at` said something on its interrupt
    /// endpoint, ended with `code`, `residual` bytes short: its link came
    /// or went, or changed its speed.
    fn net_notified(&mut self, at: usize, code: u8, residual: u32) {
        let Controller { devices, buffer, bus, .. } = self;
        let attached = &mut devices[at];
        let Some(ethernet) = attached.ethernet.as_mut() else { return };
        ethernet.listening = 0;
        let Some(notify) = ethernet.notify else { return };
        if !matches!(code, code::SUCCESS | code::SHORT_PACKET) {
            // Not read again: its link stays as it is.
            ethernet.notify = None;
            return;
        }
        let came = NOTIFICATION.saturating_sub(residual as usize);
        if attached.device.received(notify, &mut buffer[..came]).is_err() {
            return;
        }
        let mut link = ethernet.link;
        match usbnet::notification(&buffer[..came]) {
            Some(Notification::Connection(up)) => link.up = up,
            Some(Notification::Speed(down, _)) if down >= 1_000_000 => {
                link.mbps = down / 1_000_000;
            }
            _ => return,
        }
        if link == ethernet.link {
            return;
        }
        if link.up != ethernet.link.up {
            let state = if link.up { "up" } else { "down" };
            info!("USB: {}: its network adapter's link is {state}", Name(*bus, attached.path));
        }
        ethernet.link = link;
        ethernet.side.set_link(&*ethernet.pages, link);
        program::wake(ethernet.number);
    }
}
