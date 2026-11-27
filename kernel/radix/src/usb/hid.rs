// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Keyboards, mice and tablets: HID interfaces (Phase 11a, ADR 0148).
//!
//! Once a device is configured, [`Controller::start_hid`] starts each of
//! its HID interfaces: it reads the interface's report descriptor and
//! lays it out (`radix_hid::Layout`), asks it to report only changes
//! (SET_IDLE; a device may refuse), asks a mouse's wheels to count finer
//! than a notch if they can (`crate::hid::fine_wheels`), lights a
//! keyboard's lock lights as consoled(8) last asked, and starts a
//! transfer on its interrupt IN endpoint. A boot keyboard or mouse whose report descriptor cannot be
//! read or laid out is put in the boot protocol, whose reports are fixed.
//!
//! Each interface that is read is an input device of its own
//! (`crate::hid::Port`, ADR 0706). Each report that comes
//! ([`Controller::hid_report`]) is decoded (`radix_hid::Decoder`) and
//! handed to the input hub as one frame, with the time of the
//! controller's interrupt; then the transfer starts again. When consoled
//! changes the lights, the thread is woken and sends each keyboard its
//! Output report ([`Controller::hid_lights`]).
//! A device unplugged lets go of every key and button it held.
//!
//! A UPS (`Layout::is_ups`, ADR 0741) is not listened to: its state is
//! in Feature reports, which the thread asks for every
//! [`crate::ups::POLL_NS`] ([`Controller::upses`]) and gives
//! [`crate::ups::update`].

use alloc::vec::Vec;

use core::fmt::Write;

use radix_abi::input::BUS_USB;
use radix_hid::power::State;
use radix_hid::{BOOT_KEYBOARD, BOOT_MOUSE, Decoder, Layout};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_usb::hid::{self, HidDescriptor};
use radix_usb::{Setup, Speed};
use radix_xhci::{BUFFER_SIZE, code, endpoint_index};

use super::controller::{Attached, Controller, Name, Path, Why};
use super::shown;
use crate::hid::{Feature, Identity, Port};
use crate::sched;

/// What the log says of a mouse that took its Resolution Multipliers
/// (`crate::hid::fine_wheels`).
pub(super) const FINER: &str = "its wheel counts finer than a notch";

/// Errors in a row on an interface's reports before it is given up.
const REPORT_ERRORS: u8 = 3;
/// The most HID interfaces of a device that are read.
pub(super) const MAX_HIDS: usize = 4;

/// A HID interface in a device's configuration.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct HidPlan {
    /// bInterfaceNumber.
    pub(super) interface: u8,
    /// Whether it is of the boot subclass, and its boot protocol.
    pub(super) boot: Option<u8>,
    /// How long its report descriptor is.
    pub(super) report_length: u16,
    /// Its interrupt IN endpoint's address.
    pub(super) endpoint: u8,
    /// That endpoint's packet size, transactions in a microframe and
    /// bInterval.
    pub(super) max_packet: u16,
    pub(super) transactions: u8,
    pub(super) interval: u8,
}

impl HidPlan {
    /// The HID interface `interface` of a configuration, with its
    /// descriptor's bytes `hid` and its interrupt IN endpoint, if it has
    /// what the driver needs.
    pub(super) fn of(
        interface: radix_usb::InterfaceDescriptor,
        hid: Option<&[u8]>,
        endpoint: Option<radix_usb::EndpointDescriptor>,
    ) -> Option<HidPlan> {
        let report_length = hid.and_then(HidDescriptor::parse).map_or(0, |d| d.report_length);
        let endpoint = endpoint?;
        Some(HidPlan {
            interface: interface.number,
            boot: (interface.subclass == hid::SUBCLASS_BOOT).then_some(interface.protocol),
            report_length,
            endpoint: endpoint.address,
            max_packet: endpoint.max_packet(),
            transactions: endpoint.extra_transactions() + 1,
            interval: endpoint.interval,
        })
    }
}

/// What the thread keeps of a HID interface it reads.
pub(super) struct Hid {
    interface: u8,
    /// Its interrupt IN endpoint's device context index.
    pub(super) endpoint: u8,
    /// The transfer on it, whose event is the one awaited.
    pub(super) pending: u64,
    /// The bytes each transfer asks for: its longest report.
    length: usize,
    layout: Layout<Kmalloc>,
    decoder: Decoder,
    /// Its input device, which goes when this does; none for a UPS.
    port: Option<Port>,
    /// Errors in a row on its endpoint.
    errors: u8,
    /// The lights its keyboard was last sent, if it has lights.
    lights: Option<u32>,
    /// What the thread keeps of it if it is a UPS.
    ups: Option<UpsRead>,
}

/// How many reports a second an interrupt endpoint with `interval` as
/// its bInterval asks to be read for, on a device of `speed`.
fn rate_hz(speed: Speed, interval: u8) -> u32 {
    match speed {
        // In milliseconds.
        Speed::Low | Speed::Full => 1000 / u32::from(interval.max(1)),
        // Two to the bInterval less one, in microframes of 125 µs.
        _ => 8000 >> (interval.clamp(1, 16) - 1),
    }
}

/// Makes HID interface `interface` of the device at `path` on bus `bus`,
/// laid out as `layout`, an input device: named as the device names
/// itself, or `name` if it has no product string, and plugged in at
/// `usb-1-2/0`, its interface after the device. `interval` is its
/// interrupt endpoint's bInterval. `None` if the hub has no room.
///
/// Not inlined: what the device said of itself, and what the hub is told
/// of it, are a frame of their own (ADR 0058).
#[inline(never)]
pub(super) fn port(
    bus: usize,
    path: Path,
    (interface, interval): (u8, u8),
    name: &str,
    layout: &Layout<Kmalloc>,
) -> Option<Port> {
    let shown = shown::of(bus, path);
    let mut place = TextBuf::<32>::new();
    let _ = write!(place, "usb-{}/{interface}", Name(bus, path));
    let product = shown.as_ref().map(|shown| shown.product.as_str()).filter(|p| !p.is_empty());
    let identity = Identity {
        bus: BUS_USB,
        name: product.unwrap_or(name),
        place: place.as_str(),
        serial: shown.as_ref().map_or("", |shown| shown.serial.as_str()),
        vendor: shown.as_ref().map_or(0, |shown| shown.device.vendor),
        product: shown.as_ref().map_or(0, |shown| shown.device.product),
        version: shown.as_ref().map_or(0, |shown| shown.device.release),
        rate_hz: shown.as_ref().map_or(0, |shown| rate_hz(shown.speed, interval)),
    };
    Port::add(layout, &identity)
}

/// A UPS's interface, which the thread reads its reports from.
struct UpsRead {
    /// Its slot in [`crate::ups`].
    slot: usize,
    /// What its reports said.
    state: State,
    /// When they are read next.
    next_ns: u64,
}

/// The endpoint and transfer a UPS's interface keeps: none, as no
/// transfer is made on it.
const NO_ENDPOINT: u8 = 0;
const NO_TRANSFER: u64 = u64::MAX;

impl Controller {
    /// Starts the HID interface `plan` of the configured device in slot
    /// `slot`.
    #[inline(never)]
    pub(super) fn start_hid(&mut self, slot: u8, plan: HidPlan) -> Result<(), Why> {
        let at = self.find(slot).ok_or(Why::Device("it is gone"))?;
        let Some(layout) = self.hid_layout(at, &plan)? else { return Ok(()) };
        let (bus, path, interface) = (self.bus, self.devices[at].path, plan.interface);
        if layout.is_ups() {
            return start_ups(&mut self.devices[at], bus, interface, layout);
        }
        let (what, read) = crate::hid::what(&layout);
        info!("USB: {}: interface {interface}: {what}", Name(bus, path));
        if !read {
            return Ok(());
        }
        let mut decoder = Decoder::new();
        if self.fine_wheels(at, interface, &layout, &mut decoder) {
            info!("USB: {}: interface {interface}: {FINER}", Name(bus, path));
        }
        let attached = &mut self.devices[at];
        attached.hids.try_reserve(1).map_err(|_| Why::Device("out of memory"))?;
        let _ = attached.hids.push_within_capacity(Hid {
            interface,
            endpoint: endpoint_index(plan.endpoint),
            pending: 0,
            length: layout.input_bytes().clamp(1, BUFFER_SIZE),
            lights: layout.has_lights().then_some(u32::MAX),
            port: port(bus, path, (interface, plan.interval), attached.name.as_str(), &layout),
            layout,
            decoder,
            errors: 0,
            ups: None,
        });
        let index = attached.hids.len() - 1;
        self.hid_lights(at);
        self.listen_hid(at, index)
    }

    /// How HID interface `plan` of the device at `at` lays its reports
    /// out: as its own report descriptor says; a boot interface that has
    /// none to give, or one too much for the decoder, falls back on the
    /// boot protocol. `None`, with a line in the log, for an interface
    /// whose descriptor says nothing that can be read.
    #[inline(never)]
    fn hid_layout(&mut self, at: usize, plan: &HidPlan) -> Result<Option<Layout<Kmalloc>>, Why> {
        let Controller { xhci, host, devices, buffer, bus, .. } = self;
        let attached = &mut devices[at];
        let (path, device) = (attached.path, &mut attached.device);
        let heap = HEAP.with(Flags::MAY_SLEEP);
        let interface = plan.interface;
        let length = usize::from(plan.report_length).min(BUFFER_SIZE);
        let own = (length > 0)
            .then(|| {
                let setup = Setup::report_descriptor(interface, length as u16);
                device.control_in(xhci, host, setup, &mut buffer[..length]).ok()
            })
            .flatten()
            .map(|came| Layout::parse(&buffer[..came], heap));
        let layout = match (own, plan.boot) {
            (Some(Ok(layout)), _) => layout,
            (own, Some(protocol @ (hid::PROTOCOL_KEYBOARD | hid::PROTOCOL_MOUSE))) => {
                if let Some(Err(err)) = own {
                    warn!(
                        "USB: {}: interface {interface}: {err}; using its boot reports",
                        Name(*bus, path)
                    );
                }
                device.control_out(xhci, host, Setup::set_protocol(interface, false), &[])?;
                let boot =
                    if protocol == hid::PROTOCOL_KEYBOARD { BOOT_KEYBOARD } else { BOOT_MOUSE };
                Layout::parse(boot, heap).map_err(|_| Why::Device("out of memory"))?
            }
            (Some(Err(err)), _) => {
                warn!("USB: {}: interface {interface}: {err}", Name(*bus, path));
                return Ok(None);
            }
            (None, _) => return Err(Why::Device("its report descriptor could not be read")),
        };
        // Reports only when something changes; a device that wants to
        // send them anyway may refuse.
        let _ = device.control_out(xhci, host, Setup::set_idle(interface), &[]);
        Ok(Some(layout))
    }

    /// Asks the wheels of HID interface `interface` of the device at
    /// `at` to count finer than a notch (`crate::hid::fine_wheels`).
    #[inline(never)]
    fn fine_wheels(
        &mut self,
        at: usize,
        interface: u8,
        layout: &Layout<Kmalloc>,
        decoder: &mut Decoder,
    ) -> bool {
        let Controller { xhci, host, devices, buffer, .. } = self;
        let device = &mut devices[at].device;
        let mut ask = |feature: Feature<'_>| match feature {
            Feature::Get(id, report) => {
                let setup =
                    Setup::get_report(interface, hid::REPORT_FEATURE, id, report.len() as u16);
                device.control_in(xhci, host, setup, report).ok()
            }
            Feature::Set(id, report) => {
                let setup =
                    Setup::set_report(interface, hid::REPORT_FEATURE, id, report.len() as u16);
                device.control_out(xhci, host, setup, report).ok().map(|()| 0)
            }
        };
        crate::hid::fine_wheels(layout, decoder, &mut buffer[..], &mut ask)
    }

    /// Starts the transfer on HID interface `index` of the device at `at`.
    fn listen_hid(&mut self, at: usize, index: usize) -> Result<(), Why> {
        let attached = &mut self.devices[at];
        let Some(hid) = attached.hids.get_mut(index) else { return Ok(()) };
        hid.pending = attached.device.submit_in(&self.xhci, hid.endpoint, hid.length)?;
        Ok(())
    }

    /// The transfer on HID interface `index` of the device at `at`
    /// finished with `code`, `residual` bytes short: tells what its report
    /// changed, and starts it again.
    #[inline(never)]
    pub(super) fn hid_report(&mut self, at: usize, index: usize, code: u8, residual: u32) {
        let Controller { devices, buffer, bus, xhci, host, came_ns, .. } = self;
        let attached = &mut devices[at];
        let (slot, path) = (attached.device.slot(), attached.path);
        let Some(hid) = attached.hids.get_mut(index) else { return };
        match code {
            code::SUCCESS | code::SHORT_PACKET => {
                hid.errors = 0;
                let came = hid.length.saturating_sub(residual as usize);
                if attached.device.received(hid.endpoint, &mut buffer[..came]).is_err() {
                    return;
                }
                if let Some(port) = &mut hid.port {
                    port.report(&mut hid.decoder, &hid.layout, &buffer[..came], *came_ns);
                }
            }
            // Stopped by the driver.
            code::STOPPED => return,
            failed => {
                hid.errors += 1;
                let (endpoint, errors, interface) = (hid.endpoint, hid.errors, hid.interface);
                if errors > REPORT_ERRORS {
                    warn!(
                        "USB: {}: interface {interface} stopped reporting: {}",
                        Name(*bus, path),
                        code::name(failed)
                    );
                    return;
                }
                if attached.device.recover(xhci, host, endpoint, failed).is_err() {
                    return;
                }
            }
        }
        if let Some(at) = self.find(slot)
            && let Err(why) = self.listen_hid(at, index)
        {
            warn!("USB: {}: its reports cannot be read: {why}", Name(self.bus, path));
        }
    }

    /// Sends the keyboards of the device at `at` the lock lights
    /// consoled(8) asked for, if they changed since.
    pub(super) fn hid_lights(&mut self, at: usize) {
        let leds = crate::input::lights();
        let Controller { devices, buffer, xhci, host, .. } = self;
        let attached = &mut devices[at];
        for hid in attached.hids.iter_mut().filter(|h| h.lights.is_some_and(|l| l != leds)) {
            hid.lights = Some(leds);
            let Some((id, len)) = hid.layout.lights_report(leds, &mut buffer[..]) else { continue };
            let setup = Setup::set_report(hid.interface, hid::REPORT_OUTPUT, id, len as u16);
            // A keyboard that will not take them is not asked again until
            // they change.
            let _ = attached.device.control_out(xhci, host, setup, &buffer[..len]);
        }
    }
}

/// Keeps HID interface `interface` of `attached`, on bus `bus`, as a UPS,
/// whose reports the thread asks for every [`crate::ups::POLL_NS`]. Out of
/// `Controller::start_hid` so that its frame stays small.
#[inline(never)]
fn start_ups(
    attached: &mut Attached,
    bus: usize,
    interface: u8,
    layout: Layout<Kmalloc>,
) -> Result<(), Why> {
    let path = attached.path;
    let (model, maker) = shown::strings(bus, path);
    let model = if model.is_empty() { attached.name.clone() } else { model };
    info!(
        "USB: {}: interface {interface}: a UPS, read every {} s into /kernel/class/power_supply",
        Name(bus, path),
        crate::ups::POLL_NS / 1_000_000_000
    );
    let Some(slot) = crate::ups::add(model.as_str(), maker.as_str()) else {
        warn!("USB: {}: too many UPSes; this one is not read", Name(bus, path));
        return Ok(());
    };
    if attached.hids.try_reserve(1).is_err() {
        crate::ups::remove(slot);
        return Err(Why::Device("out of memory"));
    }
    let _ = attached.hids.push_within_capacity(Hid {
        interface,
        endpoint: NO_ENDPOINT,
        pending: NO_TRANSFER,
        length: 0,
        layout,
        decoder: Decoder::new(),
        port: None,
        errors: 0,
        lights: None,
        ups: Some(UpsRead { slot, state: State::default(), next_ns: 0 }),
    });
    Ok(())
}

/// Lets go of every key and button that `hids`, a device's HID
/// interfaces, held, and of its UPS: the device is gone.
pub(super) fn gone(hids: &mut [Hid]) {
    for hid in hids {
        // Its input device goes, with what it held down.
        hid.port = None;
        if let Some(ups) = hid.ups.take() {
            crate::ups::remove(ups.slot);
        }
    }
}

impl Controller {
    /// Reads the reports of each UPS whose time has come; gives whether it
    /// read any.
    #[inline(never)]
    pub(super) fn upses(&mut self) -> bool {
        let now = sched::now_ns();
        let mut any = false;
        let Controller { devices, buffer, xhci, host, bus, .. } = self;
        for attached in devices.iter_mut() {
            for hid in &mut attached.hids {
                let Some(ups) = hid.ups.as_mut().filter(|ups| ups.next_ns <= now) else {
                    continue;
                };
                any = true;
                ups.next_ns = now.saturating_add(crate::ups::POLL_NS);
                let (layout, interface) = (&hid.layout, hid.interface);
                let mut answered = false;
                let mut each = |report: radix_hid::power::Report| {
                    let length = report.bytes.clamp(1, buffer.len());
                    let setup =
                        Setup::get_report(interface, report.kind as u8, report.id, length as u16);
                    if let Ok(came) =
                        attached.device.control_in(xhci, host, setup, &mut buffer[..length])
                    {
                        answered = true;
                        layout.read_power(report.kind, report.id, &buffer[..came], &mut ups.state);
                    }
                };
                layout.power_reports(&mut each);
                if answered {
                    hid.errors = 0;
                    crate::ups::update(ups.slot, ups.state);
                } else {
                    hid.errors = hid.errors.saturating_add(1);
                    if hid.errors == REPORT_ERRORS {
                        warn!(
                            "USB: {}: interface {interface}: the UPS answers no report",
                            Name(*bus, attached.path)
                        );
                    }
                }
            }
        }
        any
    }
}

/// An empty list of HID interfaces, for a device just set up.
pub(super) fn none() -> Vec<Hid, Kmalloc> {
    Vec::new_in(HEAP.with(Flags::MAY_SLEEP))
}
