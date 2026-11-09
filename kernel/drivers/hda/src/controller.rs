// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The controller: its reset, which wakes the codecs on its link, and
//! the two rings that carry commands to them (the CORB) and their
//! responses back (the RIRB), in one page of DMA memory (sections 4.2 to
//! 4.4).
//!
//! The driver sends one command at a time and waits for its response by
//! looking at the RIRB's write pointer, every 10 microseconds for at most
//! 100 ms, as FreeBSD's hdac(4) does. Codecs answer in microseconds, and
//! commands come only when the output is set up, started, stopped or its
//! volume or jacks change, so nothing is gained from an interrupt for
//! them. Unsolicited responses are left off: the jacks are read when the
//! output is looked at ([`crate::Hda::look`]).

use radix_hw::{Dma, Mmio, device_barrier};

use crate::regs::*;
use crate::{Error, Host};

/// Where the RIRB starts in the rings' page: the CORB's 256 entries of 4
/// bytes come first.
const RIRB_OFFSET: usize = 2048;

/// How many times, 10 microseconds apart, the driver looks for a
/// response before it gives up: 100 ms.
const RESPONSE_LOOKS: u32 = 10_000;

/// How many times, 10 microseconds apart, the driver looks for the
/// controller to enter or leave its reset: 100 ms, as FreeBSD's hdac(4).
const RESET_LOOKS: u32 = 10_000;

/// Where a stream sits and how it plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSlot {
    /// The offset of its descriptor's registers.
    pub offset: usize,
    /// Whether it is a bidirectional stream, which plays only with
    /// SD_CTL's direction bit set.
    pub bidirectional: bool,
}

/// An HDA controller, out of reset, with its rings running.
pub struct Controller {
    mmio: Mmio,
    /// The CORB, then the RIRB from [`RIRB_OFFSET`].
    rings: Dma,
    corb_entries: u16,
    rirb_entries: u16,
    /// The last CORB entry written.
    corb_wp: u16,
    /// The last RIRB entry read.
    rirb_rp: u16,
    gcap: u16,
    /// STATESTS after the reset: one bit for each codec address that
    /// asked for the link.
    codecs: u16,
    /// Commands in a row that got no response.
    missed: u32,
}

/// The entries of the largest ring a size register says the controller
/// has (256, 16 or, with neither bit, 2), and the value that chooses it.
fn ring_size(register: u8) -> (u16, u8) {
    if register & SIZE_CAP_256 != 0 {
        (256, 2)
    } else if register & SIZE_CAP_16 != 0 {
        (16, 1)
    } else {
        // Controllers with no bits set have the two-entry ring only.
        (2, 0)
    }
}

impl Controller {
    /// Resets the controller at `mmio`, which wakes its codecs, and
    /// starts its rings in memory from `host`.
    pub fn new(mmio: Mmio, host: &mut dyn Host) -> Result<Controller, Error> {
        let gcap = mmio.read16(GCAP);
        if gcap == 0xffff {
            return Err(Error::Gone);
        }
        // Every stream descriptor GCAP counts must be within the
        // registers mapped, or the driver would reach past them.
        let streams = usize::from((gcap >> 8) & 0xf)
            + usize::from((gcap >> 12) & 0xf)
            + usize::from((gcap >> 3) & 0x1f);
        if mmio.len() < window_len(streams) {
            return Err(Error::Gone);
        }
        let low = gcap & GCAP_64OK == 0;
        let rings = host.alloc(1, low).ok_or(Error::NoMemory)?;
        let mut controller = Controller {
            mmio,
            rings,
            corb_entries: 0,
            rirb_entries: 0,
            corb_wp: 0,
            rirb_rp: 0,
            gcap,
            codecs: 0,
            missed: 0,
        };
        controller.reset(host)?;
        controller.start_rings(host)?;
        Ok(controller)
    }

    /// How many streams of each kind the controller has: input, output
    /// and bidirectional.
    fn streams(&self) -> (usize, usize, usize) {
        let gcap = usize::from(self.gcap);
        ((gcap >> 8) & 0xf, (gcap >> 12) & 0xf, (gcap >> 3) & 0x1f)
    }

    /// Whether the controller reaches memory above 4 GiB.
    pub fn wide(&self) -> bool {
        self.gcap & GCAP_64OK != 0
    }

    /// The stream the output plays on: the first output stream, or the
    /// first bidirectional one if there is none.
    pub fn output_stream(&self) -> Option<StreamSlot> {
        let (input, output, both) = self.streams();
        let (index, bidirectional) = if output > 0 {
            (input, false)
        } else if both > 0 {
            (input + output, true)
        } else {
            return None;
        };
        Some(StreamSlot { offset: SD_BASE + index * SD_SIZE, bidirectional })
    }

    /// The codec addresses on the link: a bit for each.
    pub fn codecs(&self) -> u16 {
        self.codecs
    }

    /// The registers.
    pub fn mmio(&self) -> &Mmio {
        &self.mmio
    }

    /// Stops every stream and both rings, and puts the controller in
    /// reset, as the driver leaves it.
    pub fn halt(&self, host: &mut dyn Host) {
        self.stop_engines(host);
        let gctl = self.mmio.read32(GCTL);
        self.mmio.write32(GCTL, gctl & !GCTL_CRST);
    }

    /// Stops every stream's DMA and both rings'.
    fn stop_engines(&self, host: &mut dyn Host) {
        let (input, output, both) = self.streams();
        for index in 0..input + output + both {
            let offset = SD_BASE + index * SD_SIZE;
            self.mmio.write32(offset + SD_CTL, 0);
        }
        self.mmio.write8(CORBCTL, 0);
        self.mmio.write8(RIRBCTL, 0);
        // The engines stop within a few microseconds; wait until they say
        // so, so that nothing reaches the rings' memory afterwards.
        for _ in 0..RESET_LOOKS {
            let corb = self.mmio.read8(CORBCTL) & CORBCTL_RUN;
            let rirb = self.mmio.read8(RIRBCTL) & RIRBCTL_DMAEN;
            if corb == 0 && rirb == 0 {
                break;
            }
            host.delay_us(10);
        }
        self.mmio.write32(DPLBASE, 0);
        self.mmio.write32(DPUBASE, 0);
    }

    /// Resets the controller and the link: CRST held low for at least
    /// 100 microseconds, then high, then a millisecond for the codecs to
    /// ask for the link (section 4.3, at least 521 microseconds).
    fn reset(&mut self, host: &mut dyn Host) -> Result<(), Error> {
        self.stop_engines(host);
        let gctl = self.mmio.read32(GCTL);
        self.mmio.write32(GCTL, gctl & !GCTL_CRST);
        if !self.wait_gctl(false, host) {
            return Err(Error::StuckInReset);
        }
        host.delay_us(100);
        let gctl = self.mmio.read32(GCTL);
        self.mmio.write32(GCTL, (gctl | GCTL_CRST) & !GCTL_UNSOL);
        if !self.wait_gctl(true, host) {
            return Err(Error::StuckInReset);
        }
        host.delay_us(1000);
        self.codecs = self.mmio.read16(STATESTS) & 0x7fff;
        self.mmio.write16(STATESTS, self.codecs);
        // No interrupts: the kernel looks at the streams.
        self.mmio.write32(INTCTL, 0);
        Ok(())
    }

    /// Waits until GCTL's CRST reads `set`.
    fn wait_gctl(&self, set: bool, host: &mut dyn Host) -> bool {
        for _ in 0..RESET_LOOKS {
            if (self.mmio.read32(GCTL) & GCTL_CRST != 0) == set {
                return true;
            }
            host.delay_us(10);
        }
        false
    }

    /// Sets the rings up in their page and starts them.
    fn start_rings(&mut self, host: &mut dyn Host) -> Result<(), Error> {
        let corb = self.rings.phys();
        let rirb = corb + RIRB_OFFSET as u64;
        let (corb_entries, corb_size) = ring_size(self.mmio.read8(CORBSIZE));
        let (rirb_entries, rirb_size) = ring_size(self.mmio.read8(RIRBSIZE));
        self.corb_entries = corb_entries;
        self.rirb_entries = rirb_entries;
        // The CORB: its size, its address, and both pointers at 0. The
        // read pointer's reset bit should read back as 1 and then 0
        // (section 3.3.21); some controllers never show it, and some
        // keep it until it is cleared, so the driver waits a little for
        // each and goes on regardless.
        self.mmio.write8(CORBSIZE, corb_size);
        self.mmio.write32(CORBLBASE, corb as u32);
        self.mmio.write32(CORBUBASE, (corb >> 32) as u32);
        self.mmio.write16(CORBWP, 0);
        self.corb_wp = 0;
        self.mmio.write16(CORBRP, CORBRP_RST);
        self.wait_corbrp(true, host);
        self.mmio.write16(CORBRP, 0);
        self.wait_corbrp(false, host);
        // The RIRB: its size, its address, its write pointer at 0, and an
        // interrupt count of one (none is raised: RINTCTL stays off).
        self.mmio.write8(RIRBSIZE, rirb_size);
        self.mmio.write32(RIRBLBASE, rirb as u32);
        self.mmio.write32(RIRBUBASE, (rirb >> 32) as u32);
        self.mmio.write16(RIRBWP, RIRBWP_RST);
        self.rirb_rp = 0;
        // Each response sets RIRBSTS's response flag, which the driver
        // clears as it reads them. QEMU's controller takes no more commands
        // once RINTCNT responses have come until the flag is cleared, and
        // sets the flag only while RINTCTL is on; the interrupt it would
        // raise never reaches the CPU, as INTCTL stays 0.
        self.mmio.write16(RINTCNT, 1);
        self.mmio.write8(RIRBSTS, RIRBSTS_CLEAR);
        device_barrier();
        self.mmio.write8(RIRBCTL, RIRBCTL_DMAEN | RIRBCTL_RINTCTL);
        self.mmio.write8(CORBCTL, CORBCTL_RUN);
        for _ in 0..RESET_LOOKS {
            if self.mmio.read8(CORBCTL) & CORBCTL_RUN != 0 {
                return Ok(());
            }
            host.delay_us(10);
        }
        Err(Error::StuckInReset)
    }

    /// Waits up to a millisecond for CORBRP's reset bit to read `set`.
    fn wait_corbrp(&self, set: bool, host: &mut dyn Host) {
        for _ in 0..100 {
            if (self.mmio.read16(CORBRP) & CORBRP_RST != 0) == set {
                return;
            }
            host.delay_us(10);
        }
    }

    /// Sends `verb` (the 20 bits after the address and the node: see
    /// [`crate::verb`]) to node `nid` of codec `cad`, and returns its
    /// response.
    pub fn command(
        &mut self,
        cad: u8,
        nid: u8,
        verb: u32,
        host: &mut dyn Host,
    ) -> Result<u32, Error> {
        let word = (u32::from(cad & 0xf) << 28) | (u32::from(nid) << 20) | (verb & 0xf_ffff);
        self.corb_wp = (self.corb_wp + 1) % self.corb_entries;
        self.rings.write32(usize::from(self.corb_wp) * 4, word);
        device_barrier();
        self.mmio.write16(CORBWP, self.corb_wp);
        for _ in 0..RESPONSE_LOOKS {
            if let Some(response) = self.take_response(cad)? {
                self.missed = 0;
                return Ok(response);
            }
            host.delay_us(10);
        }
        self.missed = self.missed.saturating_add(1);
        Err(Error::NoAnswer)
    }

    /// The response from codec `cad`, if one came: reads every entry the
    /// RIRB gained, passing over unsolicited ones and any for other
    /// codecs (a response that came after its command was given up on).
    fn take_response(&mut self, cad: u8) -> Result<Option<u32>, Error> {
        let wp = self.mmio.read16(RIRBWP);
        if wp == 0xffff {
            return Err(Error::Gone);
        }
        let wp = wp & 0xff;
        let mut found = None;
        while self.rirb_rp != wp % self.rirb_entries {
            self.rirb_rp = (self.rirb_rp + 1) % self.rirb_entries;
            device_barrier();
            let at = RIRB_OFFSET + usize::from(self.rirb_rp) * 8;
            let response = self.rings.read32(at);
            let extended = self.rings.read32(at + 4);
            let unsolicited = extended & (1 << 4) != 0;
            // The last one is the answer to the command just sent.
            if !unsolicited && (extended & 0xf) as u8 == cad & 0xf {
                found = Some(response);
            }
        }
        self.mmio.write8(RIRBSTS, RIRBSTS_CLEAR);
        Ok(found)
    }

    /// Whether the controller stopped answering: commands in a row went
    /// unanswered, or its registers read as all ones.
    pub fn failed(&self) -> bool {
        self.missed >= 3 || self.mmio.read16(GCAP) == 0xffff
    }
}
