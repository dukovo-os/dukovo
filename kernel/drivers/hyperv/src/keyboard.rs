// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The synthetic keyboard, which a Generation 2 machine has in place of a
//! PS/2 keyboard.
//!
//! The guest asks for protocol 1.0 (PROTOCOL_REQUEST); the host answers
//! PROTOCOL_RESPONSE with bit 0 set if it takes it. Then each key pressed
//! or let go comes as an EVENT: its make code in scan code set 1, and
//! flags for a release and for the 0xE0 and 0xE1 prefixes, which the
//! kernel turns back into the bytes a PS/2 keyboard would send. An event
//! that carries a UTF-16 character instead (the host's "type clipboard
//! text") is dropped.

/// Messages' types.
const PROTOCOL_REQUEST: u32 = 1;
const PROTOCOL_RESPONSE: u32 = 2;
const EVENT: u32 = 3;

/// Protocol 1.0: minor in the low half, major in the high.
const VERSION: u32 = 1 << 16;
/// The response's bit: the host takes the version.
const ACCEPTED: u32 = 1;

/// An event's flags.
const UNICODE: u32 = 1;
const BREAK: u32 = 2;
const E0: u32 = 4;
const E1: u32 = 8;

/// The request for the protocol, the first message.
pub fn protocol_request() -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[0..4].copy_from_slice(&PROTOCOL_REQUEST.to_le_bytes());
    bytes[4..8].copy_from_slice(&VERSION.to_le_bytes());
    bytes
}

/// What a message from the host says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// The answer to the protocol request: whether the host takes it.
    Protocol {
        /// Whether it does.
        accepted: bool,
    },
    /// A key.
    Key {
        /// The bytes a PS/2 keyboard sends for it, in set 1: the first
        /// `len`.
        bytes: [u8; 2],
        /// How many.
        len: usize,
    },
    /// Something else, or a character the kernel drops.
    Other,
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The message `data`; `None` if it is too short.
pub fn parse(data: &[u8]) -> Option<Message> {
    Some(match u32_at(data, 0)? {
        PROTOCOL_RESPONSE => Message::Protocol { accepted: u32_at(data, 4)? & ACCEPTED != 0 },
        EVENT => {
            // The make code at 4, its flags at 8, as the host lays them.
            let code = data.get(4..6).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
            let flags = u32_at(data, 8)?;
            if flags & UNICODE != 0 {
                return Some(Message::Other);
            }
            let byte = (code & 0x7f) as u8 | if flags & BREAK != 0 { 0x80 } else { 0 };
            let prefix = if flags & E0 != 0 {
                Some(0xe0)
            } else if flags & E1 != 0 {
                Some(0xe1)
            } else {
                None
            };
            match prefix {
                Some(prefix) => Message::Key { bytes: [prefix, byte], len: 2 },
                None => Message::Key { bytes: [byte, 0], len: 1 },
            }
        }
        _ => Message::Other,
    })
}
