// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The system calls of cancellation points (radix.hpp's `cancellable` and
// `cancellable_masked`, ADRs 0104 and 0802). radix.hpp's inline assembly
// calls them with the system call in its registers, as radix_syscall's
// `syscall` instruction takes it: the number in RAX and the arguments in
// RDI, RSI, RDX, R10, R8, R9 and R12 to R15. Each returns as the
// instruction does, with the status in RAX, the results in those ten
// registers and RCX and R11 changed, and every other register as it was,
// so that the call needs no memory for its arguments or results.
//
// A cancel that is due before the call is made is acted on instead:
// __radix_cancellable_syscall ends the thread (__radix_cancel), and
// __radix_cancellable_syscall_masked returns CANCELED for its caller to
// act on once it has put back what it holds. Between a stub's begin and
// end labels the call has not been made, or the kernel has put the thread
// back on the `syscall` instruction to make it again (a handler with
// SA_RESTART ran); SIGCANCEL's handler sends a thread it finds there to
// that stub's now label (CancelInContext, in core.cpp), with the stack as
// the stub has it. The cancel bits read are the Tcb's cancelBits:
// tcbCancelEnableBit and tcbCancelTriggerBit (mlibc's tcb.hpp, which has
// the offset checked).

	.section .text
	.global __radix_cancellable_syscall
	.hidden __radix_cancellable_syscall
	.global __radix_cancel_begin
	.hidden __radix_cancel_begin
	.global __radix_cancel_end
	.hidden __radix_cancel_end
	.global __radix_cancel_now
	.hidden __radix_cancel_now
	.type __radix_cancellable_syscall, @function
__radix_cancellable_syscall:
__radix_cancel_begin:
	mov %fs:0x30, %r11d
	and $5, %r11d
	cmp $5, %r11d
	je __radix_cancel_now
	syscall
__radix_cancel_end:
	ret
__radix_cancel_now:
	// The thread ends here, in C, which needs the stack 16-byte aligned
	// for its call; nothing returns.
	and $-16, %rsp
	call __radix_cancel
	ud2
	.size __radix_cancellable_syscall, . - __radix_cancellable_syscall

	.global __radix_cancellable_syscall_masked
	.hidden __radix_cancellable_syscall_masked
	.global __radix_cancel_masked_begin
	.hidden __radix_cancel_masked_begin
	.global __radix_cancel_masked_end
	.hidden __radix_cancel_masked_end
	.global __radix_cancel_masked_now
	.hidden __radix_cancel_masked_now
	.type __radix_cancellable_syscall_masked, @function
__radix_cancellable_syscall_masked:
__radix_cancel_masked_begin:
	mov %fs:0x30, %r11d
	and $5, %r11d
	cmp $5, %r11d
	je __radix_cancel_masked_now
	syscall
__radix_cancel_masked_end:
	ret
__radix_cancel_masked_now:
	// CANCELED (radix.hpp), with no results.
	mov $-1, %rax
	ret
	.size __radix_cancellable_syscall_masked, . - __radix_cancellable_syscall_masked

	.section .note.GNU-stack,"",%progbits
