// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a terminal's settings say of a serial line (Phase 11g, ADR 0246;
//! shared with the PC's own serial port in Phase 14, ADR 0318): its
//! speed, its character size, its parity and stop bits and its flow
//! control, a [`Line`], read from a Linux `c_cflag` and `c_ospeed`
//! ([`Line::from_cflag`]), or from the options of the kernel flag
//! `console=` ([`Line::from_console_options`], Phase 16, ADR 0480). Each
//! serial driver then sets its port as the line says: a USB adapter
//! through its class's or its maker's requests, a 16550 through its
//! divisor and line control register.

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
    /// The receiver is on.
    pub const CREAD: u32 = 0o200;
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
    /// 115200 baud, 8 data bits, no parity, one stop bit: the line GRUB
    /// and the kernel log run a PC's serial ports at unless `console=`
    /// says otherwise.
    pub const DEFAULT: Line =
        Line { rate: 115_200, data_bits: 8, parity: Parity::None, stop: Stop::One, rts_cts: false };

    /// The line Linux's console options say, as `console=ttyS1,57600n8`
    /// gives them: the speed in bits per second, then optionally the
    /// parity, `n`, `o` or `e`, the data bits, `7` or `8`, and `r` for RTS
    /// and CTS flow control, in that order. `None` for a speed Linux has
    /// no code for, or anything else.
    pub fn from_console_options(options: &str) -> Option<Line> {
        let digits = options.find(|c: char| !c.is_ascii_digit()).unwrap_or(options.len());
        let (rate, rest) = options.split_at(digits);
        let rate: u32 = rate.parse().ok()?;
        if rate == 0 || !RATES.iter().any(|&(_, known)| known == rate) {
            return None;
        }
        let mut line = Line { rate, ..Line::DEFAULT };
        let mut rest = rest.chars().peekable();
        if let Some(&parity) = rest.peek()
            && !parity.is_ascii_digit()
            && parity != 'r'
        {
            line.parity = match parity {
                'n' => Parity::None,
                'o' => Parity::Odd,
                'e' => Parity::Even,
                _ => return None,
            };
            rest.next();
        }
        if let Some(&bits) = rest.peek()
            && bits != 'r'
        {
            line.data_bits = match bits {
                '7' => 7,
                '8' => 8,
                _ => return None,
            };
            rest.next();
        }
        match (rest.next(), rest.next()) {
            (None, _) => {}
            (Some('r'), None) => line.rts_cts = true,
            _ => return None,
        }
        Some(line)
    }

    /// The bits of a Linux `c_cflag` that say this line, as
    /// [`Line::from_cflag`] reads them, without `CREAD` and `CLOCAL`;
    /// `None` for a speed Linux has no code for.
    pub fn cflag(&self) -> Option<u32> {
        let code = RATES.iter().find(|&&(_, rate)| rate == self.rate).map(|&(code, _)| code)?;
        let mut flags = code | (u32::from(self.data_bits.clamp(5, 8) - 5) << 4);
        if self.stop != Stop::One {
            flags |= cflag::CSTOPB;
        }
        flags |= match self.parity {
            Parity::None => 0,
            Parity::Odd => cflag::PARENB | cflag::PARODD,
            Parity::Even => cflag::PARENB,
            Parity::Mark => cflag::PARENB | cflag::CMSPAR | cflag::PARODD,
            Parity::Space => cflag::PARENB | cflag::CMSPAR,
        };
        if self.rts_cts {
            flags |= cflag::CRTSCTS;
        }
        Some(flags)
    }

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
