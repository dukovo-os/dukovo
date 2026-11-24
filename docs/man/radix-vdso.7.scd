radix-vdso(7) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

radix-vdso - the clock page and the vDSO, which programs read the time
with without a system call

# DESCRIPTION

*exec* maps two things into every program it starts: the *clock page*,
read-only, and right above it the *vDSO*, a small shared object that
reads the page (ADR 0616). The auxiliary vector gives the vDSO's address
as *AT_SYSINFO_EHDR* (33). Dukovo's C library finds it there, as glibc,
musl and Go's runtime find Linux's, and reads *CLOCK_REALTIME*,
*CLOCK_MONOTONIC* and *CLOCK_BOOTTIME* without entering the kernel.

Nothing is mapped, and no *AT_SYSINFO_EHDR* given, where programs may not
read the clock's counter: on a machine whose clock the kernel has not
calibrated, and on an architecture without a vDSO. A program the kernel
starts itself has none either. The C libraries then make the system
calls, as they would on an old Linux.

# THE VDSO

The vDSO is an ELF shared object named _linux-vdso.so.1_, one segment
that is read and executed, with no relocations and no writable data. It
exports these functions, each under the version *LINUX_2.6*, and each
also under its name without the prefix, as a weak symbol, as Linux's
does:

*\_\_vdso_clock_gettime*(_clock_, _ts_)
	Linux's *clock_gettime*. The real-time, monotonic and boot-time
	clocks, *CLOCK_REALTIME_COARSE*, *CLOCK_MONOTONIC_COARSE* and
	*CLOCK_MONOTONIC_RAW* are read from the page; any other clock, and a
	real-time clock while the wall clock is not known, is the system
	call.

*\_\_vdso_gettimeofday*(_tv_, _tz_)
	The real-time clock in microseconds; _tz_, if given, is zeros.

*\_\_vdso_time*(_t_)
	The real-time clock in seconds.

*\_\_vdso_clock_getres*(_clock_, _res_)
	1 ns for the clocks the page answers.

*\_\_vdso_getcpu*(_cpu_, _node_)
	The CPU the thread runs on, from *RDPID* or *RDTSCP*, which read the
	CPU's index that the kernel keeps in *IA32_TSC_AUX*; the node is
	always 0. Without either instruction, the system call.

Under the version *DUKOVO_1* it exports *\_\_dukovo_clock_read*(_clock_,
_ns_), which Dukovo's C library calls with the kernel's own clock numbers
(*radix-clock*(2)): 0 and the time in nanoseconds, or -1 when the page
cannot answer.

# THE CLOCK PAGE

The page lies 4096 bytes below the vDSO. It is written by the kernel
whenever the wall clock is set or slewed, under a sequence number that is
odd while it is written: a reader reads the number, the fields, and the
number again, and starts over if the two differ or are odd. Its fields,
each a little-endian 64-bit word, are:

[[ *Offset*
:[ *Field*
|  0
:  The sequence number.
|  8
:  The time stamp counter's value at the kernel clock's zero.
|  16
:  Nanoseconds per tick, times 2^32.
|  24
:  Flags: 1, the counter may be read; 2, *RDPID* gives the CPU; 4,
   *RDTSCP* does.
|  32
:  1 if the wall clock is known.
|  40
:  The wall clock's offset from the kernel's clock, in nanoseconds.
|  48
:  When the current slew began, on the kernel's clock.
|  56
:  The slew in nanoseconds, signed, applied at most 1 ns in 2000.

The monotonic clock is ((*rdtsc* - start) × scale) >> 32. The real-time
clock is that plus the offset and as much of the slew as the time since
it began allows. The layout is the kernel's, not a stable interface:
programs read the clocks through the vDSO, which comes with the kernel
that wrote the page.

# SECURITY

The page holds only the time, which every program may read anyway, and
is mapped read-only; *mprotect* cannot make it, or the vDSO, writable. A
program may unmap them, which costs it only its fast clocks. *fork*
shares both, as every process does.

# SEE ALSO

*radix-posix*(7), *radix-clock*(2), *radix-address-space*(2)
