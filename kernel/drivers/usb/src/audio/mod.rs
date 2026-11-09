// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB sound outputs: speakers, headsets and DACs of the *USB Device
//! Class Definition for Audio Devices*, releases 1.0 and 2.0 (UAC1 and
//! UAC2; Phase 14, ADR 0323).
//!
//! An audio function is an AudioControl interface, whose class-specific
//! descriptors are a graph of *entities* (terminals where sound enters or
//! leaves the function, units that mix, select or change it, and in UAC2
//! the clocks), and AudioStreaming interfaces, whose alternate settings
//! each carry one format on an isochronous endpoint (setting 0 has none,
//! and takes no bandwidth).
//!
//! - [`find`] reads a configuration for an output: a streaming interface
//!   whose isochronous OUT endpoint feeds a USB streaming input terminal
//!   that a path of units leads from to an output terminal (a speaker,
//!   headphones, a headset, a line out), and the setting of it that plays
//!   signed PCM in two channels (or one) in the fewest bytes a sample. On
//!   the way it notes the feature unit nearest the output, whose volume
//!   and mute the kernel sets, and in UAC2 the clock source behind the
//!   input terminal, whose rate it sets.
//! - [`request`] makes the class's requests: the rate (UAC1 to the
//!   endpoint, UAC2 to the clock), the rates a UAC2 clock takes, a
//!   feature unit's volume range, volume and mute; [`Range`] and
//!   [`level`] turn the kernel's volume into the unit's decibels.
//! - [`Pacer`] sizes each service interval's packet: the rate's frames,
//!   with the fraction carried, so 44.1 kHz at full speed is 44 frames a
//!   millisecond with a 45 every tenth. An asynchronous endpoint's device
//!   says through its feedback endpoint how fast it really plays, which
//!   the pacer follows.
//! - [`Stream`] keeps the ring of samples the controller reads the
//!   packets from: what the kernel wrote into it, what is queued as TDs
//!   and what they have played. It cuts each packet's bytes into the
//!   pieces a TD takes, where the ring wraps and where 64 KiB of memory
//!   end, and keeps the transfer ring from overfilling.
//!
//! The specifications are the USB Implementers Forum's; FreeBSD's
//! uaudio(4) (BSD-2-Clause) was read for what devices do that the
//! specifications do not say: feedback in the other speed's format, and
//! clock selectors. No code is taken from it, or from Linux's.

use crate::{
    CLASS, Configuration, DEVICE_TO_HOST, EndpointDescriptor, Interface, Setup, TO_ENDPOINT,
    TO_INTERFACE, TransferType, class, descriptor,
};

#[cfg(test)]
mod tests;

/// The AudioControl interface's subclass.
pub const SUBCLASS_CONTROL: u8 = 1;
/// An AudioStreaming interface's subclass.
pub const SUBCLASS_STREAMING: u8 = 2;
/// A UAC2 function's interfaces' protocol (IP_VERSION_02_00).
const PROTOCOL_V2: u8 = 0x20;
/// A class-specific interface or endpoint descriptor.
const CS_INTERFACE: u8 = 0x24;
const CS_ENDPOINT: u8 = 0x25;
/// An interface association descriptor (USB 2.0 ECN), which says which
/// interfaces a UAC2 function has.
const INTERFACE_ASSOCIATION: u8 = 0x0b;

/// The AudioControl interface's descriptors' subtypes (UAC1 table A-5,
/// UAC2 table A-9).
mod ac {
    pub const HEADER: u8 = 0x01;
    pub const INPUT_TERMINAL: u8 = 0x02;
    pub const OUTPUT_TERMINAL: u8 = 0x03;
    pub const MIXER_UNIT: u8 = 0x04;
    pub const SELECTOR_UNIT: u8 = 0x05;
    pub const FEATURE_UNIT: u8 = 0x06;
    /// UAC1's processing unit, UAC2's effect unit.
    pub const UNIT_07: u8 = 0x07;
    /// UAC1's extension unit, UAC2's processing unit.
    pub const UNIT_08: u8 = 0x08;
    /// UAC2's extension unit.
    pub const EXTENSION_V2: u8 = 0x09;
    pub const CLOCK_SOURCE: u8 = 0x0a;
    pub const CLOCK_SELECTOR: u8 = 0x0b;
    pub const CLOCK_MULTIPLIER: u8 = 0x0c;
}

/// An AudioStreaming interface's descriptors' subtypes, and its data
/// endpoint's (both 1).
mod streaming {
    pub const GENERAL: u8 = 0x01;
    pub const FORMAT_TYPE: u8 = 0x02;
}

/// A format of type I: samples one after another, a frame of a sample
/// for each channel (UAC Formats, 2.2).
const FORMAT_TYPE_I: u8 = 1;
/// UAC1's wFormatTag for PCM.
const PCM_TAG: u16 = 0x0001;
/// UAC2's bmFormats bit for PCM.
const PCM_BIT: u32 = 1;
/// The terminal type of a USB streaming terminal (UAC Terminal Types,
/// 2.1); input terminals' types are 0x02xx, output terminals' 0x03xx,
/// and bidirectional, telephony, external and embedded ones 0x04xx to
/// 0x07xx.
const USB_STREAMING: u16 = 0x0101;

/// The most rates of a UAC1 setting's list that are kept.
pub const MAX_RATES: usize = 8;
/// The most steps a path from an output terminal back to its input
/// terminal, or from a terminal to its clock source, is followed for.
const MAX_DEPTH: usize = 16;

/// Which release of the class the function follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// Release 1.0 (1998): full speed, rates set on the endpoint.
    One,
    /// Release 2.0 (2006): high speed too, rates set on clocks.
    Two,
}

/// How an isochronous endpoint keeps time with the device (USB 2.0,
/// 5.12.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    /// It does not say.
    None,
    /// The device keeps its own clock, and says how fast it plays
    /// through a feedback endpoint.
    Asynchronous,
    /// The device follows the rate the host sends at.
    Adaptive,
    /// The device's clock follows the bus's frames.
    Synchronous,
}

/// What the output is, from its output terminal's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A speaker, of any sort, or a speakerphone.
    Speaker,
    /// Headphones.
    Headphones,
    /// A headset: headphones with a microphone.
    Headset,
    /// A line out, to an amplifier or another machine.
    Line,
    /// A digital out: S/PDIF and the like.
    Digital,
    /// Anything else.
    Other,
}

impl Kind {
    /// The kind of terminal type `terminal`.
    fn of(terminal: u16) -> Kind {
        match terminal {
            // Speakers of every sort, and speakerphones.
            0x0301 | 0x0304..=0x0307 | 0x0403..=0x0405 => Kind::Speaker,
            0x0302 => Kind::Headphones,
            0x0402 => Kind::Headset,
            0x0603 => Kind::Line,
            0x0602 | 0x0605..=0x0607 => Kind::Digital,
            _ => Kind::Other,
        }
    }

    /// In words, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Speaker => "speaker",
            Kind::Headphones => "headphones",
            Kind::Headset => "headset",
            Kind::Line => "line out",
            Kind::Digital => "digital out",
            Kind::Other => "output",
        }
    }
}

/// The rates a setting plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rates {
    /// The first `count` of `rates`.
    List {
        /// The rates, in frames a second.
        rates: [u32; MAX_RATES],
        /// How many of them count.
        count: u8,
    },
    /// Any from the first to the second.
    Range(u32, u32),
    /// Those of its clock, which [`request::rate_ranges`] asks (UAC2).
    Clock,
}

impl Rates {
    /// Whether `rate` is among them; for [`Rates::Clock`], always (the
    /// clock's answer says).
    pub fn plays(&self, rate: u32) -> bool {
        match *self {
            Rates::List { rates, count } => rates[..usize::from(count)].contains(&rate),
            Rates::Range(low, high) => (low..=high).contains(&rate),
            Rates::Clock => true,
        }
    }
}

/// An alternate setting of the streaming interface: one format on its
/// isochronous OUT endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    /// bAlternateSetting.
    pub alternate: u8,
    /// The channels in a frame.
    pub channels: u8,
    /// The bytes each sample takes (bSubframeSize, bSubslotSize): 2 to 4.
    pub subframe: u8,
    /// The bits of each that count, the highest (bBitResolution).
    pub bits: u8,
    /// The rates it plays.
    pub rates: Rates,
    /// The endpoint that takes the samples, with what its SuperSpeed
    /// companion says.
    pub endpoint: EndpointDescriptor,
    /// How that endpoint keeps time.
    pub sync: Sync,
    /// The endpoint the device says its rate on, for an asynchronous one
    /// that has one.
    pub feedback: Option<EndpointDescriptor>,
    /// UAC1: the endpoint takes its rate (its Sampling Frequency
    /// Control).
    pub rate_control: bool,
}

impl Setting {
    /// The bytes of a frame.
    pub fn frame_bytes(&self) -> u32 {
        u32::from(self.channels) * u32::from(self.subframe)
    }
}

/// The feature unit nearest the output: its volume and mute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feature {
    /// Its bUnitID.
    pub unit: u8,
    /// The channels with a mute the host may set, a bit each, bit 0 the
    /// master.
    pub mute: u16,
    /// The channels with a volume the host may set.
    pub volume: u16,
}

impl Feature {
    /// The channels whose volume the kernel sets, of the first
    /// `channels`: the master alone if it has one, else each that has
    /// its own.
    pub fn volume_channels(&self, channels: u8) -> impl Iterator<Item = u8> + use<> {
        controlled(self.volume, channels)
    }

    /// The channels the kernel mutes, the same way.
    pub fn mute_channels(&self, channels: u8) -> impl Iterator<Item = u8> + use<> {
        controlled(self.mute, channels)
    }
}

/// The channels of `mask` the kernel sets, of the first `channels`: the
/// master (0) alone if it is there, else the channels from 1.
fn controlled(mask: u16, channels: u8) -> impl Iterator<Item = u8> + use<> {
    let wanted = if mask & 1 != 0 {
        1
    } else {
        // Bits 1 to `channels`.
        let below = (1u32 << (u32::from(channels).min(15) + 1)) - 2;
        mask & below as u16
    };
    (0..16u8).filter(move |&c| wanted & (1 << c) != 0)
}

/// The UAC2 clock behind the output's input terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// The clock source's bClockID, whose rate is set.
    pub source: u8,
    /// Whether its rate can be set (its Clock Frequency Control is
    /// programmable).
    pub programmable: bool,
    /// The clock selector on the way to it, if any: it is set to its
    /// first input, which the source is behind.
    pub selector: Option<u8>,
}

/// An output of an audio function, as [`find`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    /// The release of the class the function follows.
    pub version: Version,
    /// The AudioControl interface's number: what requests to its
    /// entities name.
    pub control: u8,
    /// The AudioStreaming interface's.
    pub streaming: u8,
    /// Its setting that is played through.
    pub setting: Setting,
    /// What the output terminal it leads to is.
    pub kind: Kind,
    /// The feature unit nearest that terminal, if one on the way has a
    /// volume or a mute.
    pub feature: Option<Feature>,
    /// UAC2: the clock whose rate is set.
    pub clock: Option<Clock>,
}

/// The first output of the audio functions of `config`, if it has one.
pub fn find(config: &Configuration<'_>) -> Option<Output> {
    let audio = |subclass: u8| {
        move |i: &Interface<'_>| {
            i.descriptor.class == class::AUDIO && i.descriptor.subclass == subclass
        }
    };
    let controls =
        config.interfaces().filter(audio(SUBCLASS_CONTROL)).filter(|i| i.descriptor.alternate == 0);
    for control in controls {
        let entities = Entities { control: control.clone() };
        let version = version(control.descriptor.protocol, &entities);
        let number = control.descriptor.number;
        let found = config
            .interfaces()
            .filter(audio(SUBCLASS_STREAMING))
            .filter(|i| i.descriptor.alternate > 0 && belongs(config, &entities, number, i))
            .filter_map(|i| {
                let setting = read_setting(&i, version)?;
                let link = terminal_link(&i)?;
                let (kind, feature) = path_to(&entities, version, link)?;
                Some((i.descriptor.number, setting, link, kind, feature))
            })
            .min_by_key(|&(_, setting, ..)| preference(&setting));
        let Some((streaming, setting, link, kind, feature)) = found else { continue };
        let clock = match version {
            Version::One => None,
            Version::Two => clock_of(&entities, link),
        };
        return Some(Output { version, control: number, streaming, setting, kind, feature, clock });
    }
    None
}

/// Which release a function's AudioControl interface, of `protocol`,
/// follows: UAC2 if its protocol or its header's bcdADC says so.
fn version(protocol: u8, entities: &Entities<'_>) -> Version {
    let adc = entities.header().map_or(0x0100, |d| word(d, 3));
    if protocol == PROTOCOL_V2 || adc >= 0x0200 { Version::Two } else { Version::One }
}

/// Whether streaming interface `streaming` is of the function whose
/// AudioControl interface is `control`: among those a UAC1 header lists,
/// or those the interface association that holds the control interface
/// says. A function that says neither has them all.
fn belongs(
    config: &Configuration<'_>,
    entities: &Entities<'_>,
    control: u8,
    streaming: &Interface<'_>,
) -> bool {
    let number = streaming.descriptor.number;
    // UAC1's header: bInCollection, then the interfaces' numbers. UAC2's
    // has no list (its eighth byte is bmControls), so its protocol says
    // which it is.
    if let Some(header) = entities.header()
        && entities.control.descriptor.protocol != PROTOCOL_V2
        && word(header, 3) < 0x0200
        && byte(header, 7) > 0
    {
        let count = usize::from(byte(header, 7));
        return header.get(8..8 + count).is_some_and(|list| list.contains(&number));
    }
    let association = config.descriptors().find(|&(kind, d)| {
        kind == INTERFACE_ASSOCIATION
            && d.len() >= 4
            && (d[2]..d[2].saturating_add(d[3])).contains(&control)
    });
    association.is_none_or(|(_, d)| (d[2]..d[2].saturating_add(d[3])).contains(&number))
}

/// How much a setting is wanted, least first: two channels, then one;
/// two bytes a sample, then three, then four; 48 kHz among its rates.
fn preference(setting: &Setting) -> (u8, u8, bool) {
    (u8::from(setting.channels != 2), setting.subframe, !setting.rates.plays(48000))
}

/// The class-specific descriptors of an AudioControl interface: its
/// header and its entities.
struct Entities<'a> {
    control: Interface<'a>,
}

impl<'a> Entities<'a> {
    /// Each class-specific descriptor of four bytes or more.
    fn all(&self) -> impl Iterator<Item = &'a [u8]> + use<'a> {
        self.control
            .descriptors()
            .filter(|&(kind, d)| kind == CS_INTERFACE && d.len() >= 4)
            .map(|(_, d)| d)
    }

    /// The header.
    fn header(&self) -> Option<&'a [u8]> {
        self.all().find(|d| d[2] == ac::HEADER)
    }

    /// Entity `id`'s descriptor (its ID is its fourth byte).
    fn get(&self, id: u8) -> Option<&'a [u8]> {
        self.all().find(|d| d[2] != ac::HEADER && d[3] == id)
    }
}

/// Byte `at` of `d`, or 0 past its end.
fn byte(d: &[u8], at: usize) -> u8 {
    d.get(at).copied().unwrap_or(0)
}

/// The little-endian u16 at `at` of `d`, 0 past its end.
fn word(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([byte(d, at), byte(d, at + 1)])
}

/// The entities the sound of entity `d`, a unit or an output terminal,
/// comes from.
fn sources(d: &[u8], version: Version) -> &[u8] {
    // One source at `at`.
    let one = |at: usize| d.get(at..at + 1).unwrap_or(&[]);
    // bNrInPins at `at`, then that many sources.
    let list = |at: usize| {
        let count = usize::from(byte(d, at));
        d.get(at + 1..at + 1 + count).unwrap_or(&[])
    };
    match (d[2], version) {
        (ac::OUTPUT_TERMINAL, _) => one(7),
        (ac::FEATURE_UNIT, _) => one(4),
        (ac::MIXER_UNIT | ac::SELECTOR_UNIT, _) => list(4),
        // UAC2's effect unit has one source after its type.
        (ac::UNIT_07, Version::Two) => one(6),
        // Processing and extension units: a type, then their sources.
        (ac::UNIT_07 | ac::UNIT_08, Version::One)
        | (ac::UNIT_08 | ac::EXTENSION_V2, Version::Two) => list(6),
        _ => &[],
    }
}

/// The output terminal a path of units leads to from the USB streaming
/// input terminal `link`, and the feature unit nearest it on the way.
fn path_to(entities: &Entities<'_>, version: Version, link: u8) -> Option<(Kind, Option<Feature>)> {
    let input = entities.get(link)?;
    if input[2] != ac::INPUT_TERMINAL || word(input, 4) != USB_STREAMING {
        return None;
    }
    entities.all().filter(|d| d[2] == ac::OUTPUT_TERMINAL && d.len() >= 8).find_map(|output| {
        let terminal = word(output, 4);
        // An output terminal that is a USB stream (0x01xx) is a
        // recording's.
        if terminal >> 8 < 0x03 {
            return None;
        }
        let feature = search(entities, version, output, link, 0)?;
        Some((Kind::of(terminal), feature))
    })
}

/// Whether the sound of entity `d` comes, through units, from entity
/// `link`: Some, with the feature unit nearest `d` on the way if there is
/// one.
fn search(
    entities: &Entities<'_>,
    version: Version,
    d: &[u8],
    link: u8,
    depth: usize,
) -> Option<Option<Feature>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let here = if d[2] == ac::FEATURE_UNIT { feature(d, version) } else { None };
    for &source in sources(d, version) {
        if source == link {
            return Some(here);
        }
        let Some(next) = entities.get(source) else { continue };
        if let Some(further) = search(entities, version, next, link, depth + 1) {
            return Some(here.or(further));
        }
    }
    None
}

/// What feature unit `d` lets the host set, if it has a volume or a mute.
fn feature(d: &[u8], version: Version) -> Option<Feature> {
    // Each channel's controls, the master's first: UAC1's of bControlSize
    // bytes, a bit each (mute D0, volume D1); UAC2's of four, two bits
    // each, both set when the host may change it (mute D1..0, volume
    // D3..2).
    let (start, size, mute_bits, volume_bits) = match version {
        Version::One => (6, usize::from(byte(d, 5)), 0b1, 0b10),
        Version::Two => (5, 4, 0b11, 0b1100),
    };
    if size == 0 {
        return None;
    }
    // The controls run to the last byte, iFeature.
    let count = (d.len().saturating_sub(start + 1) / size).min(16);
    let mut feature = Feature { unit: d[3], mute: 0, volume: 0 };
    for channel in 0..count {
        let at = start + channel * size;
        let bytes = d.get(at..at + size.min(4)).unwrap_or(&[]);
        let controls = bytes.iter().rev().fold(0u32, |all, &b| all << 8 | u32::from(b));
        if controls & mute_bits == mute_bits {
            feature.mute |= 1 << channel;
        }
        if controls & volume_bits == volume_bits {
            feature.volume |= 1 << channel;
        }
    }
    (feature.mute != 0 || feature.volume != 0).then_some(feature)
}

/// The UAC2 clock source behind input terminal `link`, through
/// selectors (their first input) and multipliers.
fn clock_of(entities: &Entities<'_>, link: u8) -> Option<Clock> {
    let mut id = byte(entities.get(link)?, 7);
    let mut selector = None;
    for _ in 0..MAX_DEPTH {
        let d = entities.get(id)?;
        match d[2] {
            ac::CLOCK_SOURCE => {
                let programmable = byte(d, 5) & 3 == 3;
                return Some(Clock { source: id, programmable, selector });
            }
            ac::CLOCK_SELECTOR if byte(d, 4) > 0 => {
                selector = selector.or(Some(id));
                id = byte(d, 5);
            }
            ac::CLOCK_MULTIPLIER => id = byte(d, 4),
            _ => return None,
        }
    }
    None
}

/// The input terminal streaming interface `interface` feeds.
fn terminal_link(interface: &Interface<'_>) -> Option<u8> {
    interface
        .descriptors()
        .find(|&(kind, d)| kind == CS_INTERFACE && d.len() >= 4 && d[2] == streaming::GENERAL)
        .map(|(_, d)| d[3])
}

/// The setting `interface` is, if it is one the kernel plays through:
/// PCM of type I, in one or two channels of 16 to 32 bits, on an
/// isochronous OUT endpoint.
fn read_setting(interface: &Interface<'_>, version: Version) -> Option<Setting> {
    let class_descriptor = |subtype: u8| {
        interface
            .descriptors()
            .find(|&(kind, d)| kind == CS_INTERFACE && d.len() >= 4 && d[2] == subtype)
            .map(|(_, d)| d)
    };
    let general = class_descriptor(streaming::GENERAL)?;
    let format = class_descriptor(streaming::FORMAT_TYPE)?;
    if byte(format, 3) != FORMAT_TYPE_I {
        return None;
    }
    let (channels, subframe, bits, rates) = match version {
        Version::One => {
            if word(general, 5) != PCM_TAG {
                return None;
            }
            (byte(format, 4), byte(format, 5), byte(format, 6), uac1_rates(format)?)
        }
        Version::Two => {
            let formats = u32::from(word(general, 6)) | u32::from(word(general, 8)) << 16;
            if byte(general, 5) != FORMAT_TYPE_I || formats & PCM_BIT == 0 {
                return None;
            }
            (byte(general, 10), byte(format, 4), byte(format, 5), Rates::Clock)
        }
    };
    if !(1..=2).contains(&channels)
        || !(2..=4).contains(&subframe)
        || bits < 16
        || u32::from(bits) > u32::from(subframe) * 8
    {
        return None;
    }

    // The endpoints: the isochronous OUT one that is not feedback (bits
    // 5..4 of its attributes, its usage, 01), and the feedback one, which
    // UAC2 marks by its usage and UAC1 names in the data endpoint's
    // bSynchAddress (its ninth byte).
    let usage = |e: &EndpointDescriptor| (e.attributes >> 4) & 3;
    let isochronous =
        || interface.endpoints().filter(|e| e.transfer_type() == TransferType::Isochronous);
    let endpoint = isochronous().find(|e| !e.is_in() && usage(e) != 1)?;
    let mut after = interface
        .descriptors()
        .skip_while(|&(kind, d)| !(kind == descriptor::ENDPOINT && byte(d, 2) == endpoint.address));
    let synch_address = after.next().map_or(0, |(_, d)| byte(d, 8));
    // The class's descriptor of the endpoint follows it (and its
    // companion): bmAttributes D0 is the Sampling Frequency Control.
    let rate_control = version == Version::One
        && after.take_while(|&(kind, _)| kind != descriptor::ENDPOINT).any(|(kind, d)| {
            kind == CS_ENDPOINT && byte(d, 2) == streaming::GENERAL && byte(d, 3) & 1 != 0
        });
    let sync = match (endpoint.attributes >> 2) & 3 {
        1 => Sync::Asynchronous,
        2 => Sync::Adaptive,
        3 => Sync::Synchronous,
        _ => Sync::None,
    };
    let feedback = isochronous()
        .filter(EndpointDescriptor::is_in)
        .find(|e| usage(e) == 1 || (synch_address != 0 && e.address == synch_address))
        .filter(|_| sync == Sync::Asynchronous);
    Some(Setting {
        alternate: interface.descriptor.alternate,
        channels,
        subframe,
        bits,
        rates,
        endpoint,
        sync,
        feedback,
        rate_control,
    })
}

/// The rates of a UAC1 Type I format descriptor: its list, or its range
/// when bSamFreqType is 0.
fn uac1_rates(d: &[u8]) -> Option<Rates> {
    let rate = |i: usize| {
        let at = 8 + 3 * i;
        Some(u32::from_le_bytes([*d.get(at)?, *d.get(at + 1)?, *d.get(at + 2)?, 0]))
    };
    let kinds = usize::from(byte(d, 7));
    if kinds == 0 {
        return Some(Rates::Range(rate(0)?, rate(1)?));
    }
    let mut rates = [0; MAX_RATES];
    let mut count = 0;
    for r in (0..kinds).map_while(rate).take(MAX_RATES) {
        rates[count] = r;
        count += 1;
    }
    (count > 0).then_some(Rates::List { rates, count: count as u8 })
}

/// The range of a feature unit's volume, in 1/256 dB (UAC1 5.2.2.4.3.2,
/// UAC2 5.2.5.7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// The lowest level.
    pub min: i16,
    /// The highest.
    pub max: i16,
    /// The step between levels.
    pub resolution: i16,
}

impl Range {
    /// The range a UAC2 RANGE answer holds: wNumSubRanges, then each
    /// subrange's minimum, maximum and resolution, two bytes each. The
    /// lowest minimum, the highest maximum and the first resolution; None
    /// if it holds none.
    pub fn parse_v2(bytes: &[u8]) -> Option<Range> {
        let count = usize::from(word(bytes, 0));
        let mut range: Option<Range> = None;
        for s in bytes.get(2..)?.as_chunks::<6>().0.iter().take(count) {
            let field = |at: usize| i16::from_le_bytes([s[at], s[at + 1]]);
            let (min, max, resolution) = (field(0), field(2), field(4));
            range = Some(match range {
                None => Range { min, max, resolution },
                Some(r) => Range { min: r.min.min(min), max: r.max.max(max), ..r },
            });
        }
        range.filter(|r| r.min <= r.max)
    }
}

/// The level, in 1/256 dB, that a volume `attenuation` (in 1/256 dB)
/// below full makes in `range`. Full is 0 dB, or the range's top if it
/// ends lower; the level is on the range's steps, the nearest one that is
/// not louder than full, and never below its bottom.
pub fn level(range: Range, attenuation: u32) -> i16 {
    let (min, max) = (i32::from(range.min), i32::from(range.max));
    let top = max.min(0).max(min);
    let wanted = (top - attenuation.min(0x8000) as i32).max(min);
    let step = i32::from(range.resolution).max(1);
    // The nearest step from the bottom, then one down if that is louder
    // than full.
    let mut level = min + (wanted - min + step / 2) / step * step;
    if level > top {
        level -= step;
    }
    level.clamp(min, max) as i16
}

/// The class's requests.
pub mod request {
    use super::*;

    /// UAC1 (A.9): sets a control's value.
    pub const SET_CUR: u8 = 0x01;
    /// UAC1: asks a control's value.
    pub const GET_CUR: u8 = 0x81;
    /// UAC1: asks the lowest value a control takes.
    pub const GET_MIN: u8 = 0x82;
    /// UAC1: asks the highest.
    pub const GET_MAX: u8 = 0x83;
    /// UAC1: asks the step between them.
    pub const GET_RES: u8 = 0x84;
    /// UAC2 (A.14): a control's value, set or asked.
    pub const CUR: u8 = 0x01;
    /// UAC2: the ranges of values a control takes.
    pub const RANGE: u8 = 0x02;

    /// A feature unit's mute (UAC1 A.10.2, UAC2 A.17.7).
    pub const MUTE: u8 = 0x01;
    /// A feature unit's volume.
    pub const VOLUME: u8 = 0x02;
    /// UAC1's endpoint control of the rate and UAC2's clock source's
    /// (A.10.3, A.17.1).
    pub const SAMPLING_FREQ: u8 = 0x01;
    /// UAC2's clock selector's input (A.17.2).
    pub const CLOCK_SELECTOR: u8 = 0x01;

    /// The bytes [`rate_ranges`] asks for: room for 16 subranges.
    pub const RATE_RANGES_LENGTH: u16 = 2 + 12 * 16;
    /// The bytes [`volume_range`] asks for: room for 16 subranges.
    pub const VOLUME_RANGE_LENGTH: u16 = 2 + 6 * 16;

    /// A class request to entity `entity` of AudioControl interface
    /// `interface`: control `control` of channel `channel`.
    fn to_entity(
        into_host: bool,
        request: u8,
        (interface, entity): (u8, u8),
        (control, channel): (u8, u8),
        length: u16,
    ) -> Setup {
        Setup {
            request_type: if into_host { DEVICE_TO_HOST } else { 0 } | CLASS | TO_INTERFACE,
            request,
            value: u16::from(control) << 8 | u16::from(channel),
            index: u16::from(entity) << 8 | u16::from(interface),
            length,
        }
    }

    /// UAC1: sets the rate of the endpoint at `address`; the three bytes
    /// are the data.
    pub fn set_rate_v1(address: u8, rate: u32) -> (Setup, [u8; 3]) {
        let setup = Setup {
            request_type: CLASS | TO_ENDPOINT,
            request: SET_CUR,
            value: u16::from(SAMPLING_FREQ) << 8,
            index: u16::from(address),
            length: 3,
        };
        let [a, b, c, _] = rate.to_le_bytes();
        (setup, [a, b, c])
    }

    /// UAC2: sets the rate of clock source `clock`; the four bytes are
    /// the data.
    pub fn set_rate_v2(interface: u8, clock: u8, rate: u32) -> (Setup, [u8; 4]) {
        let setup = to_entity(false, CUR, (interface, clock), (SAMPLING_FREQ, 0), 4);
        (setup, rate.to_le_bytes())
    }

    /// UAC2: asks the rates clock source `clock` takes; [`ranges_hold`]
    /// reads the answer.
    pub fn rate_ranges(interface: u8, clock: u8) -> Setup {
        to_entity(true, RANGE, (interface, clock), (SAMPLING_FREQ, 0), RATE_RANGES_LENGTH)
    }

    /// UAC2: puts clock selector `selector` on its input `pin` (from 1);
    /// the byte is the data.
    pub fn select_clock(interface: u8, selector: u8, pin: u8) -> (Setup, [u8; 1]) {
        (to_entity(false, CUR, (interface, selector), (CLOCK_SELECTOR, 0), 1), [pin])
    }

    /// Whether `rate` is among the rates of a clock's RANGE answer:
    /// wNumSubRanges, then each subrange's minimum, maximum and
    /// resolution, four bytes each.
    pub fn ranges_hold(bytes: &[u8], rate: u32) -> bool {
        let count = usize::from(word(bytes, 0));
        let Some(rest) = bytes.get(2..) else { return false };
        rest.as_chunks::<12>().0.iter().take(count).any(|s| {
            let field = |at: usize| u32::from_le_bytes([s[at], s[at + 1], s[at + 2], s[at + 3]]);
            let (min, max, resolution) = (field(0), field(4), field(8));
            (min..=max).contains(&rate)
                && (resolution == 0 || (rate - min).is_multiple_of(resolution))
        })
    }

    /// UAC1: asks `what` (GET_MIN, GET_MAX or GET_RES) of the volume of
    /// channel `channel` of feature unit `unit`: two bytes.
    pub fn volume_v1(what: u8, interface: u8, unit: u8, channel: u8) -> Setup {
        to_entity(true, what, (interface, unit), (VOLUME, channel), 2)
    }

    /// UAC2: asks the range of the volume of channel `channel` of feature
    /// unit `unit`, which [`Range::parse_v2`] reads.
    pub fn volume_range(interface: u8, unit: u8, channel: u8) -> Setup {
        to_entity(true, RANGE, (interface, unit), (VOLUME, channel), VOLUME_RANGE_LENGTH)
    }

    /// Sets the volume of channel `channel` of feature unit `unit` to
    /// `level` (1/256 dB); the two bytes are the data. UAC1's SET_CUR and
    /// UAC2's CUR are the same request.
    pub fn set_volume(interface: u8, unit: u8, channel: u8, level: i16) -> (Setup, [u8; 2]) {
        (to_entity(false, SET_CUR, (interface, unit), (VOLUME, channel), 2), level.to_le_bytes())
    }

    /// Mutes channel `channel` of feature unit `unit`, or unmutes it.
    pub fn set_mute(interface: u8, unit: u8, channel: u8, muted: bool) -> (Setup, [u8; 1]) {
        (to_entity(false, SET_CUR, (interface, unit), (MUTE, channel), 1), [u8::from(muted)])
    }
}

/// The unit a [`Pacer`] counts frames in: 1/(8000 << 16) of one, so that
/// a rate in frames a second over an interval of microframes (8000 a
/// second) and feedback in 16.16 are both exact.
const PACER_UNIT: u64 = 8000 << 16;

/// Sizes each service interval's packet: the frames the rate makes in
/// it, the fraction carried to the next, so that over a second the rate
/// is exact. For an asynchronous endpoint the device's feedback sets the
/// rate instead, as it plays.
#[derive(Clone, Copy, Debug)]
pub struct Pacer {
    /// The frames of an interval at the nominal rate, in [`PACER_UNIT`]s.
    nominal: u64,
    /// Those of the rate played now.
    now: u64,
    /// The fraction of a frame carried.
    carry: u64,
    /// Microframes (125 µs) an interval.
    microframes: u32,
    /// The most frames a packet holds.
    most: u32,
    /// The bits feedback is shifted by to be 16.16 frames an interval,
    /// once its format is known: positive left.
    shift: Option<i8>,
}

impl Pacer {
    /// A pacer for `rate` frames a second, packets every `microframes`
    /// microframes, of at most `most` frames.
    pub fn new(rate: u32, microframes: u32, most: u32) -> Pacer {
        let microframes = microframes.max(1);
        let nominal = (u64::from(rate) * u64::from(microframes)) << 16;
        Pacer { nominal, now: nominal, carry: 0, microframes, most, shift: None }
    }

    /// The frames of the next packet, taken.
    pub fn next_frames(&mut self) -> u32 {
        let total = self.carry + self.now;
        self.carry = total % PACER_UNIT;
        (total / PACER_UNIT).min(u64::from(self.most)) as u32
    }

    /// The frames of the next packet, without taking it.
    pub fn peek_frames(&self) -> u32 {
        ((self.carry + self.now) / PACER_UNIT).min(u64::from(self.most)) as u32
    }

    /// The rate played now, in frames a second.
    pub fn rate(&self) -> u32 {
        (self.now / (u64::from(self.microframes) << 16)) as u32
    }

    /// Takes what a feedback endpoint sent: the device's rate in frames a
    /// frame (1 ms) as 10.14 in three bytes at full speed, or a
    /// microframe as 16.16 in four at high speed (USB 2.0, 5.12.4.2).
    /// Devices send either at either speed, so the first value is shifted
    /// until it is within an eighth of the nominal rate, and the rest as
    /// it was. False, and nothing changes, for a value too far from it.
    pub fn feedback(&mut self, bytes: &[u8], full_speed: bool) -> bool {
        let raw = match *bytes {
            [a, b, c] => u32::from_le_bytes([a, b, c, 0]),
            [a, b, c, d, ..] => u32::from_le_bytes([a, b, c, d]),
            _ => return false,
        };
        // As the speed's own format says, in 16.16 frames an interval.
        let microframes = u64::from(self.microframes);
        let base = if full_speed {
            (u64::from(raw) << 2) * microframes / 8
        } else {
            u64::from(raw) * microframes
        };
        // In the pacer's units.
        let shifted = |shift: i8| {
            let value = if shift >= 0 { base << shift } else { base >> -shift };
            value.saturating_mul(8000)
        };
        let near = |value: u64| value.abs_diff(self.nominal) <= self.nominal / 8;
        let Some(shift) = self.shift.or_else(|| (-4..=4).find(|&s| near(shifted(s)))) else {
            return false;
        };
        let value = shifted(shift);
        if !near(value) {
            return false;
        }
        self.shift = Some(shift);
        self.now = value;
        true
    }
}

/// The most TDs a [`Stream`] keeps track of, and the most TRBs they may
/// take: a transfer ring's, less its Link TRB.
pub const MAX_TDS: usize = 255;
/// The most pieces a packet is cut into: where the ring wraps, and where
/// 64 KiB of memory end.
pub const MAX_PIECES: usize = 3;

/// A TD queued: where its last TRB is (which its event names), how many
/// TRBs it took, and the stream's position after it.
#[derive(Clone, Copy, Debug, Default)]
struct Queued {
    last_trb: u64,
    trbs: u8,
    end: u64,
}

/// The samples an output plays, on their way to the controller: a ring
/// of `size` bytes at physical address `base` that the kernel writes and
/// the controller reads, and the TDs that read it. Positions count bytes
/// from the stream's start, so they only grow.
#[derive(Debug)]
pub struct Stream {
    base: u64,
    size: u32,
    frame: u32,
    /// Bytes the kernel wrote.
    written: u64,
    /// Bytes queued in TDs.
    queued: u64,
    /// Bytes their TDs have played (or skipped).
    done: u64,
    /// The TDs queued and not yet seen done, oldest first, from `first`.
    tds: [Queued; MAX_TDS],
    first: usize,
    count: usize,
    /// Their TRBs.
    trbs: usize,
}

impl Stream {
    /// A stream through the ring of `size` bytes at `base`, of frames of
    /// `frame` bytes.
    pub fn new(base: u64, size: u32, frame: u32) -> Stream {
        Stream {
            base,
            size,
            frame: frame.max(1),
            written: 0,
            queued: 0,
            done: 0,
            tds: [Queued::default(); MAX_TDS],
            first: 0,
            count: 0,
            trbs: 0,
        }
    }

    /// Bytes the kernel may write now: the ring's room.
    pub fn room(&self) -> u32 {
        self.size - (self.written - self.done) as u32
    }

    /// Where in the ring the kernel writes next.
    pub fn write_offset(&self) -> u32 {
        (self.written % u64::from(self.size)) as u32
    }

    /// Notes that the kernel wrote `bytes` from [`Stream::write_offset`]
    /// on (wrapping at the ring's end), at most [`Stream::room`].
    pub fn wrote(&mut self, bytes: u32) {
        self.written += u64::from(bytes.min(self.room()));
    }

    /// Bytes written and not yet queued.
    pub fn waiting(&self) -> u32 {
        (self.written - self.queued) as u32
    }

    /// Bytes written and not yet played.
    pub fn unplayed(&self) -> u64 {
        self.written - self.done
    }

    /// Bytes played (or skipped) since the stream started.
    pub fn played(&self) -> u64 {
        self.done
    }

    /// TDs queued and not yet seen done.
    pub fn in_flight(&self) -> usize {
        self.count
    }

    /// The pieces of the next packet, of `frames` frames, into `out`: how
    /// many, and the packet's bytes. None if they are not written yet, or
    /// the transfer ring has no room for their TRBs. With `short`, fewer
    /// frames are taken if fewer are written (the end of a sound).
    pub fn next_packet(
        &self,
        frames: u32,
        short: bool,
        out: &mut [(u64, u32); MAX_PIECES],
    ) -> Option<(usize, u32)> {
        let want = frames * self.frame;
        let waiting = self.waiting() / self.frame * self.frame;
        let bytes = match waiting {
            w if w >= want => want,
            w if short && w > 0 => w,
            _ => return None,
        };
        let mut count = 0;
        let mut at = (self.queued % u64::from(self.size)) as u32;
        let mut left = bytes;
        while left > 0 {
            if count == MAX_PIECES {
                return None;
            }
            let phys = self.base + u64::from(at);
            // To the ring's end, or the end of 64 KiB of memory.
            let to_boundary = 0x1_0000 - (phys & 0xffff) as u32;
            let piece = left.min(self.size - at).min(to_boundary);
            out[count] = (phys, piece);
            count += 1;
            left -= piece;
            at = (at + piece) % self.size;
        }
        if self.count == MAX_TDS || self.trbs + count > MAX_TDS {
            return None;
        }
        Some((count, bytes))
    }

    /// Notes that the packet [`Stream::next_packet`] gave, `bytes` in
    /// `trbs` TRBs, was queued as a TD whose last TRB is at `last_trb`.
    pub fn queued_packet(&mut self, bytes: u32, trbs: usize, last_trb: u64) {
        self.queued += u64::from(bytes);
        let at = (self.first + self.count) % MAX_TDS;
        self.tds[at] = Queued { last_trb, trbs: trbs as u8, end: self.queued };
        self.count += 1;
        self.trbs += trbs;
    }

    /// The TD whose last TRB is at `trb` is done (played, or skipped when
    /// its interval was missed), and so is every TD before it. False if
    /// it is not one queued.
    pub fn done_through(&mut self, trb: u64) -> bool {
        let found = (0..self.count).find(|&i| self.tds[(self.first + i) % MAX_TDS].last_trb == trb);
        let Some(n) = found else { return false };
        for _ in 0..=n {
            let td = self.tds[self.first];
            self.done = td.end;
            self.trbs -= usize::from(td.trbs);
            self.first = (self.first + 1) % MAX_TDS;
            self.count -= 1;
        }
        true
    }

    /// The controller ran out of TDs (a ring underrun) or let go of them
    /// (the endpoint stopped): every TD queued is done.
    pub fn all_done(&mut self) {
        self.done = self.queued;
        self.first = 0;
        self.count = 0;
        self.trbs = 0;
    }

    /// Forgets everything written and queued: the stream starts again,
    /// empty. Its TDs must be gone from the transfer ring.
    pub fn clear(&mut self) {
        *self = Stream::new(self.base, self.size, self.frame);
    }
}
