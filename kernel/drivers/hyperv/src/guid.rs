// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The GUIDs that name VMBus's kinds of device.

use core::fmt;

/// A GUID as VMBus has it in memory: its first three fields
/// little-endian, its last two as bytes in order.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Guid(pub [u8; 16]);

/// The value of the hexadecimal digit `digit`.
const fn nibble(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + 10,
        b'A'..=b'F' => digit - b'A' + 10,
        _ => panic!("a GUID holds only hexadecimal digits"),
    }
}

impl Guid {
    /// The GUID written `text`, as `ba6163d9-04a1-4d29-b605-72e2ffb1dc7f`.
    ///
    /// # Panics
    ///
    /// If `text` is not a GUID written so (at compile time, where the
    /// constants below are made).
    pub const fn parse(text: &str) -> Guid {
        let text = text.as_bytes();
        assert!(text.len() == 36, "a GUID is 36 characters long");
        // The bytes in the order they are written, then the first three
        // fields turned around.
        let mut written = [0u8; 16];
        let (mut at, mut byte) = (0, 0);
        while at < 36 {
            if text[at] == b'-' {
                at += 1;
                continue;
            }
            written[byte] = nibble(text[at]) << 4 | nibble(text[at + 1]);
            byte += 1;
            at += 2;
        }
        assert!(byte == 16, "a GUID has 16 bytes");
        let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut bytes = [0u8; 16];
        let mut i = 0;
        while i < 16 {
            bytes[i] = written[order[i]];
            i += 1;
        }
        Guid(bytes)
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-\
             {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12],
            b[13], b[14], b[15]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The synthetic SCSI controller (storvsc), as Generation 2 machines and
/// Azure attach their disks.
pub const STORAGE: Guid = Guid::parse("ba6163d9-04a1-4d29-b605-72e2ffb1dc7f");
/// The synthetic IDE controller's fast path, which a Generation 1
/// machine's emulated IDE disks also have; left to the emulated IDE
/// controller.
pub const IDE: Guid = Guid::parse("32412632-86cb-44a2-9b5c-50d1417354f5");
/// The synthetic network card (netvsc).
pub const NETWORK: Guid = Guid::parse("f8615163-df3e-46c5-913f-f2d2f965ed0e");
/// The synthetic keyboard.
pub const KEYBOARD: Guid = Guid::parse("f912ad6d-2b17-48ea-bd65-f927a61c7684");
/// The synthetic mouse, which the kernel does not drive.
pub const MOUSE: Guid = Guid::parse("cfa8b69e-5b4a-4cc0-b98b-8ba1a1f3f95a");
/// The synthetic video card, which the kernel leaves to the firmware's
/// framebuffer.
pub const VIDEO: Guid = Guid::parse("da0a7802-e377-4aac-8e77-0558eb1073f8");
/// The shutdown integration service.
pub const SHUTDOWN: Guid = Guid::parse("0e0b6031-5213-4934-818b-38d90ced39db");
/// The time synchronization integration service.
pub const TIMESYNC: Guid = Guid::parse("9527e630-d0ae-497b-adce-e80ab0175caf");
/// The heartbeat integration service.
pub const HEARTBEAT: Guid = Guid::parse("57164f39-9115-4e78-ab55-382f3bd5422d");
/// The key-value pair exchange integration service, which the kernel
/// does not drive.
pub const KVP: Guid = Guid::parse("a9a0f4e7-5a45-4d96-b827-8a841e8c03e6");
/// The volume shadow copy integration service, which the kernel does not
/// drive.
pub const VSS: Guid = Guid::parse("35fa2e29-ea23-4236-96ae-3a6ebacba440");
/// Dynamic memory (the balloon), which the kernel does not drive.
pub const DYNAMIC_MEMORY: Guid = Guid::parse("525074dc-8985-46e2-8057-a307dc18a502");
/// The PCI pass-through bus (vPCI), which Azure's accelerated networking
/// gives its MANA and Mellanox cards on.
pub const PCI: Guid = Guid::parse("44c4f61d-4444-4400-9d52-802e27ede19f");

/// What the kernel calls a kind of VMBus device, for the log: `None` for
/// one it does not know.
pub fn name(kind: &Guid) -> Option<&'static str> {
    let names = [
        (STORAGE, "the synthetic SCSI controller"),
        (IDE, "the synthetic IDE controller"),
        (NETWORK, "the synthetic network card"),
        (KEYBOARD, "the synthetic keyboard"),
        (MOUSE, "the synthetic mouse"),
        (VIDEO, "the synthetic video card"),
        (SHUTDOWN, "the shutdown service"),
        (TIMESYNC, "the time service"),
        (HEARTBEAT, "the heartbeat service"),
        (KVP, "the key-value exchange service"),
        (VSS, "the backup service"),
        (DYNAMIC_MEMORY, "dynamic memory"),
        (PCI, "the PCI pass-through bus"),
    ];
    names.iter().find(|(guid, _)| guid == kind).map(|&(_, name)| name)
}
