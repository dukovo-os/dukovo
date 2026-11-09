// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The controller's registers (High Definition Audio Specification 1.0a,
//! section 3.3), each read and written at its own width, as the
//! specification lays them out: some controllers, and QEMU's, find a
//! register only by its own offset and size.

/// Global capabilities (16 bits): the streams of each kind, and whether
/// the controller reaches memory above 4 GiB.
pub const GCAP: usize = 0x00;
/// GCAP: 64-bit addresses.
pub const GCAP_64OK: u16 = 1 << 0;

/// Global control (32 bits).
pub const GCTL: usize = 0x08;
/// GCTL: out of reset (0 holds the controller and the link in reset).
pub const GCTL_CRST: u32 = 1 << 0;
/// GCTL: accept unsolicited responses.
pub const GCTL_UNSOL: u32 = 1 << 8;

/// Which codecs asked for the link after the reset: one bit for each
/// codec address (16 bits, write 1 to clear).
pub const STATESTS: usize = 0x0e;

/// Interrupt control (32 bits): GIE, CIE and a bit for each stream.
pub const INTCTL: usize = 0x20;

/// The command ring (CORB): its address, its write pointer (16 bits),
/// its read pointer (16 bits), its control and status (8 bits each) and
/// its size (8 bits).
pub const CORBLBASE: usize = 0x40;
pub const CORBUBASE: usize = 0x44;
pub const CORBWP: usize = 0x48;
pub const CORBRP: usize = 0x4a;
/// CORBRP: resets the read pointer.
pub const CORBRP_RST: u16 = 1 << 15;
pub const CORBCTL: usize = 0x4c;
/// CORBCTL: the CORB engine runs.
pub const CORBCTL_RUN: u8 = 1 << 1;
pub const CORBSIZE: usize = 0x4e;

/// The response ring (RIRB): its address, its write pointer (16 bits),
/// how many responses raise an interrupt (16 bits), its control and
/// status (8 bits each) and its size (8 bits).
pub const RIRBLBASE: usize = 0x50;
pub const RIRBUBASE: usize = 0x54;
pub const RIRBWP: usize = 0x58;
/// RIRBWP: resets the write pointer.
pub const RIRBWP_RST: u16 = 1 << 15;
pub const RINTCNT: usize = 0x5a;
pub const RIRBCTL: usize = 0x5c;
/// RIRBCTL: the response flag of RIRBSTS is set when RINTCNT responses
/// have come (an interrupt only if INTCTL lets one through).
pub const RIRBCTL_RINTCTL: u8 = 1 << 0;
/// RIRBCTL: the RIRB engine writes responses.
pub const RIRBCTL_DMAEN: u8 = 1 << 1;
pub const RIRBSTS: usize = 0x5d;
/// RIRBSTS: a response came (bit 0) and the ring overran (bit 2), both
/// write 1 to clear.
pub const RIRBSTS_CLEAR: u8 = 0x05;
pub const RIRBSIZE: usize = 0x5e;

/// The ring sizes the CORBSIZE and RIRBSIZE registers can hold: bits 7:4
/// say which the controller has (2, 16 and 256 entries), bits 1:0 choose
/// one.
pub const SIZE_CAP_16: u8 = 1 << 5;
pub const SIZE_CAP_256: u8 = 1 << 6;

/// The DMA position buffer's address (32 bits each); bit 0 turns it on.
pub const DPLBASE: usize = 0x70;
pub const DPUBASE: usize = 0x74;

/// The first stream descriptor, and the bytes of each. The input
/// streams come first, then the output streams, then the bidirectional
/// ones.
pub const SD_BASE: usize = 0x80;
pub const SD_SIZE: usize = 0x20;

/// A stream descriptor's control (the low 24 bits of a 32-bit register
/// whose top byte is its status, [`SD_STS`]).
pub const SD_CTL: usize = 0x00;
/// SD_CTL: the stream is reset while set.
pub const SD_CTL_SRST: u32 = 1 << 0;
/// SD_CTL: the stream's DMA engine runs.
pub const SD_CTL_RUN: u32 = 1 << 1;
/// SD_CTL: a bidirectional stream plays (rather than records).
pub const SD_CTL_DIR_OUT: u32 = 1 << 19;
/// SD_CTL: where the stream's tag sits (bits 23:20).
pub const SD_CTL_STREAM_SHIFT: u32 = 20;
/// A stream descriptor's status (8 bits; bits 2 to 4 write 1 to clear).
pub const SD_STS: usize = 0x03;
/// SD_STS: every write-1-to-clear bit (buffer completion, FIFO error,
/// descriptor error).
pub const SD_STS_CLEAR: u8 = 0x1c;
/// The link position in the buffer: the bytes of the cyclic buffer the
/// stream has sent on the link (32 bits).
pub const SD_LPIB: usize = 0x04;
/// The cyclic buffer's length in bytes (32 bits).
pub const SD_CBL: usize = 0x08;
/// The last valid index of the buffer descriptor list (16 bits).
pub const SD_LVI: usize = 0x0c;
/// The bytes the DMA engine fetches ahead of the link (16 bits).
pub const SD_FIFOS: usize = 0x10;
/// The stream's format (16 bits), as the codec's converters take it.
pub const SD_FMT: usize = 0x12;
/// The buffer descriptor list's address (32 bits each).
pub const SD_BDPL: usize = 0x18;
pub const SD_BDPU: usize = 0x1c;

/// The bytes of a buffer descriptor list entry: a 64-bit address, a
/// 32-bit length and a 32-bit flag word (bit 0, interrupt on
/// completion).
pub const BDL_ENTRY: usize = 16;
/// The most entries a buffer descriptor list has.
pub const BDL_MAX: usize = 256;

/// The registers' bytes, from GCAP to the end of the last of `streams`
/// stream descriptors: as far as the driver reaches.
pub const fn window_len(streams: usize) -> usize {
    SD_BASE + streams * SD_SIZE
}
