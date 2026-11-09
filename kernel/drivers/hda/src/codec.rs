// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A codec's outputs: which of its pins play, and the way from each to
//! a converter (section 7).
//!
//! A codec is a graph of widgets in its audio function group: converters
//! (DACs) turn the link's samples into sound, mixers add their inputs,
//! selectors pass one, and pins are the jacks and the built-in speakers.
//! Which pin is which in this machine the firmware says, in each pin's
//! configuration default: whether anything is connected, and whether it
//! is a speaker, headphones or a line out. The driver plays through
//! every output pin the firmware says is connected, of those three kinds,
//! that is not digital (HDMI, DisplayPort and S/PDIF wait for later).
//!
//! From each such pin the driver searches the graph, breadth first, for
//! the converters it reaches through mixers and selectors, and picks the
//! one the most pins reach, so that one converter plays everywhere; a pin
//! that cannot reach it gets its own, on the same stream, which the
//! converters all take. Along each pin's way it chooses the input of
//! each pin and selector, unmutes the input of each mixer and mutes the
//! mixer's others (the codec's analog loopback, which would otherwise
//! add the microphone), turns every amplifier to 0 dB, powers each widget
//! up and turns the pin's external amplifier (EAPD) on. The volume is the
//! first amplifier with steps from the converter outward on each way; if
//! a way has none, the kernel scales the samples instead.
//!
//! Headphones take the sound from the speakers: when a jack that senses
//! what is plugged in has headphones in it, the speakers and line outs
//! are turned off, and when a line out has something in it, the speakers
//! are. A pin whose jack cannot sense always plays, and turns nothing
//! off. The jacks are read each time the output is looked at
//! ([`Codec::refresh`]).

use crate::controller::Controller;
use crate::verb::{self, AmpCaps, Config, amp, param, pincap, wcap};
use crate::{Error, Host};

/// The most output pins the driver plays through.
pub const MAX_PINS: usize = 8;
/// The most widgets on the way from a pin to its converter, both
/// included.
const MAX_HOPS: usize = 6;
/// The most entries of a connection list the driver reads.
const MAX_CONNECTIONS: usize = 32;

/// What a pin plays to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A built-in speaker.
    Speaker,
    /// A headphone jack.
    Headphones,
    /// A line out jack.
    Line,
}

/// One widget on a pin's way to its converter.
#[derive(Clone, Copy, Debug, Default)]
struct Hop {
    nid: u8,
    caps: u32,
    /// The entry of its connection list that leads to the next widget.
    select: u8,
    /// How long its connection list is.
    connections: u8,
}

/// The way from a pin (the first hop) to a converter (the last).
#[derive(Clone, Copy, Debug, Default)]
struct Path {
    hops: [Hop; MAX_HOPS],
    len: usize,
    /// The amplifier that sets the volume, and its capabilities.
    volume: Option<(u8, AmpCaps)>,
}

impl Path {
    fn dac(&self) -> u8 {
        self.hops[self.len - 1].nid
    }
}

/// An output pin.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    /// Its node.
    pub nid: u8,
    /// What it plays to.
    pub role: Role,
    /// Its configuration default.
    pub config: Config,
    caps: u32,
    /// Whether its jack says what is plugged in.
    sensed: bool,
    path: Path,
    /// Whether something is plugged in, as last read (true for a pin that
    /// cannot sense).
    present: bool,
    /// Whether it plays.
    on: bool,
    /// Whether its control was set: each pin's is the first time the
    /// jacks are read (the firmware may have left any on), and after that
    /// when it changes.
    set: bool,
}

impl Pin {
    /// The converter it plays from.
    pub fn dac(&self) -> u8 {
        self.path.dac()
    }

    /// Whether it plays now.
    pub fn on(&self) -> bool {
        self.on
    }
}

/// What plays now: a bit for each role with a pin that plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Route {
    /// The speakers play.
    pub speakers: bool,
    /// Headphones play.
    pub headphones: bool,
    /// A line out plays.
    pub line: bool,
}

/// Commands to one codec.
pub struct Link<'a> {
    controller: &'a mut Controller,
    host: &'a mut dyn Host,
    cad: u8,
}

impl<'a> Link<'a> {
    /// Commands to codec `cad` on `controller`.
    pub fn new(controller: &'a mut Controller, host: &'a mut dyn Host, cad: u8) -> Link<'a> {
        Link { controller, host, cad }
    }

    /// Sends a 12-bit verb.
    pub fn verb(&mut self, nid: u8, verb: u16, payload: u8) -> Result<u32, Error> {
        self.controller.command(self.cad, nid, verb::v12(verb, payload), self.host)
    }

    /// Sends a 4-bit verb.
    pub fn verb4(&mut self, nid: u8, verb: u8, payload: u16) -> Result<u32, Error> {
        self.controller.command(self.cad, nid, verb::v4(verb, payload), self.host)
    }

    /// Reads a parameter.
    pub fn param(&mut self, nid: u8, which: u8) -> Result<u32, Error> {
        self.verb(nid, verb::GET_PARAMETER, which)
    }

    /// Waits `us` microseconds.
    fn delay_us(&mut self, us: u32) {
        self.host.delay_us(us);
    }
}

/// A connection list, read whole.
struct Connections {
    nids: [u8; MAX_CONNECTIONS],
    len: usize,
}

impl Connections {
    fn as_slice(&self) -> &[u8] {
        &self.nids[..self.len]
    }

    fn push(&mut self, nid: u32) {
        // A node past 255 cannot be addressed by a command; one past the
        // most read is left out, and with it every later entry.
        if self.len < MAX_CONNECTIONS
            && let Ok(nid) = u8::try_from(nid)
        {
            self.nids[self.len] = nid;
            self.len += 1;
        }
    }
}

/// Reads the connection list of widget `nid` (7.3.3.3): entries of 8
/// bits (four a response) or, in the long form, of 16 (two); an entry
/// with its top bit set stands for every node from the one before it to
/// itself.
fn connections(link: &mut Link<'_>, nid: u8, caps: u32) -> Result<Connections, Error> {
    let mut list = Connections { nids: [0; MAX_CONNECTIONS], len: 0 };
    if caps & wcap::CONN_LIST == 0 {
        return Ok(list);
    }
    let length = link.param(nid, param::CONNECTION_LIST_LEN)?;
    let long = length & 0x80 != 0;
    let count = (length & 0x7f) as usize;
    let (per, bits) = if long { (2, 16) } else { (4, 8) };
    let range_bit = 1u32 << (bits - 1);
    let mask = (1u32 << bits) - 1;
    let mut previous = 0;
    let mut index = 0;
    while index < count && list.len < MAX_CONNECTIONS {
        let response = link.verb(nid, verb::GET_CONNECTION_LIST, index as u8)?;
        for slot in 0..per {
            if index + slot >= count {
                break;
            }
            let entry = (response >> (slot * bits)) & mask;
            let node = entry & !range_bit;
            if entry & range_bit != 0 && previous != 0 && previous < node {
                for each in previous + 1..=node {
                    list.push(each);
                }
            } else {
                list.push(node);
            }
            previous = node;
        }
        index += per;
    }
    Ok(list)
}

/// Whether a widget of `caps` can be on the way from a pin to its
/// converter: a mixer, a selector, or the converter itself (an analog
/// one). Capabilities of 0 are a node that is not there, which answers
/// every parameter with 0, not a converter of no abilities.
fn passes(caps: u32) -> bool {
    if caps == 0 || caps == u32::MAX {
        return false;
    }
    match wcap::kind(caps) {
        wcap::MIXER | wcap::SELECTOR => true,
        wcap::OUTPUT => caps & wcap::DIGITAL == 0,
        _ => false,
    }
}

/// What a breadth-first search from a pin found: how each widget it
/// reached was reached (the widget before it, and the entry of that
/// widget's list that leads to it).
struct Search {
    from: [u8; 256],
    entry: [u8; 256],
    /// The converters reached, nearest first.
    dacs: [u8; MAX_PINS],
    found: usize,
}

/// Searches from pin `pin` (with caps `caps`) through the widgets from
/// `first` to `last` for the converters it reaches.
fn search(link: &mut Link<'_>, pin: u8, caps: u32, first: u8, last: u8) -> Result<Search, Error> {
    let mut found = Search { from: [0; 256], entry: [0; 256], dacs: [0; MAX_PINS], found: 0 };
    let mut depth = [0u8; 256];
    let mut queue = [0u8; 256];
    let (mut head, mut tail) = (0, 0);
    queue[tail] = pin;
    tail += 1;
    found.from[usize::from(pin)] = pin;
    while head < tail {
        let nid = queue[head];
        head += 1;
        let caps = if nid == pin { caps } else { link.param(nid, param::WIDGET_CAPS)? };
        if nid != pin && wcap::kind(caps) == wcap::OUTPUT {
            if found.found < MAX_PINS {
                found.dacs[found.found] = nid;
                found.found += 1;
            }
            continue;
        }
        if usize::from(depth[usize::from(nid)]) + 1 >= MAX_HOPS {
            continue;
        }
        let list = connections(link, nid, caps)?;
        for (index, &next) in list.as_slice().iter().enumerate() {
            if next < first || next > last || found.from[usize::from(next)] != 0 {
                continue;
            }
            let next_caps = link.param(next, param::WIDGET_CAPS)?;
            if !passes(next_caps) {
                continue;
            }
            found.from[usize::from(next)] = nid;
            found.entry[usize::from(next)] = index as u8;
            depth[usize::from(next)] = depth[usize::from(nid)] + 1;
            if tail < queue.len() {
                queue[tail] = next;
                tail += 1;
            }
        }
    }
    Ok(found)
}

/// The way from pin `pin` to converter `dac`, as `search` found it.
fn path_to(link: &mut Link<'_>, found: &Search, pin: u8, dac: u8) -> Result<Path, Error> {
    // Back from the converter to the pin, then turned round.
    let mut nids = [0u8; MAX_HOPS];
    let mut len = 0;
    let mut nid = dac;
    loop {
        if len == MAX_HOPS {
            return Err(Error::NoOutput);
        }
        nids[len] = nid;
        len += 1;
        if nid == pin {
            break;
        }
        nid = found.from[usize::from(nid)];
    }
    nids[..len].reverse();
    let mut path = Path { len, ..Path::default() };
    for (i, &nid) in nids[..len].iter().enumerate() {
        let caps = link.param(nid, param::WIDGET_CAPS)?;
        let connections = if caps & wcap::CONN_LIST != 0 {
            (link.param(nid, param::CONNECTION_LIST_LEN)? & 0x7f) as u8
        } else {
            0
        };
        let select = if i + 1 < len { found.entry[usize::from(nids[i + 1])] } else { 0 };
        path.hops[i] = Hop { nid, caps, select, connections };
    }
    Ok(path)
}

/// A codec, with the outputs the driver plays through.
pub struct Codec {
    /// Its address on the link.
    pub cad: u8,
    /// Its audio function group.
    pub afg: u8,
    /// Its vendor (bits 31:16) and device (15:0).
    pub vendor: u32,
    /// The card's vendor (31:16) and model (15:0), from the firmware.
    pub subsystem: u32,
    pins: [Option<Pin>; MAX_PINS],
    /// The converters that play, the first of them the one most pins
    /// play from.
    dacs: [u8; MAX_PINS],
    dac_count: usize,
    /// The rates every converter takes: bits of [`verb::PCM_RATES`].
    pub rates: u32,
    /// Whether every converter takes two channels.
    pub stereo: bool,
    /// Whether every way has an amplifier that sets the volume.
    pub hardware_volume: bool,
    /// The group's amplifiers, for widgets that have none of their own.
    afg_in_amp: AmpCaps,
    afg_out_amp: AmpCaps,
    /// The group's rates and sizes, for converters that have none of
    /// their own.
    afg_pcm: u32,
}

impl Codec {
    /// The codec at address `cad`, if it has an audio function group with
    /// an output the driver plays through: found, routed and powered up,
    /// its converters taking the output's stream.
    pub fn probe(link: &mut Link<'_>) -> Result<Option<Codec>, Error> {
        let vendor = link.param(0, param::VENDOR_ID)?;
        if vendor == 0 || vendor == 0xffff_ffff {
            return Ok(None);
        }
        let groups = link.param(0, param::NODE_COUNT)?;
        let (start, count) = ((groups >> 16) & 0xff, groups & 0xff);
        let mut afg = None;
        for nid in start..(start + count).min(256) {
            let nid = nid as u8;
            if link.param(nid, param::FUNCTION_TYPE)? & 0xff == verb::FG_AUDIO {
                afg = Some(nid);
                break;
            }
        }
        let Some(afg) = afg else { return Ok(None) };
        power_up(link, afg)?;
        let subsystem = link.verb(afg, verb::GET_SUBSYSTEM_ID, 0)?;
        crate::quirks::apply(link, vendor, subsystem)?;
        let mut codec = Codec {
            cad: link.cad,
            afg,
            vendor,
            subsystem,
            pins: [None; MAX_PINS],
            dacs: [0; MAX_PINS],
            dac_count: 0,
            rates: 0,
            stereo: true,
            hardware_volume: true,
            afg_in_amp: AmpCaps(link.param(afg, param::IN_AMP_CAPS)?),
            afg_out_amp: AmpCaps(link.param(afg, param::OUT_AMP_CAPS)?),
            afg_pcm: link.param(afg, param::PCM)?,
        };
        let widgets = link.param(afg, param::NODE_COUNT)?;
        let first = ((widgets >> 16) & 0xff) as u8;
        let last = (((widgets >> 16) & 0xff) + (widgets & 0xff)).saturating_sub(1).min(255) as u8;
        if widgets & 0xff == 0 {
            return Ok(None);
        }
        let found = codec.find_pins(link, first, last)?;
        if found == 0 {
            return Ok(None);
        }
        codec.choose_paths(link, first, last)?;
        if codec.dac_count == 0 {
            return Ok(None);
        }
        codec.read_formats(link)?;
        if codec.rates == 0 {
            return Ok(None);
        }
        codec.route(link)?;
        codec.refresh(link)?;
        Ok(Some(codec))
    }

    /// The output pins the driver plays through, speakers first, then
    /// headphones, then line outs, each in the order of their nodes.
    pub fn pins(&self) -> impl Iterator<Item = &Pin> {
        self.pins.iter().flatten()
    }

    /// The converters that play.
    pub fn dacs(&self) -> &[u8] {
        &self.dacs[..self.dac_count]
    }

    /// What plays now.
    pub fn route_now(&self) -> Route {
        let mut route = Route::default();
        for pin in self.pins().filter(|pin| pin.on) {
            match pin.role {
                Role::Speaker => route.speakers = true,
                Role::Headphones => route.headphones = true,
                Role::Line => route.line = true,
            }
        }
        route
    }

    /// Finds the output pins, sorted by role; returns how many.
    fn find_pins(&mut self, link: &mut Link<'_>, first: u8, last: u8) -> Result<usize, Error> {
        let mut count = 0;
        for role in [Role::Speaker, Role::Headphones, Role::Line] {
            for nid in first..=last {
                if count == MAX_PINS {
                    return Ok(count);
                }
                let caps = link.param(nid, param::WIDGET_CAPS)?;
                if wcap::kind(caps) != wcap::PIN || caps & wcap::DIGITAL != 0 {
                    continue;
                }
                let pin_caps = link.param(nid, param::PIN_CAPS)?;
                if pin_caps & pincap::OUTPUT == 0 || pin_caps & (pincap::HDMI | pincap::DP) != 0 {
                    continue;
                }
                let config = Config(link.verb(nid, verb::GET_CONFIG_DEFAULT, 0)?);
                let Some(this) = role_of(config) else { continue };
                if this != role {
                    continue;
                }
                let sensed = pin_caps & pincap::PRESENCE != 0
                    && !config.no_presence_detect()
                    && config.connectivity() != verb::CONN_FIXED;
                self.pins[count] = Some(Pin {
                    nid,
                    role,
                    config,
                    caps: pin_caps,
                    sensed,
                    path: Path::default(),
                    present: !sensed,
                    on: false,
                    set: false,
                });
                count += 1;
            }
        }
        Ok(count)
    }

    /// Finds each pin's way to a converter: the one the most pins reach
    /// if it can, else the nearest it reaches. A pin that reaches none is
    /// dropped.
    fn choose_paths(&mut self, link: &mut Link<'_>, first: u8, last: u8) -> Result<(), Error> {
        // Which converters each pin reaches.
        let mut reach = [[0u8; MAX_PINS]; MAX_PINS];
        let mut reached = [0usize; MAX_PINS];
        for (i, pin) in self.pins.iter().enumerate() {
            let Some(pin) = pin else { continue };
            let caps = link.param(pin.nid, param::WIDGET_CAPS)?;
            let found = search(link, pin.nid, caps, first, last)?;
            reach[i] = found.dacs;
            reached[i] = found.found;
        }
        // The converter the most pins reach; the first pin's nearest
        // first among equals.
        let mut best = None;
        let mut best_count = 0;
        for (i, dacs) in reach.iter().enumerate() {
            for &dac in &dacs[..reached[i]] {
                let count =
                    (0..MAX_PINS).filter(|&j| reach[j][..reached[j]].contains(&dac)).count();
                if count > best_count {
                    best = Some(dac);
                    best_count = count;
                }
            }
        }
        let Some(best) = best else {
            self.pins = [None; MAX_PINS];
            return Ok(());
        };
        self.dacs[0] = best;
        self.dac_count = 1;
        for i in 0..MAX_PINS {
            let Some(mut pin) = self.pins[i] else { continue };
            let dacs = &reach[i][..reached[i]];
            let dac = if dacs.contains(&best) {
                best
            } else if let Some(&nearest) = dacs.first() {
                nearest
            } else {
                self.pins[i] = None;
                continue;
            };
            let caps = link.param(pin.nid, param::WIDGET_CAPS)?;
            let found = search(link, pin.nid, caps, first, last)?;
            pin.path = path_to(link, &found, pin.nid, dac)?;
            pin.path.volume = self.volume_amp(link, &pin.path)?;
            if !self.dacs().contains(&dac) && self.dac_count < MAX_PINS {
                self.dacs[self.dac_count] = dac;
                self.dac_count += 1;
            }
            self.pins[i] = Some(pin);
        }
        Ok(())
    }

    /// The amplifier with steps nearest the converter on `path`.
    fn volume_amp(&self, link: &mut Link<'_>, path: &Path) -> Result<Option<(u8, AmpCaps)>, Error> {
        for hop in path.hops[..path.len].iter().rev() {
            if hop.caps & wcap::OUT_AMP == 0 {
                continue;
            }
            let caps = self.out_amp(link, *hop)?;
            if caps.steps() > 0 {
                return Ok(Some((hop.nid, caps)));
            }
        }
        Ok(None)
    }

    /// A widget's output amplifier's capabilities.
    fn out_amp(&self, link: &mut Link<'_>, hop: Hop) -> Result<AmpCaps, Error> {
        Ok(if hop.caps & wcap::AMP_OVERRIDE != 0 {
            AmpCaps(link.param(hop.nid, param::OUT_AMP_CAPS)?)
        } else {
            self.afg_out_amp
        })
    }

    /// A widget's input amplifiers' capabilities.
    fn in_amp(&self, link: &mut Link<'_>, hop: Hop) -> Result<AmpCaps, Error> {
        Ok(if hop.caps & wcap::AMP_OVERRIDE != 0 {
            AmpCaps(link.param(hop.nid, param::IN_AMP_CAPS)?)
        } else {
            self.afg_in_amp
        })
    }

    /// The rates and channels every converter takes.
    fn read_formats(&mut self, link: &mut Link<'_>) -> Result<(), Error> {
        let mut rates = (1u32 << verb::PCM_RATES.len()) - 1;
        let mut stereo = true;
        for i in 0..self.dac_count {
            let dac = self.dacs[i];
            let caps = link.param(dac, param::WIDGET_CAPS)?;
            let pcm = if caps & wcap::FORMAT_OVERRIDE != 0 {
                link.param(dac, param::PCM)?
            } else {
                self.afg_pcm
            };
            if pcm & verb::PCM_16BIT == 0 {
                rates = 0;
            }
            rates &= pcm;
            stereo &= caps & wcap::STEREO != 0;
        }
        self.rates = rates & ((1 << verb::PCM_RATES.len()) - 1);
        self.stereo = stereo;
        let every = self.pins().all(|pin| pin.path.volume.is_some());
        self.hardware_volume = every;
        Ok(())
    }

    /// Routes every pin's way: powers each widget up, chooses its input,
    /// sets its amplifiers, and turns the pin's external amplifier on.
    fn route(&self, link: &mut Link<'_>) -> Result<(), Error> {
        for pin in self.pins() {
            let path = &pin.path;
            for (i, hop) in path.hops[..path.len].iter().enumerate() {
                if hop.caps & wcap::POWER != 0 {
                    link.verb(hop.nid, verb::SET_POWER_STATE, 0)?;
                }
                let last = i + 1 == path.len;
                let kind = wcap::kind(hop.caps);
                if !last && kind != wcap::MIXER && hop.connections > 1 {
                    link.verb(hop.nid, verb::SET_CONNECTION_SELECT, hop.select)?;
                }
                if !last && hop.caps & wcap::IN_AMP != 0 {
                    match kind {
                        wcap::MIXER => self.route_mixer(link, *hop)?,
                        wcap::SELECTOR => {
                            let caps = self.in_amp(link, *hop)?;
                            set_in_amp(
                                link,
                                hop.nid,
                                hop.select,
                                Some(caps.offset().min(caps.steps())),
                            )?;
                        }
                        _ => {}
                    }
                }
                if hop.caps & wcap::OUT_AMP != 0 && path.volume.map(|(nid, _)| nid) != Some(hop.nid)
                {
                    let caps = self.out_amp(link, *hop)?;
                    set_out_amp(link, hop.nid, Some(caps.offset().min(caps.steps())), None)?;
                }
            }
            if pin.caps & pincap::EAPD != 0 {
                link.verb(pin.nid, verb::SET_EAPD, verb::EAPD)?;
            }
        }
        for &dac in self.dacs() {
            link.verb(dac, verb::SET_STREAM_CHANNEL, crate::stream::TAG << 4)?;
        }
        Ok(())
    }

    /// Unmutes the inputs of mixer `hop` that some way passes through,
    /// at 0 dB, and mutes its others.
    fn route_mixer(&self, link: &mut Link<'_>, hop: Hop) -> Result<(), Error> {
        let caps = self.in_amp(link, hop)?;
        let gain = caps.offset().min(caps.steps());
        for input in 0..hop.connections.min(16) {
            let wanted = self.pins().any(|pin| {
                let path = &pin.path;
                path.hops[..path.len.saturating_sub(1)]
                    .iter()
                    .any(|other| other.nid == hop.nid && other.select == input)
            });
            set_in_amp(link, hop.nid, input, wanted.then_some(gain))?;
        }
        Ok(())
    }

    /// Sets the converters to take `format` (a format word).
    pub fn set_format(&self, link: &mut Link<'_>, format: u16) -> Result<(), Error> {
        for &dac in self.dacs() {
            link.verb4(dac, verb::SET_FORMAT, format)?;
        }
        Ok(())
    }

    /// Sets the volume of every way to `left` and `right` percent, 0 to
    /// 100, as decibels below the 0 dB step: the cube of the fraction, as
    /// the kernel's own scaling. False if a way has no amplifier for it.
    pub fn set_volume(&self, link: &mut Link<'_>, left: u8, right: u8) -> Result<bool, Error> {
        if !self.hardware_volume {
            return Ok(false);
        }
        for pin in self.pins() {
            let Some((nid, caps)) = pin.path.volume else { continue };
            set_out_amp(link, nid, step(caps, left), Some((step(caps, right), amp::RIGHT)))?;
        }
        Ok(true)
    }

    /// Reads the jacks that sense and turns each pin on or off: any
    /// headphones turn off the speakers and line outs, and a line out with
    /// something in it the speakers. Returns whether what plays changed.
    pub fn refresh(&mut self, link: &mut Link<'_>) -> Result<bool, Error> {
        for pin in self.pins.iter_mut().flatten() {
            if pin.sensed {
                let sense = link.verb(pin.nid, verb::GET_PIN_SENSE, 0)?;
                pin.present = sense & verb::SENSE_PRESENT != 0;
            }
        }
        let plugged = |role: Role, pins: &[Option<Pin>]| {
            pins.iter().flatten().any(|pin| pin.role == role && pin.sensed && pin.present)
        };
        let headphones = plugged(Role::Headphones, &self.pins);
        let line = plugged(Role::Line, &self.pins);
        let mut changed = false;
        for pin in self.pins.iter_mut().flatten() {
            let on = match pin.role {
                Role::Headphones => pin.present,
                Role::Line => !headphones,
                Role::Speaker => !headphones && !line,
            };
            if on != pin.on || !pin.set {
                let mut control = 0;
                if on {
                    control = verb::PIN_OUT;
                    if pin.role == Role::Headphones && pin.caps & pincap::HP != 0 {
                        control |= verb::PIN_HP;
                    }
                }
                link.verb(pin.nid, verb::SET_PIN_CONTROL, control)?;
                changed |= on != pin.on;
                pin.on = on;
                pin.set = true;
            }
        }
        Ok(changed)
    }
}

/// What the firmware says pin of `config` plays to, if it is one of the
/// outputs the driver plays through.
fn role_of(config: Config) -> Option<Role> {
    match (config.connectivity(), config.device()) {
        (verb::CONN_NONE, _) => None,
        (_, verb::DEVICE_SPEAKER) => Some(Role::Speaker),
        (_, verb::DEVICE_HP_OUT) => Some(Role::Headphones),
        // A line out built in is a speaker by another name.
        (verb::CONN_FIXED, verb::DEVICE_LINE_OUT) => Some(Role::Speaker),
        (_, verb::DEVICE_LINE_OUT) => Some(Role::Line),
        _ => None,
    }
}

/// Powers the function group up (D0) and waits, up to 100 ms, until it
/// says it is.
fn power_up(link: &mut Link<'_>, afg: u8) -> Result<(), Error> {
    link.verb(afg, verb::SET_POWER_STATE, 0)?;
    for _ in 0..100 {
        if (link.verb(afg, verb::GET_POWER_STATE, 0)? >> 4) & 0xf == 0 {
            return Ok(());
        }
        link.delay_us(1000);
    }
    Ok(())
}

/// Sets input `input`'s amplifier of widget `nid`, both channels: to
/// step `gain`, or muted.
fn set_in_amp(link: &mut Link<'_>, nid: u8, input: u8, gain: Option<u8>) -> Result<(), Error> {
    let level = gain.map_or(amp::MUTE, |gain| u16::from(gain & 0x7f));
    let payload = amp::INPUT | amp::LEFT | amp::RIGHT | amp::index(input) | level;
    link.verb4(nid, verb::SET_AMP, payload)?;
    Ok(())
}

/// Sets widget `nid`'s output amplifier: the left channel (and the right,
/// unless `right` gives it apart) to step `gain`, or muted.
fn set_out_amp(
    link: &mut Link<'_>,
    nid: u8,
    gain: Option<u8>,
    right: Option<(Option<u8>, u16)>,
) -> Result<(), Error> {
    let level = |gain: Option<u8>| gain.map_or(amp::MUTE, |gain| u16::from(gain & 0x7f));
    let both = if right.is_some() { amp::LEFT } else { amp::LEFT | amp::RIGHT };
    link.verb4(nid, verb::SET_AMP, amp::OUTPUT | both | level(gain))?;
    if let Some((gain, channel)) = right {
        link.verb4(nid, verb::SET_AMP, amp::OUTPUT | channel | level(gain))?;
    }
    Ok(())
}

/// The step of an amplifier of `caps` for `percent` volume: as many steps
/// below the 0 dB one as the attenuation needs (radix-audio's), rounded, and never above
/// it. `None`, muted, for 0 when the amplifier can mute; the lowest step
/// when it cannot.
pub fn step(caps: AmpCaps, percent: u8) -> Option<u8> {
    let top = caps.offset().min(caps.steps());
    if percent == 0 {
        return if caps.mute() { None } else { Some(0) };
    }
    let quarter = caps.quarter_db();
    let attenuation = u32::from(radix_audio::attenuation(percent));
    let down = (attenuation + quarter / 2) / quarter;
    Some(u32::from(top).saturating_sub(down) as u8)
}
