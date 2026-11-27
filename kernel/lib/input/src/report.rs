// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The hub in words: `/kernel/class/input/state`, which `hardware inputs`
//! and whatever else an administrator lists the input devices with read.
//!
//! The first line is the hub's own:
//!
//! ```text
//! hub readers=3 hold=0
//! ```
//!
//! how many descriptions of input files are open, and whether the secure
//! key's hold is on. Then a line for each device, lowest number first:
//!
//! ```text
//! device 2 bus=usb vendor=046d product=c077 version=0111 rate=125 readers=1 place=usb-1-2 key=000000…07 rel=0308 abs= sw= led= name=USB Optical Mouse
//! ```
//!
//! `bus` is `usb`, `ps2`, `i2c`, `virtual` or the bus's number; the
//! numbers after it are four hexadecimal digits each; `rate` is the
//! reports a second the device asks to be read for, 0 where it sends when
//! it likes; `readers` is how many descriptions of its own file are open.
//! `key`, `rel`, `abs`, `sw` and `led` are the codes of each kind it has,
//! as radix-input(2)'s INPUT_BITS answers, each byte as two hexadecimal
//! digits, without the zero bytes at the end. The name is the rest of the
//! line, whatever it holds.
//!
//! Anyone may read the file, so it has no serial number: a program that
//! is handed a device's own file asks that for it (INPUT_SERIAL).

use core::alloc::Allocator;
use core::fmt::{self, Write};

use radix_abi::input::{
    ABS_CODES, BUS_I2C, BUS_I8042, BUS_USB, BUS_VIRTUAL, EV_ABS, EV_KEY, EV_LED, EV_REL, EV_SW,
    KEY_CODES,
};

use crate::device::{Device, text};
use crate::hub::{Hub, Wake};

/// The bytes of the longest answer INPUT_BITS has.
const BITS_MAX: usize = KEY_CODES as usize / 8;

const _: () = assert!(ABS_CODES <= KEY_CODES);

impl<W: Wake, A: Allocator + Copy> Hub<W, A> {
    /// Writes the hub in words to `out`, as the module says.
    ///
    /// # Errors
    ///
    /// `out`'s.
    pub fn describe(&self, out: &mut dyn Write) -> fmt::Result {
        writeln!(out, "hub readers={} hold={}", self.readers(), u8::from(self.is_held()))?;
        for device in self.devices() {
            describe_device(device, self.readers_of(device.number), out)?;
        }
        Ok(())
    }
}

/// Writes the line of `device`, whose own file `readers` descriptions
/// have open.
fn describe_device(device: &Device, readers: usize, out: &mut dyn Write) -> fmt::Result {
    let about = &device.about;
    write!(out, "device {} bus=", device.number)?;
    match about.bus {
        BUS_USB => out.write_str("usb")?,
        BUS_I8042 => out.write_str("ps2")?,
        BUS_I2C => out.write_str("i2c")?,
        BUS_VIRTUAL => out.write_str("virtual")?,
        other => write!(out, "{other}")?,
    }
    write!(
        out,
        " vendor={:04x} product={:04x} version={:04x} rate={} readers={readers} place={}",
        about.vendor,
        about.product,
        about.version,
        about.rate_hz,
        text(&about.place),
    )?;
    for (name, kind) in
        [("key", EV_KEY), ("rel", EV_REL), ("abs", EV_ABS), ("sw", EV_SW), ("led", EV_LED)]
    {
        let mut bits = [0u8; BITS_MAX];
        let len = about.bits(kind, &mut bits).unwrap_or(0).min(BITS_MAX);
        let used = bits[..len].iter().rposition(|&byte| byte != 0).map_or(0, |last| last + 1);
        write!(out, " {name}=")?;
        for byte in &bits[..used] {
            write!(out, "{byte:02x}")?;
        }
    }
    out.write_str(" name=")?;
    // One line, whatever the device calls itself.
    for character in text(&about.name).chars() {
        out.write_char(if character.is_control() { ' ' } else { character })?;
    }
    out.write_char('\n')
}

#[cfg(test)]
mod tests {
    use alloc::alloc::Global;
    use alloc::string::String;

    use radix_abi::input::{BTN_LEFT, KEY_A, KEY_ESC, LED_CAPSL, REL_WHEEL_HI_RES, REL_X, REL_Y};

    use super::*;
    use crate::device::{About, Few, Keys, padded};
    use crate::hub::Source;

    struct Quiet;

    impl Wake for Quiet {
        fn wake(&self) {}
    }

    #[test]
    fn the_hub_is_described_a_device_to_a_line_with_no_serial() {
        let mut hub: Hub<Quiet, Global> = Hub::new_in(Global);
        let mut keyboard = About::new(BUS_I8042, "AT keyboard", "ps2");
        keyboard.keys = Keys::of(&[KEY_ESC, KEY_A]);
        keyboard.lights = Few::of(&[LED_CAPSL]);
        hub.add(&keyboard, 0).unwrap();
        let mut mouse = About::new(BUS_USB, "USB\tOptical Mouse", "usb-1-2");
        (mouse.vendor, mouse.product, mouse.version, mouse.rate_hz) = (0x046d, 0xc077, 0x111, 125);
        mouse.serial = padded("SECRET");
        mouse.keys = Keys::of(&[BTN_LEFT]);
        mouse.moves = Few::of(&[REL_X, REL_Y, REL_WHEEL_HI_RES]);
        hub.add(&mouse, 0).unwrap();
        hub.open(1, Source::Device(1), Quiet).unwrap();
        hub.open(2, Source::Keyboard, Quiet).unwrap();

        let mut said = String::new();
        hub.describe(&mut said).unwrap();
        let key = |bytes: usize, last: &str| "00".repeat(bytes) + last;
        let lines = [
            String::from("hub readers=2 hold=0"),
            String::from(
                "device 0 bus=ps2 vendor=0000 product=0000 version=0000 rate=0 readers=0 place=ps2 key=020000",
            ) + "40 rel= abs= sw= led=02 name=AT keyboard",
            String::from(
                "device 1 bus=usb vendor=046d product=c077 version=0111 rate=125 readers=1 place=usb-1-2 key=",
            ) + &key(34, "01")
                + " rel=0308 abs= sw= led= name=USB Optical Mouse",
        ];
        assert_eq!(said.lines().collect::<alloc::vec::Vec<_>>(), lines);
        assert!(!said.contains("SECRET"));
    }
}
