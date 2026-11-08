// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a terminal's settings say of a serial line (Phase 11g, ADR 0246;
//! shared with the PC's own serial port in Phase 14, ADR 0318): its
//! speed, its character size, its parity and stop bits and its flow
//! control, a [`Line`], read from a Linux `c_cflag` and `c_ospeed`
//! ([`Line::from_cflag`]). Each serial driver then sets its port as the
//! line says: a USB adapter through its class's or its maker's requests,
//! a 16550 through its divisor and line control register.

/// The bits of a Linux `c_cflag` that say how the line goes.
pub mod cflag {
    /// The speed's code (`CBAUD`), with [`BOTHER`] among its bits.
    pub const CBAUD: u32 = 0o10017;
    /// The speed is the one `c_ospeed` holds, not a code.
    pub const BOTHER: u32 = 0o10000;
    /// The character size (`CSIZE`): `CS5` is 0, `CS6`, `CS7`, `CS8`.
    pub const CSIZE: u32 = 0o60;
    /// Two stop bits.
    pub const CSTOPB: u32 = 0o100;
    /// A parity bit is sent and checked.
    pub const PARENB: u32 = 0o400;
    /// Odd parity (with [`CMSPAR`], mark).
    pub const PARODD: u32 = 0o1000;
    /// The modem lines are lowered on the last close.
    pub const HUPCL: u32 = 0o2000;
    /// The modem's carrier is not waited for.
    pub const CLOCAL: u32 = 0o4000;
    /// Mark or space parity.
    pub const CMSPAR: u32 = 0o10000000000;
    /// RTS and CTS flow control.
    pub const CRTSCTS: u32 = 0o20000000000;
}

/// Linux's speed codes, by code, and the bits per second each is.
const RATES: [(u32, u32); 31] = [
    (0o0, 0),
    (0o1, 50),
    (0o2, 75),
    (0o3, 110),
    (0o4, 134),
    (0o5, 150),
    (0o6, 200),
    (0o7, 300),
    (0o10, 600),
    (0o11, 1200),
    (0o12, 1800),
    (0o13, 2400),
    (0o14, 4800),
    (0o15, 9600),
    (0o16, 19200),
    (0o17, 38400),
    (0o10001, 57600),
    (0o10002, 115_200),
    (0o10003, 230_400),
    (0o10004, 460_800),
    (0o10005, 500_000),
    (0o10006, 576_000),
    (0o10007, 921_600),
    (0o10010, 1_000_000),
    (0o10011, 1_152_000),
    (0o10012, 1_500_000),
    (0o10013, 2_000_000),
    (0o10014, 2_500_000),
    (0o10015, 3_000_000),
    (0o10016, 3_500_000),
    (0o10017, 4_000_000),
];

/// Linux's B9600, the speed a serial port starts at.
pub const B9600: u32 = 0o15;

/// Linux's B115200, the speed of the PC's first serial port, which the
/// kernel's log and GRUB already use when a program opens it.
pub const B115200: u32 = 0o10002;

/// The bits per second of Linux's speed code `code`, if it is one.
pub fn rate_of(code: u32) -> Option<u32> {
    RATES.iter().find(|&&(c, _)| c == code).map(|&(_, rate)| rate)
}

/// A parity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    /// No parity bit.
    None,
    /// The bit makes the ones odd.
    Odd,
    /// The bit makes the ones even.
    Even,
    /// The bit is always 1.
    Mark,
    /// The bit is always 0.
    Space,
}

/// How many stop bits end a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// One.
    One,
    /// One and a half, which a 5-bit character has instead of two.
    OneAndHalf,
    /// Two.
    Two,
}

/// How a line goes, as a terminal's settings say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    /// Bits per second; 0 hangs the line up (B0), lowering DTR and RTS.
    pub rate: u32,
    /// Bits in a character, 5 to 8.
    pub data_bits: u8,
    /// Its parity.
    pub parity: Parity,
    /// Its stop bits.
    pub stop: Stop,
    /// RTS and CTS flow control.
    pub rts_cts: bool,
}

impl Line {
    /// The line a Linux `c_cflag` says, with `ospeed` the speed in bits per
    /// second if the cflag's code is [`cflag::BOTHER`]; a code Linux does
    /// not have is 9600.
    pub fn from_cflag(flags: u32, ospeed: u32) -> Line {
        let code = flags & cflag::CBAUD;
        let rate = if code == cflag::BOTHER { ospeed } else { rate_of(code).unwrap_or(9600) };
        let data_bits = 5 + ((flags & cflag::CSIZE) >> 4) as u8;
        let parity = match (
            flags & cflag::PARENB != 0,
            flags & cflag::CMSPAR != 0,
            flags & cflag::PARODD != 0,
        ) {
            (false, ..) => Parity::None,
            (true, false, true) => Parity::Odd,
            (true, false, false) => Parity::Even,
            (true, true, true) => Parity::Mark,
            (true, true, false) => Parity::Space,
        };
        let stop = match (flags & cflag::CSTOPB != 0, data_bits) {
            (false, _) => Stop::One,
            (true, 5) => Stop::OneAndHalf,
            (true, _) => Stop::Two,
        };
        Line { rate, data_bits, parity, stop, rts_cts: flags & cflag::CRTSCTS != 0 }
    }
}
