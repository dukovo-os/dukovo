// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMware's backdoor (ADR 0424): an `in` from its port, or a string to or
//! from its high-bandwidth port, with seven registers set before and read
//! after, which VMware's hypervisors answer themselves.
//!
//! Two of the registers, RBX and RBP, are ones Rust's inline assembly
//! cannot name, so each call saves them on the stack, loads all seven from
//! a frame in memory whose address it keeps on the stack too, does the
//! instruction, and writes all seven back to the frame, as OpenBSD's
//! vmt(4) does.

use core::arch::asm;

use radix_hal::vmware::Regs;

use crate::cpu;

/// CPUID 0x4000_0000: the hypervisor's name, in EBX, ECX and EDX.
const SIGNATURE: [u8; 12] = *b"VMwareVMware";

/// Whether the hypervisor names itself VMware's.
pub fn signature() -> bool {
    if !cpu::feature::hypervisor() {
        return false;
    }
    let leaf = cpu::cpuid(0x4000_0000, 0);
    let mut name = [0u8; 12];
    name[..4].copy_from_slice(&leaf.ebx.to_le_bytes());
    name[4..8].copy_from_slice(&leaf.ecx.to_le_bytes());
    name[8..].copy_from_slice(&leaf.edx.to_le_bytes());
    name == SIGNATURE
}

/// The registers of a call, as the assembly reads and writes them: RAX,
/// RBX, RCX, RDX, RSI, RDI and RBP, eight bytes apiece.
#[repr(C)]
struct Frame([u64; 7]);

impl Frame {
    fn new(regs: &Regs) -> Self {
        Frame([regs.ax, regs.bx, regs.cx, regs.dx, regs.si, regs.di, regs.bp].map(u64::from))
    }

    fn read(&self, regs: &mut Regs) {
        let [ax, bx, cx, dx, si, di, bp] = self.0.map(|word| word as u32);
        *regs = Regs { ax, bx, cx, dx, si, di, bp };
    }
}

/// Does `$op` with the registers of the frame at `$frame` (a `*mut
/// Frame`), and writes them back.
macro_rules! backdoor {
    ($op:literal, $frame:expr) => {
        asm!(
            "push rbx",
            "push rbp",
            // The frame's address, kept for after.
            "push rax",
            "mov rbp, [rax + 48]",
            "mov rdi, [rax + 40]",
            "mov rsi, [rax + 32]",
            "mov rdx, [rax + 24]",
            "mov rcx, [rax + 16]",
            "mov rbx, [rax + 8]",
            "mov rax, [rax]",
            $op,
            // The frame's address back, and RAX's result in its place.
            "xchg rax, [rsp]",
            "mov [rax + 48], rbp",
            "mov [rax + 40], rdi",
            "mov [rax + 32], rsi",
            "mov [rax + 24], rdx",
            "mov [rax + 16], rcx",
            "mov [rax + 8], rbx",
            "pop qword ptr [rax]",
            "pop rbp",
            "pop rbx",
            inout("rax") $frame => _,
            out("rcx") _,
            out("rdx") _,
            out("rsi") _,
            out("rdi") _,
        )
    };
}

/// `in eax, dx` with `regs`.
pub fn call(regs: &mut Regs) {
    let mut frame = Frame::new(regs);
    let pointer: *mut Frame = &raw mut frame;
    // SAFETY: only called where the hypervisor is VMware's (or one that
    // answers its backdoor), whose port reads change no memory of the
    // guest's. The assembly reads and writes `frame` alone, and restores
    // RBX, RBP and the stack.
    unsafe { backdoor!("in eax, dx", pointer) };
    frame.read(regs);
}

/// `rep outsb` of `bytes` with `regs`.
pub fn send(regs: &mut Regs, bytes: &[u8]) {
    let mut frame = Frame::new(regs);
    frame.0[2] = bytes.len() as u64;
    frame.0[4] = bytes.as_ptr() as u64;
    let pointer: *mut Frame = &raw mut frame;
    // SAFETY: as for `call`; the string instruction reads RCX bytes from
    // RSI, which are `bytes`, and the direction flag is clear, as Rust's
    // inline assembly guarantees.
    unsafe { backdoor!("rep outsb", pointer) };
    frame.read(regs);
}

/// `rep insb` of `bytes.len()` bytes into `bytes` with `regs`.
pub fn receive(regs: &mut Regs, bytes: &mut [u8]) {
    let mut frame = Frame::new(regs);
    frame.0[2] = bytes.len() as u64;
    frame.0[5] = bytes.as_mut_ptr() as u64;
    let pointer: *mut Frame = &raw mut frame;
    // SAFETY: as for `send`; the string instruction writes RCX bytes at
    // RDI, which are `bytes`.
    unsafe { backdoor!("rep insb", pointer) };
    frame.read(regs);
}
