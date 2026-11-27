// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One input device: what it says of itself, and what is down on it.
//!
//! A driver describes its device with an [`About`]. The hub keeps a
//! [`Device`] for it: the keys and buttons that are down, the switches
//! that are on, and where each axis last was. From that it answers a
//! reader that asks for the state, lets go for a device that is pulled
//! out of what it held down, drops what changes nothing (a key that goes
//! up without having gone down, a position the pointer is already at),
//! and makes the `Motion` that _/devices/mouse_ reads.

use radix_abi::input::{
    ABS_CODES, ABS_X, ABS_Y, BTN_LEFT, EV_ABS, EV_KEY, EV_KINDS, EV_LED, EV_REL, EV_SW, EV_SYN,
    InputAxis, InputInfo, KEY_CODES, LED_CODES, MOTION_ABSOLUTE, Motion, POSITION_MAX, REL_CODES,
    REL_HWHEEL_HI_RES, REL_WHEEL_HI_RES, REL_X, REL_Y, REPEATED, SW_CODES,
};

use crate::bits::Bits;
use crate::event::Said;

/// A bit for each key and button code.
pub type Keys = Bits<{ KEY_CODES as usize / 64 }>;
/// A bit for each code of a kind with at most 64.
pub type Few = Bits<1>;

/// The most axes with a range a device has.
pub const AXES_MAX: usize = 8;
/// How many buttons _/devices/mouse_ tells of: BTN_LEFT and the fifteen
/// codes after it.
const BUTTONS: u16 = 16;
/// A wheel's notch, in what REL_WHEEL_HI_RES counts.
pub const NOTCH: i32 = 120;

/// One axis that says where the device points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Axis {
    /// An ABS_*.
    pub code: u16,
    /// The least it says.
    pub min: i32,
    /// The most.
    pub max: i32,
    /// How much it wavers while still.
    pub fuzz: i32,
    /// How far from the middle still counts as the middle.
    pub flat: i32,
    /// Units in a millimetre, or 0.
    pub resolution: i32,
}

/// What a device says of itself.
#[derive(Clone, Debug)]
pub struct About {
    /// Its name, padded with zeros.
    pub name: [u8; 64],
    /// Where it is plugged in, padded with zeros.
    pub place: [u8; 32],
    /// Its serial number, padded with zeros.
    pub serial: [u8; 32],
    /// How it is connected: a BUS_*.
    pub bus: u16,
    /// Its maker's number.
    pub vendor: u16,
    /// Its model's number.
    pub product: u16,
    /// Its version.
    pub version: u16,
    /// Reports a second it asks to be read for, or 0.
    pub rate_hz: u32,
    /// The keys and buttons it has.
    pub keys: Keys,
    /// The REL_* axes it has.
    pub moves: Few,
    /// The switches it has.
    pub switches: Few,
    /// The lights it has.
    pub lights: Few,
    /// The ABS_* axes it has, with their ranges.
    pub axes: [Option<Axis>; AXES_MAX],
}

/// `text` as a field of `N` bytes padded with zeros: as much of it as
/// fits with a zero after, cut between characters.
pub fn padded<const N: usize>(text: &str) -> [u8; N] {
    let mut out = [0; N];
    let mut len = text.len().min(N.saturating_sub(1));
    while !text.is_char_boundary(len) {
        len -= 1;
    }
    out[..len].copy_from_slice(&text.as_bytes()[..len]);
    out
}

/// The text of a field padded with zeros; empty if it is not UTF-8.
pub fn text(field: &[u8]) -> &str {
    let len = field.iter().position(|&byte| byte == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..len]).unwrap_or("")
}

impl About {
    /// A device on `bus` named `name` and plugged in at `place`, which
    /// has nothing yet.
    pub fn new(bus: u16, name: &str, place: &str) -> About {
        About {
            name: padded(name),
            place: padded(place),
            serial: [0; 32],
            bus,
            vendor: 0,
            product: 0,
            version: 0,
            rate_hz: 0,
            keys: Keys::new(),
            moves: Few::new(),
            switches: Few::new(),
            lights: Few::new(),
            axes: [None; AXES_MAX],
        }
    }

    /// Gives it `axis`. Returns false if it has [`AXES_MAX`] already;
    /// an axis it has is replaced.
    pub fn add_axis(&mut self, axis: Axis) -> bool {
        let same = self.axes.iter().position(|have| have.is_some_and(|a| a.code == axis.code));
        let free = self.axes.iter().position(Option::is_none);
        match same.or(free) {
            Some(index) => {
                self.axes[index] = Some(axis);
                true
            }
            None => false,
        }
    }

    /// Where the axis `code` is among its axes.
    fn axis_index(&self, code: u16) -> Option<usize> {
        self.axes.iter().position(|axis| axis.is_some_and(|axis| axis.code == code))
    }

    /// The ABS_* codes it has.
    fn positions(&self) -> Few {
        let mut bits = Few::new();
        for axis in self.axes.iter().flatten() {
            bits.set(axis.code, true);
        }
        bits
    }

    /// INPUT_BITS: writes a bit for each code of `kind` it has into
    /// `out`, as many bytes as fit, and returns the bytes of them all;
    /// `None` for a kind that has no codes.
    pub fn bits(&self, kind: u16, out: &mut [u8]) -> Option<usize> {
        Some(match kind {
            EV_SYN => self.kinds().write(EV_KINDS, out),
            EV_KEY => self.keys.write(KEY_CODES, out),
            EV_REL => self.moves.write(REL_CODES, out),
            EV_ABS => self.positions().write(ABS_CODES, out),
            EV_SW => self.switches.write(SW_CODES, out),
            EV_LED => self.lights.write(LED_CODES, out),
            _ => return None,
        })
    }

    /// The kinds it has codes of.
    fn kinds(&self) -> Few {
        let mut kinds = Few::of(&[EV_SYN]);
        kinds.set(EV_KEY, !self.keys.is_empty());
        kinds.set(EV_REL, !self.moves.is_empty());
        kinds.set(EV_ABS, self.axes.iter().any(Option::is_some));
        kinds.set(EV_SW, !self.switches.is_empty());
        kinds.set(EV_LED, !self.lights.is_empty());
        kinds
    }
}

/// What the hub does with one thing a device said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// It changed something: every reader hears it.
    Changed,
    /// A key that is down, pressed again: _/devices/kbd_ reads a repeat,
    /// and a device's file nothing.
    Repeat,
    /// It changed nothing, or the device has no such thing.
    Nothing,
}

/// What one frame of a pointer adds up to, for _/devices/mouse_.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    dx: i32,
    dy: i32,
    /// It said where it points.
    placed: bool,
    /// A button changed.
    pressed: bool,
}

/// A device the hub knows.
#[derive(Debug)]
pub struct Device {
    /// Its number, which no other device ever has.
    pub number: u32,
    /// What it says of itself.
    pub about: About,
    /// The keys and buttons that are down.
    down: Keys,
    /// The switches that are on.
    on: Few,
    /// Where each axis of `about.axes` last was.
    values: [i32; AXES_MAX],
    /// What each wheel has turned that is not yet a whole notch, in
    /// 120ths: vertical, horizontal.
    turned: [i32; 2],
}

impl Device {
    /// The device `number`, with nothing down.
    pub fn new(number: u32, about: &About) -> Device {
        Device {
            number,
            about: about.clone(),
            down: Keys::new(),
            on: Few::new(),
            values: [0; AXES_MAX],
            turned: [0; 2],
        }
    }

    /// Notes what `said` changes, and says who hears it.
    pub fn hear(&mut self, said: Said) -> Heard {
        let changed = match said.kind {
            EV_KEY if said.value == i32::from(REPEATED) => {
                return if self.down.has(said.code) { Heard::Repeat } else { Heard::Nothing };
            }
            EV_KEY if said.value != 0 => {
                if !self.down.set(said.code, true) {
                    let held = self.down.has(said.code);
                    return if held { Heard::Repeat } else { Heard::Nothing };
                }
                true
            }
            EV_KEY => self.down.set(said.code, false),
            EV_SW => self.on.set(said.code, said.value != 0),
            EV_REL => said.value != 0,
            EV_ABS => self.place(said.code, said.value),
            _ => false,
        };
        if changed { Heard::Changed } else { Heard::Nothing }
    }

    /// Notes that the axis `code` is at `value`; false if it was there
    /// already or the device has no such axis.
    fn place(&mut self, code: u16, value: i32) -> bool {
        let Some(index) = self.about.axis_index(code) else { return false };
        let moved = self.values[index] != value;
        self.values[index] = value;
        moved
    }

    /// The keys and buttons that are down.
    pub fn down(&self) -> &Keys {
        &self.down
    }

    /// INPUT_STATE: writes a bit for each key and button that is down
    /// (EV_KEY) or each switch that is on (EV_SW) into `out`, and returns
    /// the bytes of them all. A reader that is not `heard` is told that
    /// nothing is down. `None` for another kind.
    pub fn state(&self, kind: u16, heard: bool, out: &mut [u8]) -> Option<usize> {
        match kind {
            EV_KEY if heard => Some(self.down.write(KEY_CODES, out)),
            EV_KEY => Some(Keys::new().write(KEY_CODES, out)),
            EV_SW if heard => Some(self.on.write(SW_CODES, out)),
            EV_SW => Some(Few::new().write(SW_CODES, out)),
            _ => None,
        }
    }

    /// INPUT_AXIS: the axis `code`, with where it last was; a reader that
    /// is not `heard` is told its least.
    pub fn axis(&self, code: u16, heard: bool) -> Option<InputAxis> {
        let index = self.about.axis_index(code)?;
        let axis = self.about.axes[index]?;
        Some(InputAxis {
            value: if heard { self.values[index] } else { axis.min },
            min: axis.min,
            max: axis.max,
            fuzz: axis.fuzz,
            flat: axis.flat,
            resolution: axis.resolution,
        })
    }

    /// INPUT_INFO's answer.
    pub fn info(&self) -> InputInfo {
        let about = &self.about;
        InputInfo {
            device: self.number,
            bus: about.bus,
            vendor: about.vendor,
            product: about.product,
            version: about.version,
            rate_hz: about.rate_hz,
        }
    }

    /// Adds what `said`, which changed something, means for
    /// _/devices/mouse_ to `moved`.
    pub fn add(&mut self, moved: &mut Moved, said: Said) {
        match (said.kind, said.code) {
            (EV_REL, REL_X) => moved.dx = moved.dx.saturating_add(said.value),
            (EV_REL, REL_Y) => moved.dy = moved.dy.saturating_add(said.value),
            (EV_REL, REL_WHEEL_HI_RES) => {
                self.turned[0] = self.turned[0].saturating_add(said.value);
            }
            (EV_REL, REL_HWHEEL_HI_RES) => {
                self.turned[1] = self.turned[1].saturating_add(said.value);
            }
            (EV_ABS, ABS_X | ABS_Y) => moved.placed = true,
            (EV_KEY, code) if (BTN_LEFT..BTN_LEFT + BUTTONS).contains(&code) => {
                moved.pressed = true;
            }
            _ => {}
        }
    }

    /// The buttons that are down, as `Motion` has them: a bit for
    /// BTN_LEFT, and one for each of the fifteen codes after it.
    pub fn buttons(&self) -> u32 {
        (0..BUTTONS).filter(|&bit| self.down.has(BTN_LEFT + bit)).fold(0, |all, bit| all | 1 << bit)
    }

    /// The `Motion` of a frame that came at `time_ns` and added up to
    /// `moved`, if _/devices/mouse_ has anything to read of it: a wheel
    /// that has not turned a whole notch since its last is not read yet.
    pub fn motion(&mut self, time_ns: u64, moved: &Moved) -> Option<Motion> {
        let [wheel, pan] = self.turned.map(|turned| turned / NOTCH);
        self.turned[0] -= wheel * NOTCH;
        self.turned[1] -= pan * NOTCH;
        let still = moved.dx == 0 && moved.dy == 0 && wheel == 0 && pan == 0;
        if still && !moved.placed && !moved.pressed {
            return None;
        }
        let (x, y) = if moved.placed { (self.scaled(ABS_X), self.scaled(ABS_Y)) } else { (0, 0) };
        Some(Motion {
            time_ns,
            dx: moved.dx,
            dy: moved.dy,
            wheel,
            pan,
            x,
            y,
            buttons: self.buttons(),
            flags: if moved.placed { MOTION_ABSOLUTE } else { 0 },
        })
    }

    /// Where the axis `code` is between its least (0) and its most
    /// (POSITION_MAX); 0 for an axis the device does not have.
    fn scaled(&self, code: u16) -> u32 {
        let Some(axis) = self.axis(code, true) else { return 0 };
        let span = i64::from(axis.max) - i64::from(axis.min);
        if span <= 0 {
            return 0;
        }
        let from_min = (i64::from(axis.value) - i64::from(axis.min)).clamp(0, span);
        (from_min * i64::from(POSITION_MAX) / span) as u32
    }
}

#[cfg(test)]
mod tests {
    use radix_abi::input::{BTN_RIGHT, BUS_USB, KEY_A, SW_LID};

    use super::*;

    fn tablet() -> Device {
        let mut about = About::new(BUS_USB, "A tablet", "usb 1-2");
        about.keys = Keys::of(&[BTN_LEFT, BTN_RIGHT, KEY_A]);
        about.moves = Few::of(&[REL_WHEEL_HI_RES]);
        about.switches = Few::of(&[SW_LID]);
        assert!(about.add_axis(Axis { code: ABS_X, min: 0, max: 1000, ..Axis::default() }));
        assert!(about.add_axis(Axis { code: ABS_Y, min: -100, max: 100, ..Axis::default() }));
        Device::new(7, &about)
    }

    #[test]
    fn a_name_is_cut_between_characters_and_read_back() {
        let field: [u8; 8] = padded("Müller's keyboard");
        // Seven bytes fit before the zero; "Müller" is seven.
        assert_eq!(text(&field), "Müller");
        let field: [u8; 3] = padded("aé");
        assert_eq!(text(&field), "a");
        assert_eq!(text(&padded::<4>("")), "");
        assert_eq!(text(&[0xff, 0xfe, 0]), "");
        assert_eq!(text(b"full"), "full");
    }

    #[test]
    fn what_changes_nothing_is_not_heard() {
        let mut device = tablet();
        assert_eq!(device.hear(Said::key(KEY_A, false)), Heard::Nothing);
        assert_eq!(device.hear(Said::key(KEY_A, true)), Heard::Changed);
        assert_eq!(device.hear(Said::key(KEY_A, true)), Heard::Repeat);
        let repeat = Said { kind: EV_KEY, code: KEY_A, value: i32::from(REPEATED) };
        assert_eq!(device.hear(repeat), Heard::Repeat);
        assert_eq!(device.hear(Said::key(KEY_A, false)), Heard::Changed);
        assert_eq!(device.hear(repeat), Heard::Nothing);
        assert_eq!(device.hear(Said::moved(REL_X, 0)), Heard::Nothing);
        assert_eq!(device.hear(Said::moved(REL_X, -1)), Heard::Changed);
        assert_eq!(device.hear(Said::at(ABS_X, 5)), Heard::Changed);
        assert_eq!(device.hear(Said::at(ABS_X, 5)), Heard::Nothing);
        // An axis it never said it has.
        assert_eq!(device.hear(Said::at(40, 5)), Heard::Nothing);
        let lid = Said { kind: EV_SW, code: SW_LID, value: 1 };
        assert_eq!(device.hear(lid), Heard::Changed);
        assert_eq!(device.hear(lid), Heard::Nothing);
        assert_eq!(device.hear(Said { kind: 0x15, code: 0, value: 1 }), Heard::Nothing);
    }

    #[test]
    fn a_reader_that_is_not_heard_is_told_nothing_is_down() {
        let mut device = tablet();
        device.hear(Said::key(KEY_A, true));
        device.hear(Said::at(ABS_Y, 50));
        device.hear(Said { kind: EV_SW, code: SW_LID, value: 1 });
        let mut out = [0; 96];
        assert_eq!(device.state(EV_KEY, true, &mut out), Some(96));
        assert_eq!(out[usize::from(KEY_A) / 8], 1 << (KEY_A % 8));
        assert_eq!(device.state(EV_KEY, false, &mut out), Some(96));
        assert_eq!(out, [0; 96]);
        assert_eq!(device.state(EV_SW, true, &mut out), Some(4));
        assert_eq!(out[0], 1);
        assert_eq!(device.state(EV_SW, false, &mut out), Some(4));
        assert_eq!(out[0], 0);
        assert_eq!(device.state(EV_REL, true, &mut out), None);
        assert_eq!(device.axis(ABS_Y, true).map(|axis| axis.value), Some(50));
        assert_eq!(device.axis(ABS_Y, false).map(|axis| axis.value), Some(-100));
        assert_eq!(device.axis(5, true).map(|axis| axis.max), None);
    }

    #[test]
    fn it_says_what_it_has_by_kind() {
        let device = tablet();
        let mut out = [0; 96];
        assert_eq!(device.about.bits(EV_SYN, &mut out), Some(4));
        let kinds = [EV_SYN, EV_KEY, EV_REL, EV_ABS, EV_SW];
        assert_eq!(out[0], kinds.iter().fold(0, |all, kind| all | 1 << kind));
        assert_eq!(out[2], 0);
        assert_eq!(device.about.bits(EV_ABS, &mut out), Some(8));
        assert_eq!(out[0], 0b11);
        assert_eq!(device.about.bits(EV_KEY, &mut out), Some(96));
        assert_eq!(out[usize::from(BTN_LEFT) / 8], 0b11);
        assert_eq!(device.about.bits(EV_LED, &mut out), Some(2));
        assert_eq!(device.about.bits(4, &mut out), None);
        let info = device.info();
        assert_eq!((info.device, info.bus), (7, BUS_USB));
        assert_eq!(text(&device.about.name), "A tablet");
        assert_eq!(text(&device.about.place), "usb 1-2");
        assert_eq!(text(&device.about.serial), "");
    }

    #[test]
    fn a_ninth_axis_is_refused_and_one_it_has_is_replaced() {
        let mut about = About::new(BUS_USB, "Many axes", "");
        for code in 0..8 {
            assert!(about.add_axis(Axis { code, max: 1, ..Axis::default() }));
        }
        assert!(!about.add_axis(Axis { code: 8, ..Axis::default() }));
        assert!(about.add_axis(Axis { code: 3, max: 9, ..Axis::default() }));
        assert_eq!(Device::new(0, &about).axis(3, true).map(|axis| axis.max), Some(9));
    }

    /// Hears `said` on `device` and adds up what changed.
    fn frame(device: &mut Device, said: &[Said]) -> Moved {
        let mut moved = Moved::default();
        for &said in said {
            if device.hear(said) == Heard::Changed {
                device.add(&mut moved, said);
            }
        }
        moved
    }

    #[test]
    fn a_frame_adds_up_to_one_motion_with_the_buttons_after_it() {
        let mut device = tablet();
        let moved = frame(
            &mut device,
            &[Said::moved(REL_X, 3), Said::moved(REL_X, 2), Said::key(BTN_RIGHT, true)],
        );
        let motion = device.motion(9, &moved).unwrap();
        assert_eq!((motion.time_ns, motion.dx, motion.dy), (9, 5, 0));
        assert_eq!((motion.buttons, motion.flags), (2, 0));
        // A key of the keyboard is nothing to the mouse file.
        let moved = frame(&mut device, &[Said::key(KEY_A, true)]);
        assert_eq!(device.motion(10, &moved), None);
        // The button going up is, and nothing moved with it.
        let moved = frame(&mut device, &[Said::key(BTN_RIGHT, false)]);
        let motion = device.motion(11, &moved).unwrap();
        assert_eq!((motion.dx, motion.buttons), (0, 0));
    }

    #[test]
    fn a_fine_wheel_is_read_a_whole_notch_at_a_time() {
        let mut device = tablet();
        let turn = |device: &mut Device, by: i32| {
            let moved = frame(device, &[Said::moved(REL_WHEEL_HI_RES, by)]);
            device.motion(0, &moved).map(|motion| motion.wheel)
        };
        assert_eq!(turn(&mut device, 50), None);
        assert_eq!(turn(&mut device, 50), None);
        // 130 so far: one notch, and 10 kept.
        assert_eq!(turn(&mut device, 30), Some(1));
        assert_eq!(turn(&mut device, 110), Some(1));
        assert_eq!(turn(&mut device, -240), Some(-2));
        // A plain wheel says a whole notch each time.
        assert_eq!(turn(&mut device, -120), Some(-1));
    }

    #[test]
    fn a_position_is_read_between_the_screen_s_edges() {
        let mut device = tablet();
        let moved = frame(&mut device, &[Said::at(ABS_X, 500), Said::at(ABS_Y, 100)]);
        let motion = device.motion(0, &moved).unwrap();
        assert_eq!((motion.x, motion.y, motion.flags), (32767, 65535, MOTION_ABSOLUTE));
        // Beyond its range it is at the edge.
        let moved = frame(&mut device, &[Said::at(ABS_X, 5000), Said::at(ABS_Y, -500)]);
        let motion = device.motion(0, &moved).unwrap();
        assert_eq!((motion.x, motion.y), (65535, 0));
    }
}
