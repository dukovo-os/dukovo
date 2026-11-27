// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Classic BPF on frames: the packet filters libpcap compiles from what
//! tcpdump(8) is asked for (`udp port 53`), which the kernel runs on each
//! frame a packet capture sees (ADR 0346), as Linux runs those
//! SO_ATTACH_FILTER attaches and the BSDs those BIOCSETF sets, so that a
//! frame the filter does not want is never copied.
//!
//! The machine, its checks and its interpreter are radix-bpf's, which
//! seccomp filters run on too; this is what a packet filter reads, a
//! [`Packet`], whose loads are big-endian, as the network's numbers are.
//! A load past the frame's end returns 0. Linux's extensions, at the
//! negative offsets of SKF_AD_OFF, are not loads this machine has:
//! libpcap compiles them only for Linux.

use radix_bpf::Input;
pub use radix_bpf::{INSTRUCTION, Instruction, Invalid, MAX_INSTRUCTIONS, MEMORY, check};

/// A frame as a program sees it: `head`, bytes the kernel puts before
/// it (the Ethernet header it gives a loopback packet), then `body`, the
/// frame itself.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    /// What comes before the frame.
    pub head: &'a [u8],
    /// The frame.
    pub body: &'a [u8],
}

impl Packet<'_> {
    /// Its length.
    pub fn len(&self) -> usize {
        self.head.len() + self.body.len()
    }

    /// Whether it has no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn byte(&self, at: usize) -> u8 {
        match at.checked_sub(self.head.len()) {
            None => self.head[at],
            Some(at) => self.body[at],
        }
    }
}

impl Input for Packet<'_> {
    fn length(&self) -> u32 {
        // A frame is shorter than 4 GiB.
        Packet::len(self) as u32
    }

    /// The `size` bytes at `offset`, big-endian, if they are all there.
    fn load(&self, offset: u64, size: usize) -> Option<u32> {
        let at = usize::try_from(offset).ok()?;
        if at.checked_add(size)? > Packet::len(self) {
            return None;
        }
        Some((at..at + size).fold(0, |word, i| (word << 8) | u32::from(self.byte(i))))
    }
}

/// Runs `program`, which [`check`] took, on `packet`: how many of its
/// bytes to keep, 0 for none.
pub fn run(program: &[Instruction], packet: Packet<'_>) -> u32 {
    radix_bpf::run(program, &packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn i(code: u16, k: u32) -> Instruction {
        Instruction::new(code, k)
    }

    const fn j(code: u16, k: u32, jt: u8, jf: u8) -> Instruction {
        Instruction::jump(code, k, jt, jf)
    }

    /// `tcpdump -dd udp port 53`, as libpcap 1.10 compiles it for an
    /// Ethernet link.
    const UDP_53: [Instruction; 18] = [
        i(0x28, 12),
        j(0x15, 0x86dd, 0, 6),
        i(0x30, 20),
        j(0x15, 0x11, 0, 15),
        i(0x28, 54),
        j(0x15, 53, 12, 0),
        i(0x28, 56),
        j(0x15, 53, 10, 11),
        j(0x15, 0x0800, 0, 10),
        i(0x30, 23),
        j(0x15, 0x11, 0, 8),
        i(0x28, 20),
        j(0x45, 0x1fff, 6, 0),
        i(0xb1, 14),
        i(0x48, 14),
        j(0x15, 53, 2, 0),
        i(0x48, 16),
        j(0x15, 53, 0, 1),
    ];

    /// UDP_53, with its two returns.
    fn udp_53() -> [Instruction; 20] {
        let mut program = [i(0x06, 0); 20];
        program[..18].copy_from_slice(&UDP_53);
        program[18] = i(0x06, 262_144);
        program[19] = i(0x06, 0);
        program
    }

    /// An Ethernet frame holding an IPv4 UDP datagram from port `from` to
    /// port `to`, with `options` words of IP options.
    fn udp_frame(from: u16, to: u16, options: usize) -> ([u8; 64], usize) {
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45 + options as u8;
        frame[23] = 17;
        let udp = 14 + 20 + 4 * options;
        frame[udp..udp + 2].copy_from_slice(&from.to_be_bytes());
        frame[udp + 2..udp + 4].copy_from_slice(&to.to_be_bytes());
        (frame, udp + 8)
    }

    fn whole(frame: &[u8]) -> Packet<'_> {
        Packet { head: &[], body: frame }
    }

    #[test]
    fn libpcaps_program_finds_the_port() {
        let program = udp_53();
        check(&program).unwrap();
        let (frame, len) = udp_frame(40000, 53, 0);
        assert_eq!(run(&program, whole(&frame[..len])), 262_144);
        let (frame, len) = udp_frame(53, 40000, 0);
        assert_eq!(run(&program, whole(&frame[..len])), 262_144);
        let (frame, len) = udp_frame(40000, 80, 0);
        assert_eq!(run(&program, whole(&frame[..len])), 0);
        // Past the options, by the header's length (MSH).
        let (frame, len) = udp_frame(40000, 53, 2);
        assert_eq!(run(&program, whole(&frame[..len])), 262_144);
        // A frame cut before its ports is not one it wants.
        let (frame, _) = udp_frame(40000, 53, 0);
        assert_eq!(run(&program, whole(&frame[..36])), 0);
    }

    #[test]
    fn a_head_comes_before_the_frame() {
        let program = [i(0x20, 0), j(0x15, 0x0102_0304, 0, 1), i(0x06, 9), i(0x06, 0)];
        check(&program).unwrap();
        // The word spans the head and the frame.
        let packet = Packet { head: &[1, 2], body: &[3, 4] };
        assert_eq!(run(&program, packet), 9);
        let packet = Packet { head: &[1, 2, 3], body: &[5] };
        assert_eq!(run(&program, packet), 0);
    }

    #[test]
    fn loads_past_the_end_return_nothing() {
        for code in [0x20, 0x28, 0x30] {
            let program = [i(code, 3), i(0x06, 1)];
            assert_eq!(run(&program, whole(&[0; 3])), 0, "{code:#x}");
        }
        // X + k past the end, or past 4 GiB.
        let program = [i(0x01, u32::MAX), i(0x50, 2), i(0x06, 1)];
        check(&program).unwrap();
        assert_eq!(run(&program, whole(&[0; 8])), 0);
        let program = [i(0xb1, 8), i(0x06, 1)];
        assert_eq!(run(&program, whole(&[0; 8])), 0);
    }
}
