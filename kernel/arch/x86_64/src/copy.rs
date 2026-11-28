// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's `memcpy` (ADR 0775), in place of the one
//! `compiler_builtins` gives every Rust program, whose definition is weak
//! so that a program may.
//!
//! That one copies with three `rep movs` instructions: bytes up to the
//! destination's next 8-byte boundary, then quadwords, then the bytes
//! left. Each costs some 30 cycles to start on a CPU without fast short
//! REP MOVSB (FSRM), such as the lab's Skylake, however few bytes it
//! moves, and the kernel copies short runs all the time: a packet's
//! headers, a socket's few bytes, every move of a value too large for the
//! compiler to make by registers. So a copy of fewer than 512 bytes goes
//! by general-purpose registers here, as the copies to and from programs
//! go below 64 bytes (ADR 0774), and only a longer one, where the start
//! is paid back, by `rep movsq`. The kernel uses no vector registers
//! (`kernel/targets`), which hold the program's.
//!
//! Every size is copied by whole loads and stores that may overlap,
//! which the source and the destination of a `memcpy` never do:
//!
//! - under 16 bytes, the first and the last 8, 4 or 2 bytes, or one byte;
//! - 16 to 32 bytes, the first 16 and the last 16;
//! - 33 to 511 bytes, 32 at a time from the start, then the last 32;
//! - 512 bytes or more, the first 8, then quadwords from the
//!   destination's next 8-byte boundary, then the last 8.
//!
//! Memory mapped write-combining, such as a framebuffer, is served badly
//! by both halves of that (ADR 0718). The CPU gathers the stores to such
//! memory a cache line at a time and sends each line on when it is full
//! and the stores have moved past it. A store that goes back to a line
//! already sent opens the line again for a few bytes, which then go out
//! on their own, and a `rep movs` to such memory costs what a store
//! fence after it would: a wait until everything gathered so far has
//! been sent. A picture goes to the screen as a couple of thousand
//! copies of a few kilobytes each, so those two costs came to more than
//! twice the copying itself. [`in_order`] is the copy for such a
//! destination: every byte stored once, lowest address first, by
//! general-purpose registers whatever the length.

/// The fewest bytes copied by `rep movsq`.
const LONG: usize = 512;

core::arch::global_asm!(
    crate::thunks::asm_begin!(),
    r#"
    .pushsection .text
    .balign 64
    .global memcpy
    .type memcpy, @function
memcpy:
    mov rax, rdi
    cmp rdx, 16
    jb 10f
    cmp rdx, 32
    ja 20f
    /* 16 to 32 bytes. */
    mov rcx, [rsi]
    mov r8, [rsi + 8]
    mov r9, [rsi + rdx - 16]
    mov r10, [rsi + rdx - 8]
    mov [rdi], rcx
    mov [rdi + 8], r8
    mov [rdi + rdx - 16], r9
    mov [rdi + rdx - 8], r10
    radix_ret

10: /* Under 16 bytes. */
    cmp edx, 8
    jb 11f
    mov rcx, [rsi]
    mov r8, [rsi + rdx - 8]
    mov [rdi], rcx
    mov [rdi + rdx - 8], r8
    radix_ret
11: cmp edx, 4
    jb 12f
    mov ecx, [rsi]
    mov r8d, [rsi + rdx - 4]
    mov [rdi], ecx
    mov [rdi + rdx - 4], r8d
    radix_ret
12: test edx, edx
    jz 14f
    movzx ecx, byte ptr [rsi]
    cmp edx, 2
    jb 13f
    movzx r8d, word ptr [rsi + rdx - 2]
    mov [rdi + rdx - 2], r8w
13: mov [rdi], cl
14: radix_ret

20: /* 33 bytes or more. */
    cmp rdx, {long}
    jae 30f
    /* The last 32 bytes' places, which the loop stops short of. */
    lea r11, [rsi + rdx - 32]
    lea rdx, [rdi + rdx - 32]
21: mov rcx, [rsi]
    mov r8, [rsi + 8]
    mov r9, [rsi + 16]
    mov r10, [rsi + 24]
    mov [rdi], rcx
    mov [rdi + 8], r8
    mov [rdi + 16], r9
    mov [rdi + 24], r10
    add rsi, 32
    add rdi, 32
    cmp rsi, r11
    jb 21b
    mov rcx, [r11]
    mov r8, [r11 + 8]
    mov r9, [r11 + 16]
    mov r10, [r11 + 24]
    mov [rdx], rcx
    mov [rdx + 8], r8
    mov [rdx + 16], r9
    mov [rdx + 24], r10
    radix_ret

30: /* {long} bytes or more. */
    mov r8, [rsi]
    mov r9, [rsi + rdx - 8]
    lea r10, [rdi + rdx - 8]
    mov [rdi], r8
    /* Up to the destination's next 8-byte boundary, which the first 8
       bytes covered. */
    mov ecx, edi
    neg ecx
    and ecx, 7
    add rsi, rcx
    add rdi, rcx
    sub rdx, rcx
    mov rcx, rdx
    shr rcx, 3
    rep movsq
    mov [r10], r9
    radix_ret
    .size memcpy, . - memcpy

    /* radix_copy_in_order(rdi = to, rsi = from, rdx = length). */
    .balign 64
    .global radix_copy_in_order
    .type radix_copy_in_order, @function
radix_copy_in_order:
    /* Where the source's whole 32 bytes end. */
    mov r11, rdx
    and r11, -32
    jz 41f
    add r11, rsi
40: mov rcx, [rsi]
    mov r8, [rsi + 8]
    mov r9, [rsi + 16]
    mov r10, [rsi + 24]
    mov [rdi], rcx
    mov [rdi + 8], r8
    mov [rdi + 16], r9
    mov [rdi + 24], r10
    add rsi, 32
    add rdi, 32
    cmp rsi, r11
    jb 40b
41: /* Under 32 bytes are left: 16, 8, 4, 2 and 1 of them, as the bits
       of their number say. */
    test dl, 16
    jz 42f
    mov rcx, [rsi]
    mov r8, [rsi + 8]
    mov [rdi], rcx
    mov [rdi + 8], r8
    add rsi, 16
    add rdi, 16
42: test dl, 8
    jz 43f
    mov rcx, [rsi]
    mov [rdi], rcx
    add rsi, 8
    add rdi, 8
43: test dl, 4
    jz 44f
    mov ecx, [rsi]
    mov [rdi], ecx
    add rsi, 4
    add rdi, 4
44: test dl, 2
    jz 45f
    movzx ecx, word ptr [rsi]
    mov [rdi], cx
    add rsi, 2
    add rdi, 2
45: test dl, 1
    jz 46f
    movzx ecx, byte ptr [rsi]
    mov [rdi], cl
46: ret
    .size radix_copy_in_order, . - radix_copy_in_order
    .popsection
"#,
    crate::thunks::asm_end!(),
    long = const LONG,
);

unsafe extern "C" {
    /// Copies `len` bytes from `src` to `dst`, which do not overlap, with
    /// every byte stored once and the lowest address first; see the
    /// assembly above.
    fn radix_copy_in_order(dst: *mut u8, src: *const u8, len: usize);
}

/// Copies `len` bytes from `src` to `dst` with every byte stored once
/// and the lowest address first: the copy for a destination mapped
/// write-combining, which `memcpy`'s overlapping stores and its `rep
/// movsq` both slow down (ADR 0718).
///
/// # Safety
///
/// `src` is readable and `dst` writable for `len` bytes, and the two do
/// not overlap.
pub unsafe fn in_order(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's; the routine reads and writes those bytes
    // alone and uses no stack.
    unsafe { radix_copy_in_order(dst, src, len) };
}
