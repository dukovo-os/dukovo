// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What devices say, and the records readers read.
//!
//! A driver hands the hub what one report said as [`Said`]s. The hub
//! gives each reader the records of its file: a device's file reads
//! `InputEvent`s, _/devices/kbd_ reads `Event`s, _/devices/mouse_ reads
//! `Motion`s and _/devices/input/control_ reads `InputChange`s
//! (radix-input(2)). [`Record`] writes each as the bytes a read returns.

use radix_abi::input::{
    EV_ABS, EV_KEY, EV_REL, EV_SYN, Event, InputAxis, InputChange, InputEvent, InputInfo, Motion,
    PRESSED, RELEASED,
};

/// The most things one report says that reach a reader as one frame; a
/// report that says more is read as several.
pub const FRAME_MAX: usize = 64;

/// One thing a report said: an InputEvent without its time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Said {
    /// An EV_*.
    pub kind: u16,
    /// The key, the axis.
    pub code: u16,
    /// What it did.
    pub value: i32,
}

impl Said {
    /// The key or button `code` went down, or up.
    pub const fn key(code: u16, down: bool) -> Said {
        Said { kind: EV_KEY, code, value: if down { PRESSED as i32 } else { RELEASED as i32 } }
    }

    /// The axis `code` (a REL_*) moved by `by`.
    pub const fn moved(code: u16, by: i32) -> Said {
        Said { kind: EV_REL, code, value: by }
    }

    /// The axis `code` (an ABS_*) is at `at`.
    pub const fn at(code: u16, at: i32) -> Said {
        Said { kind: EV_ABS, code, value: at }
    }

    /// This, as the event of a report that came at `time_ns`.
    pub const fn event(self, time_ns: u64) -> InputEvent {
        InputEvent { time_ns, kind: self.kind, code: self.code, value: self.value }
    }
}

/// The EV_SYN event with `code` (a SYN_*) at `time_ns`.
pub const fn mark(time_ns: u64, code: u16) -> InputEvent {
    InputEvent { time_ns, kind: EV_SYN, code, value: 0 }
}

/// A structure a read of an input file returns.
pub trait Record: Copy {
    /// Its bytes.
    const SIZE: usize;

    /// Writes its `SIZE` bytes into `out`, which is that long.
    fn encode(&self, out: &mut [u8]);
}

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

impl Record for InputEvent {
    const SIZE: usize = InputEvent::SIZE;

    fn encode(&self, out: &mut [u8]) {
        put(out, InputEvent::OFFSET_TIME_NS, &self.time_ns.to_le_bytes());
        put(out, InputEvent::OFFSET_KIND, &self.kind.to_le_bytes());
        put(out, InputEvent::OFFSET_CODE, &self.code.to_le_bytes());
        put(out, InputEvent::OFFSET_VALUE, &self.value.to_le_bytes());
    }
}

impl Record for Event {
    const SIZE: usize = Event::SIZE;

    fn encode(&self, out: &mut [u8]) {
        out.fill(0);
        put(out, Event::OFFSET_TIME_NS, &self.time_ns.to_le_bytes());
        put(out, Event::OFFSET_CODE, &self.code.to_le_bytes());
        put(out, Event::OFFSET_VALUE, &self.value.to_le_bytes());
    }
}

impl Record for Motion {
    const SIZE: usize = Motion::SIZE;

    fn encode(&self, out: &mut [u8]) {
        put(out, Motion::OFFSET_TIME_NS, &self.time_ns.to_le_bytes());
        put(out, Motion::OFFSET_DX, &self.dx.to_le_bytes());
        put(out, Motion::OFFSET_DY, &self.dy.to_le_bytes());
        put(out, Motion::OFFSET_WHEEL, &self.wheel.to_le_bytes());
        put(out, Motion::OFFSET_PAN, &self.pan.to_le_bytes());
        put(out, Motion::OFFSET_X, &self.x.to_le_bytes());
        put(out, Motion::OFFSET_Y, &self.y.to_le_bytes());
        put(out, Motion::OFFSET_BUTTONS, &self.buttons.to_le_bytes());
        put(out, Motion::OFFSET_FLAGS, &self.flags.to_le_bytes());
    }
}

impl Record for InputInfo {
    const SIZE: usize = InputInfo::SIZE;

    fn encode(&self, out: &mut [u8]) {
        put(out, InputInfo::OFFSET_DEVICE, &self.device.to_le_bytes());
        put(out, InputInfo::OFFSET_BUS, &self.bus.to_le_bytes());
        put(out, InputInfo::OFFSET_VENDOR, &self.vendor.to_le_bytes());
        put(out, InputInfo::OFFSET_PRODUCT, &self.product.to_le_bytes());
        put(out, InputInfo::OFFSET_VERSION, &self.version.to_le_bytes());
        put(out, InputInfo::OFFSET_RATE_HZ, &self.rate_hz.to_le_bytes());
    }
}

impl Record for InputAxis {
    const SIZE: usize = InputAxis::SIZE;

    fn encode(&self, out: &mut [u8]) {
        put(out, InputAxis::OFFSET_VALUE, &self.value.to_le_bytes());
        put(out, InputAxis::OFFSET_MIN, &self.min.to_le_bytes());
        put(out, InputAxis::OFFSET_MAX, &self.max.to_le_bytes());
        put(out, InputAxis::OFFSET_FUZZ, &self.fuzz.to_le_bytes());
        put(out, InputAxis::OFFSET_FLAT, &self.flat.to_le_bytes());
        put(out, InputAxis::OFFSET_RESOLUTION, &self.resolution.to_le_bytes());
    }
}

impl Record for InputChange {
    const SIZE: usize = InputChange::SIZE;

    fn encode(&self, out: &mut [u8]) {
        put(out, InputChange::OFFSET_TIME_NS, &self.time_ns.to_le_bytes());
        put(out, InputChange::OFFSET_DEVICE, &self.device.to_le_bytes());
        put(out, InputChange::OFFSET_WHAT, &self.what.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use radix_abi::input::{BTN_LEFT, DEVICE_ADDED, MOTION_ABSOLUTE, REL_X, SYN_REPORT};

    use super::*;

    fn bytes<R: Record>(record: &R) -> Vec<u8> {
        let mut out = vec![0xaa; R::SIZE];
        record.encode(&mut out);
        out
    }

    #[test]
    fn an_event_is_sixteen_bytes_time_kind_code_value() {
        let event = Said::moved(REL_X, -2).event(0x0102_0304_0506_0708);
        let out = bytes(&event);
        assert_eq!(out.len(), 16);
        assert_eq!(out[..8], [8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(out[8..], [2, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff]);
        assert_eq!(bytes(&mark(1, SYN_REPORT))[8..], [0; 8]);
        assert_eq!(Said::key(BTN_LEFT, true).value, 1);
        assert_eq!(Said::key(BTN_LEFT, false).value, 0);
    }

    #[test]
    fn what_a_device_says_of_itself_and_of_an_axis_is_laid_out_in_order() {
        let info = InputInfo {
            device: 7,
            bus: 3,
            vendor: 0x046d,
            product: 0xc077,
            version: 0x0111,
            rate_hz: 1000,
        };
        assert_eq!(
            bytes(&info),
            [7, 0, 0, 0, 3, 0, 0x6d, 0x04, 0x77, 0xc0, 0x11, 0x01, 0xe8, 3, 0, 0]
        );
        let axis = InputAxis { value: 1, min: -2, max: 3, fuzz: 4, flat: 5, resolution: 6 };
        let out = bytes(&axis);
        assert_eq!(out.len(), 24);
        assert_eq!(out[..12], [1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 3, 0, 0, 0]);
        assert_eq!(out[12..], [4, 0, 0, 0, 5, 0, 0, 0, 6, 0, 0, 0]);
    }

    #[test]
    fn the_merged_files_records_are_laid_out_as_before() {
        let key = Event { time_ns: 5, code: 30, value: 1, reserved: 0 };
        let out = bytes(&key);
        assert_eq!(out, [5, 0, 0, 0, 0, 0, 0, 0, 30, 0, 1, 0, 0, 0, 0, 0]);
        let motion = Motion {
            time_ns: 1,
            dx: 2,
            dy: -1,
            wheel: 3,
            pan: 4,
            x: 5,
            y: 6,
            buttons: 7,
            flags: MOTION_ABSOLUTE,
        };
        let out = bytes(&motion);
        assert_eq!(out.len(), 40);
        let words: Vec<u32> =
            out[8..].chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
        assert_eq!(words, [2, u32::MAX, 3, 4, 5, 6, 7, 1]);
    }

    #[test]
    fn a_change_is_sixteen_bytes() {
        let change = InputChange { time_ns: 9, device: 3, what: DEVICE_ADDED };
        assert_eq!(bytes(&change), [9, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]);
    }
}
