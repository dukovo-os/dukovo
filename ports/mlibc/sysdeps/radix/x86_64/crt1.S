// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A program's first instruction (radix-startup(2)): the kernel starts it
// with the stack pointer on argc, then argv, envp and the auxiliary
// vector, as System V's x86-64 ABI has them. The same file is crt1.o and,
// for position-independent programs, Scrt1.o.

// Dukovo's ELF note (ADR 0610). The kernel gives a program Dukovo's system
// calls only if it carries this note or names /system/libraries/ld.so as
// its interpreter; every other x86-64 program is a Linux program and gets
// Linux's system calls, so a static program without the note would call
// the kernel by the wrong numbers. The name is "Dukovo" with its NUL (7
// bytes, padded to 8), the type is 1 (NT_DUKOVO_ABI) and the description
// is the version of the system interface, 1, as a 32-bit number. Every
// program that links this file has the note: the linker puts a SHT_NOTE
// section in a PT_NOTE segment and keeps it when it drops unused
// sections.
	.section .note.dukovo.abi,"a",@note
	.balign 4
	.long 7
	.long 4
	.long 1
	.asciz "Dukovo"
	.balign 4
	.long 1

	.section .text
	.global _start
	.type _start, @function
_start:
	xor %ebp, %ebp
	lea main(%rip), %rdi
	mov %rsp, %rsi
	and $-16, %rsp
	call __mlibc_entry
	ud2
	.size _start, . - _start

	.section .note.GNU-stack,"",%progbits
