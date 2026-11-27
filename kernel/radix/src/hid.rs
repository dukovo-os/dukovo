// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What HID devices report, wherever they sit: USB's HID interfaces
//! (ADR 0148) and HID over I2C's devices (ADR 0235), whose reports
//! `radix_hid` decodes.
//!
//! Each interface that reports keys or a pointer is an input device of
//! its own ([`Port`], `crate::input`, ADR 0706). What one report said is
//! handed to the input hub as one frame, with the time of the interrupt
//! that brought the report: the keys that went down or up, how far the
//! pointer moved in the counts the device reported, where a tablet
//! points, the wheels in 120ths of a notch, and the buttons that
//! changed.
//!
//! A mouse whose wheels can count finer than a notch is asked to
//! ([`fine_wheels`]), by the Resolution Multipliers of its report
//! descriptor.

use radix_abi::input::{
    ABS_X, ABS_Y, BTN_LEFT, LED_CAPS_LOCK, LED_CAPSL, LED_NUM_LOCK, LED_NUML, LED_SCROLL_LOCK,
    LED_SCROLLL, POSITION_MAX, REL_HWHEEL_HI_RES, REL_WHEEL_HI_RES, REL_X, REL_Y,
};
use radix_hid::{Decoder, Input, Layout, Pointer, usage};
use radix_input::device::padded;
use radix_mm::Kmalloc;

use crate::input::{self, About, Axis, FRAME_MAX, Said};

/// The buttons a pointer's reports tell of: BTN_LEFT and the fifteen
/// codes after it, as Linux numbers a mouse's.
const BUTTONS: u16 = 16;

/// What its driver knows of a HID interface, for whoever lists the input
/// devices.
pub struct Identity<'a> {
    /// How it is connected: a BUS_* of radix-input(2).
    pub bus: u16,
    /// What it calls itself.
    pub name: &'a str,
    /// Where it is plugged in: `usb-1-2`.
    pub place: &'a str,
    /// Its serial number, or nothing.
    pub serial: &'a str,
    /// Its maker's and its model's numbers, and its version.
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    /// How many reports a second it asks to be read for, or 0.
    pub rate_hz: u32,
}

/// What a device laid out as `layout` says of itself to the input hub.
fn about(layout: &Layout<Kmalloc>, identity: &Identity<'_>) -> About {
    let says = layout.says();
    let mut about = About::new(identity.bus, identity.name, identity.place);
    about.serial = padded(identity.serial);
    about.vendor = identity.vendor;
    about.product = identity.product;
    about.version = identity.version;
    about.rate_hz = identity.rate_hz;
    says.each_key(&mut |code| {
        about.keys.set(code, true);
    });
    for button in 0..u16::from(says.buttons).min(BUTTONS) {
        about.keys.set(BTN_LEFT + button, true);
    }
    for (has, code) in [
        (says.moves, REL_X),
        (says.moves, REL_Y),
        (says.wheel, REL_WHEEL_HI_RES),
        (says.pan, REL_HWHEEL_HI_RES),
    ] {
        about.moves.set(code, has);
    }
    if says.points {
        // `radix_hid` scales a position to this, whatever the device
        // counts in.
        for code in [ABS_X, ABS_Y] {
            let axis = Axis { code, min: 0, max: POSITION_MAX as i32, ..Axis::default() };
            about.add_axis(axis);
        }
    }
    let lights = layout.lights();
    for (bit, code) in
        [(LED_NUM_LOCK, LED_NUML), (LED_CAPS_LOCK, LED_CAPSL), (LED_SCROLL_LOCK, LED_SCROLLL)]
    {
        about.lights.set(code, lights & bit != 0);
    }
    about
}

/// A HID interface as an input device, until it is dropped: when the
/// interface goes, the device goes, with every key and button it held.
pub struct Port {
    /// Its number in the input hub.
    number: u32,
    /// The buttons its last report had down.
    buttons: u32,
}

impl Port {
    /// Adds the interface laid out as `layout` to the input devices.
    /// `None`, with a line in the log, if there is no room for it.
    pub fn add(layout: &Layout<Kmalloc>, identity: &Identity<'_>) -> Option<Port> {
        let number = input::add(&about(layout, identity))?;
        Some(Port { number, buttons: 0 })
    }

    /// Reads `report`, an Input report of the interface that came at
    /// `time_ns`, and hands the input hub what it said as one frame.
    pub fn report(
        &mut self,
        decoder: &mut Decoder,
        layout: &Layout<Kmalloc>,
        report: &[u8],
        time_ns: u64,
    ) {
        let mut frame =
            Frame { number: self.number, time_ns, said: [NOTHING; FRAME_MAX], count: 0 };
        let buttons = &mut self.buttons;
        decoder.report(layout, report, &mut |input| match input {
            Input::Key { code, down } => frame.push(Said::key(code, down)),
            Input::Pointer(pointer) => {
                frame.pointer(&pointer, *buttons);
                *buttons = pointer.buttons;
            }
        });
        frame.send();
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        input::remove(self.number);
    }
}

/// A Feature report asked of a HID interface, or sent to it, as HID over
/// USB carries one: its ID first, if the device numbers its reports.
pub enum Feature<'a> {
    /// The report of this ID is read into these bytes.
    Get(u8, &'a mut [u8]),
    /// These bytes are the report of this ID to send.
    Set(u8, &'a [u8]),
}

/// Asks the wheels of a HID interface laid out as `layout` to count
/// finer than a notch (ADR 0706). Each Feature report that holds a
/// Resolution Multiplier of a wheel's is read, its multipliers are set
/// to their most with its other values as they were, and it is sent
/// back and read again. `decoder` is told of each report the device
/// answers with the multipliers at their most; if it will not say again,
/// that it took the report is enough. A device that refuses any of it
/// is read in whole notches, as it starts. Returns whether a wheel
/// counts finer now.
///
/// `ask` carries a report out: it returns how many bytes of one came, or
/// 0 for one sent, and `None` if the device refused. `buffer` is room
/// for a report with its ID.
pub fn fine_wheels(
    layout: &Layout<Kmalloc>,
    decoder: &mut Decoder,
    buffer: &mut [u8],
    ask: &mut dyn FnMut(Feature<'_>) -> Option<usize>,
) -> bool {
    let skip = usize::from(layout.numbered());
    let mut finer = false;
    for (id, bytes) in layout.fine_reports() {
        let Some(report) = buffer.get_mut(..bytes + skip) else { continue };
        let whole = |came: Option<usize>| came == Some(bytes + skip);
        if !whole(ask(Feature::Get(id, report))) || !layout.set_fine(id, &mut report[skip..]) {
            continue;
        }
        if ask(Feature::Set(id, report)).is_none() {
            continue;
        }
        if !whole(ask(Feature::Get(id, report))) || layout.is_fine(id, &report[skip..]) {
            decoder.fine(layout, id);
            finer = true;
        }
    }
    finer
}

/// What fills a frame's places before a report does.
const NOTHING: Said = Said::key(0, false);

/// What one report said so far.
struct Frame {
    number: u32,
    time_ns: u64,
    said: [Said; FRAME_MAX],
    count: usize,
}

impl Frame {
    /// Adds `said`; a report that says more than a frame holds is handed
    /// over in parts.
    fn push(&mut self, said: Said) {
        if self.count == FRAME_MAX {
            self.send();
        }
        self.said[self.count] = said;
        self.count += 1;
    }

    /// Adds what `pointer` did, whose buttons were `buttons` before.
    fn pointer(&mut self, pointer: &Pointer, buttons: u32) {
        for (code, by) in [
            (REL_X, pointer.dx),
            (REL_Y, pointer.dy),
            (REL_WHEEL_HI_RES, pointer.wheel),
            (REL_HWHEEL_HI_RES, pointer.pan),
        ] {
            if by != 0 {
                self.push(Said::moved(code, by));
            }
        }
        if pointer.absolute {
            self.push(Said::at(ABS_X, pointer.x as i32));
            self.push(Said::at(ABS_Y, pointer.y as i32));
        }
        let changed = buttons ^ pointer.buttons;
        for button in (0..BUTTONS).filter(|button| changed & (1 << button) != 0) {
            self.push(Said::key(BTN_LEFT + button, pointer.buttons & (1 << button) != 0));
        }
    }

    /// Hands the input hub what was said, and starts anew.
    fn send(&mut self) {
        if self.count > 0 {
            input::frame(self.number, self.time_ns, &self.said[..self.count]);
            self.count = 0;
        }
    }
}

/// What a device laid out as `layout` is, and where it is read, for the
/// log; and whether anything it reports is read.
pub fn what(layout: &Layout<Kmalloc>) -> (&'static str, bool) {
    let touchpad = layout.applications().contains(&usage::TOUCH_PAD);
    let what = if layout.is_keyboard() {
        "a keyboard, read into /devices/kbd"
    } else if touchpad && layout.is_pointer() {
        // Until it is told to report its fingers, it is its mouse.
        "a touchpad, read as a mouse into /devices/mouse"
    } else if layout.is_pointer() && layout.is_absolute() {
        "a tablet, read into /devices/mouse"
    } else if layout.is_pointer() {
        "a mouse, read into /devices/mouse"
    } else if layout.has_keys() {
        "media keys, read into /devices/kbd"
    } else {
        return ("nothing it reports is read", false);
    };
    (what, true)
}
