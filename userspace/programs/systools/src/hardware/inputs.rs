// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `hardware inputs` says: every keyboard, mouse, tablet, touch
//! device and switch the kernel reads, with what each is, how it is
//! connected, what it has and how often it is read.
//!
//! All of it is the kernel's own account of its input hub,
//! _/kernel/class/input/state_ (radix-sysfs(5)): a line for the hub,
//! then a line for each device, of `key=value` words after the device's
//! number. What a device is, a keyboard or a pointer, is worked out from
//! the keys and axes it has, by the rules of the `input` library, which
//! inputd(8) and every program that reads a device use too, so that all
//! of them call it the same. Nothing here opens a device or asks a
//! driver or a service anything, so the command works with no console
//! and no desktop running, reads no key, and changes nothing.
//!
//! No serial number of a device is read, and the kernel's account has
//! none, so that the answer can be shown and put in a report as it is.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use input::abi::{LED_CAPSL, LED_NUML, LED_SCROLLL, REL_HWHEEL_HI_RES, REL_WHEEL_HI_RES};
use input::kind::{self, Has};
use serde::Serialize;

use super::words::{Words, line_of};

/// Where the kernel says what its input hub has.
pub(crate) const STATE: &str = "/kernel/class/input/state";

/// A keyboard's lights and their names, in the order they are named in.
const LIGHTS: [(u16, &str); 3] =
    [(LED_NUML, "num-lock"), (LED_CAPSL, "caps-lock"), (LED_SCROLLL, "scroll-lock")];

/// What `hardware inputs --json` says.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Inputs {
    /// The version of this document: 1.
    format: u32,
    /// Whether Ctrl+Alt+Delete has the consoles in front, so that no
    /// program but them reads a key (ADR 0708).
    held: bool,
    /// How many descriptions of input files are open, those of
    /// _/devices/kbd_ and _/devices/mouse_ among them.
    readers: u32,
    devices: Vec<Device>,
    /// Whether the kernel has an input hub at all.
    #[serde(skip)]
    hub: bool,
}

/// An input device.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct Device {
    /// The N of its file, _/devices/input/N_.
    number: u32,
    /// What it calls itself; null if it says nothing.
    name: Option<String>,
    /// What it is: any of `keyboard`, `keys`, `pointer`, `tablet`,
    /// `touch` and `switch`, or `other` alone.
    kinds: Vec<&'static str>,
    /// How it is connected: `usb`, `ps2`, `i2c`, `virtual`, or the
    /// number of a bus that has no name here.
    bus: String,
    /// Its maker's number and its own, `046d:c077`; null where it has
    /// none, as a PS/2 keyboard. Never a serial.
    ids: Option<String>,
    /// Where it is plugged in: `usb-1-2/0`.
    #[serde(rename = "where")]
    place: String,
    /// The reports a second it asks to be read for; null where it sends
    /// when it likes.
    rate_hz: Option<u32>,
    /// How many descriptions of its own file are open.
    readers: u32,
    /// How many keys it has, and how many buttons.
    keys: u32,
    buttons: u32,
    /// Whether it has a wheel, and one that turns sideways.
    wheel: bool,
    sideways_wheel: bool,
    /// Its lights: any of `num-lock`, `caps-lock` and `scroll-lock`.
    lights: Vec<&'static str>,
}

/// The bytes of `hex`, two digits each, as the kernel writes the codes a
/// device has; none of them if any is not two digits.
fn bytes(hex: &str) -> Vec<u8> {
    let byte = |pair: &[u8]| match pair {
        [high, low] if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {
            u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()
        }
        _ => None,
    };
    let bytes: Option<Vec<u8>> = hex.as_bytes().chunks(2).map(byte).collect();
    bytes.unwrap_or_default()
}

/// How many of something, as a number of 32 bits.
fn count(many: usize) -> u32 {
    u32::try_from(many).unwrap_or(u32::MAX)
}

fn device_of(number: u32, words: &Words<'_>) -> Device {
    let bits = |key| bytes(words.text(key).unwrap_or_default());
    let has =
        Has { keys: bits("key"), moves: bits("rel"), places: bits("abs"), switches: bits("sw") };
    let lights = bits("led");
    let hex =
        |key| words.text(key).and_then(|digits| u16::from_str_radix(digits, 16).ok()).unwrap_or(0);
    let (vendor, product) = (hex("vendor"), hex("product"));
    Device {
        number,
        name: words.named("name"),
        kinds: has.kinds().names().collect(),
        bus: words.named("bus").unwrap_or_default(),
        ids: (vendor != 0 || product != 0).then(|| format!("{vendor:04x}:{product:04x}")),
        place: words.named("place").unwrap_or_default(),
        rate_hz: words.said("rate"),
        readers: words.number("readers"),
        keys: count(has.key_count()),
        buttons: count(has.button_count()),
        wheel: kind::has(&has.moves, REL_WHEEL_HI_RES),
        sideways_wheel: kind::has(&has.moves, REL_HWHEEL_HI_RES),
        lights: LIGHTS
            .iter()
            .filter(|(code, _)| kind::has(&lights, *code))
            .map(|light| light.1)
            .collect(),
    }
}

/// Reads what the kernel says of its input hub from `state`. A machine
/// with no such file has no devices here.
pub(crate) fn gather(state: &Path) -> Inputs {
    parse(&fs::read_to_string(state).unwrap_or_default())
}

fn parse(text: &str) -> Inputs {
    let mut inputs = Inputs { format: 1, held: false, readers: 0, devices: Vec::new(), hub: false };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("hub ") {
            let words = Words::of(rest, "");
            inputs.hub = true;
            inputs.held = words.flag("hold");
            inputs.readers = words.number("readers");
        } else if let Some((number, words)) = line_of(line, "device", "name") {
            inputs.devices.push(device_of(number, &words));
        }
    }
    inputs
}

/// How many input devices the kernel's account `state` has, or `None`
/// if it is not an input hub's: for `hardware show`.
pub(crate) fn devices_in(state: &str) -> Option<u32> {
    let found = parse(state);
    found.hub.then(|| count(found.devices.len()))
}

/// `many` of `thing`, in words: `1 key`, `3 buttons`.
fn of(many: u32, thing: &str) -> String {
    format!("{many} {thing}{}", if many == 1 { "" } else { "s" })
}

impl Inputs {
    /// One line of JSON.
    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The text a terminal shows: each device, and under it what it is,
    /// how it is connected, what it has and its file.
    pub(crate) fn text(&self) -> String {
        if self.devices.is_empty() {
            return "No input device: the system reads no keyboard and no mouse.\n".into();
        }
        let mut text = String::new();
        if self.held {
            text.push_str("Ctrl+Alt+Delete has the consoles in front: only they read a key.\n\n");
        }
        for (at, device) in self.devices.iter().enumerate() {
            if at > 0 {
                text.push('\n');
            }
            device.write(&mut text);
        }
        text
    }
}

impl Device {
    fn write(&self, text: &mut String) {
        let name = self.name.as_deref().unwrap_or("A device that does not say what it is");
        let _ = writeln!(text, "{name}");
        let _ = writeln!(text, "  Is:        {}", self.kinds.join(", "));
        let _ = writeln!(text, "  Connected: {}", self.connected());
        let has = self.has();
        if !has.is_empty() {
            let _ = writeln!(text, "  Has:       {has}");
        }
        let open = match self.readers {
            0 => "which no program has open".into(),
            1 => "open once".into(),
            times => format!("open {times} times"),
        };
        let _ = writeln!(text, "  File:      /devices/input/{}, {open}", self.number);
    }

    /// How and where it is connected, and how often it is read.
    fn connected(&self) -> String {
        let mut parts = vec![match self.bus.as_str() {
            "usb" => "USB".to_string(),
            "ps2" => "PS/2".into(),
            "i2c" => "I2C".into(),
            "virtual" => "virtual".into(),
            other => format!("bus {other}"),
        }];
        parts.extend(self.ids.clone());
        // A PS/2 keyboard's place says no more than its bus.
        if !self.place.is_empty() && self.place != self.bus {
            parts.push(format!("at {}", self.place));
        }
        parts.extend(self.rate_hz.map(|rate| format!("read {} a second", of(rate, "time"))));
        parts.join(", ")
    }

    /// Its keys, buttons, wheels and lights, in words.
    fn has(&self) -> String {
        let mut parts = Vec::new();
        if self.keys > 0 {
            parts.push(of(self.keys, "key"));
        }
        if self.buttons > 0 {
            parts.push(of(self.buttons, "button"));
        }
        match (self.wheel, self.sideways_wheel) {
            (true, true) => parts.push("a wheel that also turns sideways".into()),
            (true, false) => parts.push("a wheel".into()),
            (false, true) => parts.push("a sideways wheel".into()),
            (false, false) => {}
        }
        if !self.lights.is_empty() {
            let lights: Vec<String> =
                self.lights.iter().map(|light| light.replace('-', " ")).collect();
            parts.push(format!("lights for {}", lights.join(", ")));
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use input::abi::{ABS_X, ABS_Y, BTN_LEFT, BTN_TOOL_PEN, REL_X, REL_Y};
    use serde_json::{Value, json};

    use super::*;

    /// `codes` as the kernel writes the codes a device has: a bit for
    /// each, and each byte as two digits.
    fn hex(codes: impl IntoIterator<Item = u16>) -> String {
        let mut bytes = Vec::new();
        for code in codes {
            let at = usize::from(code / 8);
            bytes.resize(bytes.len().max(at + 1), 0u8);
            bytes[at] |= 1 << (code % 8);
        }
        bytes.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }

    /// The kernel's account of a machine with a PS/2 keyboard whose file
    /// two descriptions have open, a USB mouse and a pen tablet in a
    /// virtual machine, as `kernel/lib/input/src/report.rs` writes it.
    fn state() -> String {
        // Esc and the three rows of letters.
        let keyboard = hex((1..=1).chain(16..=25).chain(30..=38).chain(44..=50));
        let mouse = hex([BTN_LEFT, BTN_LEFT + 1, BTN_LEFT + 2]);
        let moves = hex([REL_X, REL_Y, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES]);
        let tablet = hex([BTN_LEFT, BTN_TOOL_PEN]);
        let places = hex([ABS_X, ABS_Y]);
        format!(
            "hub readers=4 hold=0\n\
             device 0 bus=ps2 vendor=0000 product=0000 version=0000 rate=0 readers=2 place=ps2 key={keyboard} rel= abs= sw= led=07 name=PS/2 keyboard\n\
             device 2 bus=usb vendor=046d product=c077 version=0111 rate=125 readers=0 place=usb-1-2/0 key={mouse} rel={moves} abs= sw= led= name=USB Optical Mouse\n\
             device 5 bus=virtual vendor=0627 product=0003 version=0002 rate=0 readers=1 place=pci-0000:00:05.0 key={tablet} rel= abs={places} sw= led= name=\n"
        )
    }

    #[test]
    fn each_device_is_read_from_the_kernels_account() {
        let found = parse(&state());
        assert_eq!((found.hub, found.held, found.readers), (true, false, 4));
        let [keyboard, mouse, tablet] = &found.devices[..] else {
            panic!("{:?}", found.devices);
        };
        assert_eq!(keyboard.kinds, ["keyboard"]);
        assert_eq!((keyboard.number, keyboard.bus.as_str(), keyboard.readers), (0, "ps2", 2));
        assert_eq!((&keyboard.ids, keyboard.rate_hz, keyboard.buttons), (&None, None, 0));
        assert_eq!(keyboard.keys, 27);
        assert_eq!(keyboard.lights, ["num-lock", "caps-lock", "scroll-lock"]);

        assert_eq!(mouse.kinds, ["pointer"]);
        assert_eq!(mouse.name.as_deref(), Some("USB Optical Mouse"));
        assert_eq!((mouse.ids.as_deref(), mouse.rate_hz), (Some("046d:c077"), Some(125)));
        assert_eq!((mouse.place.as_str(), mouse.keys, mouse.buttons), ("usb-1-2/0", 0, 3));
        assert!(mouse.wheel && mouse.sideways_wheel && mouse.lights.is_empty());

        assert_eq!((tablet.number, &tablet.name, &tablet.kinds[..]), (5, &None, &["tablet"][..]));
        assert!(!tablet.wheel && !tablet.sideways_wheel);
    }

    #[test]
    fn the_text_says_what_each_device_is_and_has() {
        let text = parse(&state()).text();
        let wanted = "\
PS/2 keyboard
  Is:        keyboard
  Connected: PS/2
  Has:       27 keys; lights for num lock, caps lock, scroll lock
  File:      /devices/input/0, open 2 times

USB Optical Mouse
  Is:        pointer
  Connected: USB, 046d:c077, at usb-1-2/0, read 125 times a second
  Has:       3 buttons; a wheel that also turns sideways
  File:      /devices/input/2, which no program has open

A device that does not say what it is
  Is:        tablet
  Connected: virtual, 0627:0003, at pci-0000:00:05.0
  Has:       2 buttons
  File:      /devices/input/5, open once
";
        assert_eq!(text, wanted);
    }

    #[test]
    fn the_json_is_one_document_with_what_the_text_says() {
        let json = parse(&state()).json().unwrap();
        assert!(!json.contains('\n') && json.starts_with("{\"format\":1,"), "{json}");
        let read: Value = serde_json::from_str(&json).unwrap();
        assert_eq!((&read["held"], &read["readers"]), (&json!(false), &json!(4)));
        assert_eq!(
            read["devices"][1],
            json!({
                "number": 2, "name": "USB Optical Mouse", "kinds": ["pointer"], "bus": "usb",
                "ids": "046d:c077", "where": "usb-1-2/0", "rate_hz": 125, "readers": 0,
                "keys": 0, "buttons": 3, "wheel": true, "sideways_wheel": true, "lights": [],
            })
        );
        assert_eq!(read["devices"][2]["name"], Value::Null);
        // Neither a member nor a word of the hub's own is in it.
        assert!(read.get("hub").is_none());
    }

    #[test]
    fn a_machine_with_no_device_and_the_secure_keys_hold_are_said() {
        let none = parse("hub readers=0 hold=0\n");
        assert_eq!(none.text(), "No input device: the system reads no keyboard and no mouse.\n");
        assert_eq!(devices_in("hub readers=0 hold=0\n"), Some(0));
        // No account at all is not a hub with no device.
        assert_eq!(devices_in(""), None);
        assert_eq!(parse("").text(), none.text());
        assert_eq!(devices_in(&state()), Some(3));

        let held = parse(&state().replace("hold=0", "hold=1"));
        assert!(held.held);
        assert!(held.text().starts_with(
            "Ctrl+Alt+Delete has the consoles in front: only they read a key.\n\nPS/2 keyboard\n"
        ));
    }

    #[test]
    fn a_line_cut_short_or_with_codes_that_are_no_digits_names_no_code() {
        assert_eq!(bytes("0318"), [0x03, 0x18]);
        assert!(bytes("031").is_empty() && bytes("zz18").is_empty() && bytes("").is_empty());
        assert!(bytes("+f").is_empty());
        let found = parse("hub readers=1 hold=0\ndevice 3 bus=24 key=zz name=Odd  one\n");
        let [odd] = &found.devices[..] else { panic!("{:?}", found.devices) };
        assert_eq!((odd.kinds.as_slice(), odd.name.as_deref()), (&["other"][..], Some("Odd  one")));
        assert_eq!(odd.connected(), "bus 24");
        assert_eq!(odd.has(), "");
    }
}
