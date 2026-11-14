radix-crashes(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

radix-crashes - the records of programs that crashed

# DESCRIPTION

When a program crashes, the Radix kernel makes a record of how it ended,
so that *reportd*(8) can say what crashed and where (ADR 0530). A crash
is a fault that ends the program, SIGSEGV, SIGBUS, SIGILL, SIGTRAP,
SIGFPE or SIGSYS with no handler to run, or SIGABRT. The records are read from the character device
_/devices/crashes_, root's alone (mode 0400); *reportd*(8) is the one
program that reads it.

The kernel makes the record in the thread that is ending, in the
program's own address space, at the two places where a crash ends a
process: a fault with no handler to run, and the delivery of a crash
signal that no handler takes. It makes none for other signals, such as
SIGKILL, nor for a process the system cannot run without, such as Spark.
It never writes anything to disk:
the record waits in memory until it is read.

# THE DEVICE

A read takes the oldest record, whole. A buffer too small for it fails
with *EINVAL*, and the record stays. With no record waiting, a read
waits until one comes, and with a non-blocking descriptor fails with
*WouldBlock*; *poll* says the device is readable while a record waits.

At most 8 records wait, 74544 bytes each at most. A crash while 8 wait is
dropped, and the next record read says in its header how many were
dropped before it, which *reportd*(8) logs. The kernel logs one line
for each crash, whether a record was made of it or not.

# THE RECORD

A record is little-endian and holds, in order:

*the header*
	The magic *RADIXCRS*, the version of the format (1), the record's
	size, how many records were dropped before it, the signal and its
	*si_code*, the address that faulted, the process and thread numbers,
	the user and group IDs, the numbers of up to four ancestors
	(the parent first, and 0 after the last), the time of day in
	nanoseconds (0 when it is not known) and the time since boot in
	nanoseconds, the thread's name of 16 bytes, the name of the process's
	resource domain, up to 23 bytes, which for a service's processes is
	the service's own name (*radix-scheduling*(7)), the path the program was
	started by, up to 104 bytes, cut at its end and marked so when it is
	longer, and the program counter, stack pointer and frame pointer.

*the executable mappings*
	Up to 64 of them: where each starts and ends in memory, its offset
	in its file, and the file's path, up to 104 bytes, where the kernel
	knows it. A
	program's own image has the program's path. A file that was mapped
	to run code, as a library, has the path it was opened by. A mapping
	that no file backs has none.

*the stack*
	The address it starts at, which is the stack pointer, and up to
	64 KiB of the thread's stack from there to the end of its region.
	Pages that are not present end the copy.

# WHAT IT HOLDS

The stack's bytes are memory, and can hold secrets: a password that was
being typed, a key, the text of a file. Only root can read the device,
and *reportd*(8) uses the stack only to find the functions that were
running, and keeps neither it nor the registers. What a report holds of a
crash is described in *dukovo-privacy*(7).

# KERNEL FLAGS

*crashrecords*=off
	Make no records. Crashes are still logged, one line each, and
	_/devices/crashes_ stays empty (*dukovo-boot*(7)).

# FILES

_/devices/crashes_
	The device.

# SEE ALSO

*reportd*(8), *report*(1), *radix-debug*(7), *radix-files*(7),
*dukovo-boot*(7), *dukovo-privacy*(7)
