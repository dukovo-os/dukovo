// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The registers both interfaces share, the firmware's states, and the
//! way to the ready state.
//!
//! The firmware says what it is doing in the top four bits of a status
//! register (*scratch pad 0* on every part but the SAS1078, which uses its
//! first outbound message register), with its command and scatter-gather
//! limits below them. The host moves it along by writing bits to a
//! doorbell: at offset 0 on the skinny and fusion parts, at 0x20 on the
//! others.

use radix_hw::Mmio;

use crate::{Error, Family};

/// The registers, by their offsets in the memory BAR.
pub mod reg {
    /// The doorbell of the skinny and fusion parts.
    pub const DOORBELL: usize = 0x00;
    /// The SAS1078's status register (its first outbound message).
    pub const OUTBOUND_MESSAGE_0: usize = 0x18;
    /// The doorbell of the SAS1078 and SAS2108.
    pub const INBOUND_DOORBELL: usize = 0x20;
    /// The interrupts the controller raised.
    pub const OUTBOUND_STATUS: usize = 0x30;
    /// The interrupts it may not raise: a set bit masks one.
    pub const OUTBOUND_MASK: usize = 0x34;
    /// MFI: where a frame is posted, its address and size in one word.
    pub const INBOUND_QUEUE: usize = 0x40;
    /// Fusion: the index of the next reply the host will read, for the
    /// reply queue of interrupt vector 0 when the vectors are not combined.
    pub const REPLY_INDEX: usize = 0x6c;
    /// Clears raised interrupts, on the MFI parts but the skinny ones.
    pub const OUTBOUND_DOORBELL_CLEAR: usize = 0xa0;
    /// The firmware's status, on every part but the SAS1078.
    pub const SCRATCH_PAD_0: usize = 0xb0;
    /// Fusion: the chain frame size and the interrupt vectors.
    pub const SCRATCH_PAD_2: usize = 0xb4;
    /// The low half of a request: an MFI frame on the skinny parts, a
    /// request descriptor on the fusion ones.
    pub const INBOUND_LOW: usize = 0xc0;
    /// The high half, written first on the skinny parts and second on the
    /// fusion ones.
    pub const INBOUND_HIGH: usize = 0xc4;
    /// Fusion: the reply index of the first eight vectors when the
    /// firmware combines them, one register for each eight.
    pub const REPLY_INDEX_COMBINED: usize = 0x30c;
}

/// How much of the BAR the driver uses.
pub const REGISTERS: usize = 0x310;

/// Bits written to the doorbell.
pub mod doorbell {
    /// Abort every command.
    pub const ABORT: u32 = 0x01;
    /// Go from operational back to ready.
    pub const READY: u32 = 0x02;
    /// Drop the frames posted.
    pub const MFI_MODE: u32 = 0x04;
    /// Answer the firmware waiting for a handshake.
    pub const CLEAR_HANDSHAKE: u32 = 0x08;
    /// Answer a boot message.
    pub const HOTPLUG: u32 = 0x10;
    /// What sends an operational firmware back to ready.
    pub const RESET: u32 = READY | MFI_MODE | ABORT;
}

/// What the firmware is doing: the top four bits of its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not yet saying.
    Undefined,
    /// Starting its boot block.
    BootBlock,
    /// Starting.
    Starting,
    /// Waiting for the host to answer a handshake.
    WaitHandshake,
    /// Still starting, a later stage.
    StartingLate,
    /// Looking for disks.
    Scanning,
    /// Waiting for the host to answer a boot message.
    BootMessage,
    /// Flushing its cache.
    Flushing,
    /// Ready to be given its queues.
    Ready,
    /// Running, with its queues.
    Operational,
    /// Stopped by a fault.
    Fault,
    /// A state this driver does not know.
    Other(u8),
}

impl State {
    /// The state in the status word `status`.
    pub fn of(status: u32) -> State {
        match (status >> 28) as u8 {
            0x0 => State::Undefined,
            0x1 => State::BootBlock,
            0x4 => State::Starting,
            0x6 => State::WaitHandshake,
            0x7 => State::StartingLate,
            0x8 => State::Scanning,
            0x9 => State::BootMessage,
            0xa => State::Flushing,
            0xb => State::Ready,
            0xc => State::Operational,
            0xf => State::Fault,
            other => State::Other(other),
        }
    }
}

/// The firmware's status word, read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status(pub u32);

impl Status {
    /// What the firmware is doing.
    pub fn state(self) -> State {
        State::of(self.0)
    }

    /// How many commands the firmware takes at once.
    pub fn max_commands(self) -> u16 {
        self.0 as u16
    }

    /// The most scatter-gather entries an MFI command may have.
    pub fn max_sge(self) -> u8 {
        (self.0 >> 16) as u8
    }

    /// Whether the controller can interrupt through MSI-X.
    pub fn msix(self) -> bool {
        self.0 & (1 << 26) != 0
    }

    /// The fault code, when the state is [`State::Fault`].
    pub fn fault_code(self) -> u32 {
        self.0 & 0xffff
    }
}

/// The registers of one controller.
#[derive(Clone, Debug)]
pub struct Registers {
    mmio: Mmio,
    family: Family,
}

/// How long the firmware may stay in one state on its way to ready, in
/// milliseconds, as the controllers' makers give it.
pub const STATE_TIMEOUT_MS: u32 = 180_000;
/// How often its state is read while waiting, in milliseconds.
const POLL_MS: u32 = 10;

impl Registers {
    /// The registers `mmio` of a controller of `family`.
    pub fn new(mmio: Mmio, family: Family) -> Result<Registers, Error> {
        if mmio.len() < REGISTERS {
            return Err(Error::Registers);
        }
        Ok(Registers { mmio, family })
    }

    /// The controller's family.
    pub fn family(&self) -> Family {
        self.family
    }

    /// The window of registers.
    pub fn mmio(&self) -> &Mmio {
        &self.mmio
    }

    /// The firmware's status.
    pub fn status(&self) -> Status {
        let at = match self.family {
            Family::Mfi1078 => reg::OUTBOUND_MESSAGE_0,
            _ => reg::SCRATCH_PAD_0,
        };
        Status(self.mmio.read32(at))
    }

    /// Rings the doorbell with `bits`.
    pub fn ring(&self, bits: u32) {
        let at = match self.family {
            Family::Mfi1078 | Family::MfiGen2 => reg::INBOUND_DOORBELL,
            _ => reg::DOORBELL,
        };
        self.mmio.write32(at, bits);
    }

    /// Whether the doorbell still holds a reset the firmware has not
    /// taken (fusion parts clear bit 0 when they have).
    fn reset_pending(&self) -> bool {
        self.family.fusion() && self.mmio.read32(reg::DOORBELL) & 1 != 0
    }

    /// Lets the controller interrupt when a command ends.
    pub fn enable_interrupts(&self) {
        let regs = &self.mmio;
        match self.family {
            Family::Mfi1078 => {
                regs.write32(reg::OUTBOUND_DOORBELL_CLEAR, u32::MAX);
                regs.write32(reg::OUTBOUND_MASK, !0x8000_0000);
            }
            Family::MfiGen2 => {
                regs.write32(reg::OUTBOUND_DOORBELL_CLEAR, u32::MAX);
                regs.write32(reg::OUTBOUND_MASK, !0x5);
            }
            Family::MfiSkinny => regs.write32(reg::OUTBOUND_MASK, !0x1),
            _ => {
                regs.write32(reg::OUTBOUND_STATUS, u32::MAX);
                regs.write32(reg::OUTBOUND_MASK, !0x9);
            }
        }
        let _ = regs.read32(reg::OUTBOUND_MASK);
    }

    /// Stops the controller interrupting. On QEMU's MFI models this
    /// also resets the controller, so it is done only before its queues
    /// are given or when it is given up.
    pub fn disable_interrupts(&self) {
        self.mmio.write32(reg::OUTBOUND_MASK, u32::MAX);
        let _ = self.mmio.read32(reg::OUTBOUND_MASK);
    }

    /// Clears the interrupt the controller raised, if it raised one, and
    /// says whether it did. A controller that interrupts through MSI or
    /// MSI-X may not say, so then the replies are read whatever this
    /// answers.
    pub fn acknowledge(&self) -> bool {
        let regs = &self.mmio;
        let raised = regs.read32(reg::OUTBOUND_STATUS);
        let (ours, clear) = match self.family {
            Family::Mfi1078 => (raised & 0x8000_0000, reg::OUTBOUND_DOORBELL_CLEAR),
            Family::MfiGen2 => (raised & 0x5, reg::OUTBOUND_DOORBELL_CLEAR),
            Family::MfiSkinny => (raised & 0x1, reg::OUTBOUND_STATUS),
            _ => (raised & 0x9, reg::OUTBOUND_STATUS),
        };
        if ours == 0 {
            return false;
        }
        regs.write32(clear, raised);
        let _ = regs.read32(reg::OUTBOUND_STATUS);
        true
    }

    /// Brings the firmware to its ready state, answering what it waits
    /// for and sending it back from operational (where a boot loader or
    /// an earlier kernel left it), with `wait` sleeping the milliseconds
    /// it is given. Fails if it faults or stays in one state for
    /// [`STATE_TIMEOUT_MS`].
    pub fn make_ready(&self, wait: &mut dyn FnMut(u32)) -> Result<Status, Error> {
        loop {
            let status = self.status();
            match status.state() {
                State::Ready => return Ok(status),
                State::Fault => return Err(Error::Fault(status.fault_code())),
                State::WaitHandshake => {
                    let bits = match self.family {
                        Family::Mfi1078 | Family::MfiGen2 => doorbell::CLEAR_HANDSHAKE,
                        _ => doorbell::CLEAR_HANDSHAKE | doorbell::HOTPLUG,
                    };
                    self.ring(bits);
                }
                State::BootMessage => self.ring(doorbell::HOTPLUG),
                State::Operational => {
                    self.disable_interrupts();
                    self.ring(doorbell::RESET);
                    let mut waited = 0;
                    while self.reset_pending() && waited < 10_000 {
                        wait(POLL_MS);
                        waited += POLL_MS;
                    }
                }
                State::Other(_) => return Err(Error::NotReady(status.0)),
                _ => {}
            }
            // The whole word changes when the firmware moves on, if only
            // its limits below the state.
            let mut waited = 0;
            loop {
                let now = self.status();
                if now != status {
                    break;
                }
                if waited >= STATE_TIMEOUT_MS {
                    return Err(Error::NotReady(now.0));
                }
                wait(POLL_MS);
                waited += POLL_MS;
            }
        }
    }

    /// Fusion: the second scratch pad.
    pub fn scratch_pad_2(&self) -> u32 {
        self.mmio.read32(reg::SCRATCH_PAD_2)
    }

    /// Reads 32 bits at `offset`.
    pub fn read32(&self, offset: usize) -> u32 {
        self.mmio.read32(offset)
    }

    /// Writes 32 bits at `offset`.
    pub fn write32(&self, offset: usize, value: u32) {
        self.mmio.write32(offset, value);
    }
}
