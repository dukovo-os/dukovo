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

/// The fewest bytes copied by `rep movsq`.
const LONG: usize = 512;

core::arch::global_asm!(
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
    ret

10: /* Under 16 bytes. */
    cmp edx, 8
    jb 11f
    mov rcx, [rsi]
    mov r8, [rsi + rdx - 8]
    mov [rdi], rcx
    mov [rdi + rdx - 8], r8
    ret
11: cmp edx, 4
    jb 12f
    mov ecx, [rsi]
    mov r8d, [rsi + rdx - 4]
    mov [rdi], ecx
    mov [rdi + rdx - 4], r8d
    ret
12: test edx, edx
    jz 14f
    movzx ecx, byte ptr [rsi]
    cmp edx, 2
    jb 13f
    movzx r8d, word ptr [rsi + rdx - 2]
    mov [rdi + rdx - 2], r8w
13: mov [rdi], cl
14: ret

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
    ret

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
    ret
    .size memcpy, . - memcpy
    .popsection
"#,
    long = const LONG,
);
