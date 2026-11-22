radix-linux(7) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

radix-linux - Linux programs on the Radix kernel: which programs run as
Linux programs, the calls they are given, and how each is served

# DESCRIPTION

Radix runs static x86-64 programs built for Linux unchanged: a program
linked with glibc or musl, a Rust program for a Linux target, a Go
program for _linux/amd64_. Such a program makes its system calls with
Linux's numbers, and the kernel serves them with a table of Linux's
calls (ADR 0610).

Each Linux call ends in a method the program could call itself, on a
capability in its own slots: a file's *read*, a directory's *open*, its
process's *fork*, its address space's *map_anonymous*. The table finds
the slot a call means, turns Linux's arguments into the method's, and
turns the method's answer into Linux's. A Linux program therefore has the
same authority, and meets the same checks, as a Dukovo program started
in its place with the same slots.

# WHICH PROGRAMS RUN AS LINUX PROGRAMS

*exec* decides from the main program's ELF file. The program gets
Radix's own system calls if a *PT_NOTE* segment holds Dukovo's note (the
name "Dukovo", type 1, the interface version 1 as its description) or if
its interpreter is _/system/libraries/ld.so_. Every other x86-64 program
runs as a Linux program. Dukovo's toolchain writes the note into every
program it links (*radix-posix*(7), BUILDING PROGRAMS), and *ob build*
fails if a program of the system image or of a bundle would run as Linux.

The choice is the process's, made at each *exec* and kept by *fork*. A
Linux program may run a Dukovo program and the other way round; each
gets its own calls.

# STARTING

A Linux program starts as Linux starts one: its arguments, environment
and auxiliary vector on the stack, with no start-up page. The auxiliary
vector gives the process's user and group numbers (*AT_UID*, *AT_EUID*,
*AT_GID*, *AT_EGID*), the processor's features as Linux gives them
(*AT_HWCAP*, CPUID leaf 1's EDX; *AT_HWCAP2* 0), the largest signal frame
(*AT_MINSIGSTKSZ*), *AT_SECURE* 0, 16 random bytes, the page size and the
path the program was run by.

*uname* says *Linux*, release *6.1.0-dukovo*, version *#1 SMP Dukovo*
with Dukovo's version, machine *x86_64*, and the machine's name as
_/system/settings/hostname_ holds it.

# CALLS

The number is in rax and the arguments in rdi, rsi, rdx, r10, r8 and r9;
the result, or a negated error number, comes back in rax, and every other
register as it went in but rcx and r11. A method's error becomes the
error number Dukovo's C library gives for it, unless Linux answers the
call otherwise. A call a signal interrupts fails with EINTR or runs again
as Linux's would, *SA_RESTART* included.

The table has Linux's calls for files and descriptors, paths and
directories, processes and threads, memory, signals, futexes, clocks and
sleeping, sockets, *poll* and *select*, identities, limits and
scheduling, and *getrandom*. A number it does not know is ENOSYS, as on
a Linux too old for the call.

Some calls are refused for good, as Linux refuses a program without the
privilege: *io_uring_setup*, *bpf*, *perf_event_open*, *ptrace*,
*kexec_load* and the module calls are ENOSYS; *mount*, *umount2*,
*pivot_root*, *reboot*, *swapon*, *swapoff*, *sethostname*,
*setdomainname*, *settimeofday* and *clock_settime* are EPERM, as is a
*clone* that asks for a new namespace.

# PATHS AND THE WORKING DIRECTORY

The kernel keeps a Linux program's working directory, as an absolute path
with no links (ADR 0611). An absolute path is resolved from the process's
root directory capability, a relative one from the working directory, and
one given with a directory descriptor from that directory. *exec* keeps
the working directory when a Linux program runs a Dukovo program, and the
other way round.

# DESCRIPTORS

Descriptor _n_ is the process's slot 64 + _n_, as for a Dukovo program,
and a new descriptor is the lowest free one below *RLIMIT_NOFILE*.
*O_CLOEXEC* and *FD_CLOEXEC* are the slot's close-on-exec flag. A
directory's listing cursor is its open description's offset: *getdents64*
gives "." and ".." first, and *lseek* moves the cursor.

# MEMORY

*mmap*, *munmap*, *mprotect* and *brk* map, unmap and protect through the
address space's methods; a page may not be both writable and executable,
and asking for both is EACCES. *madvise* with *MADV_DONTNEED* or
*MADV_FREE* lets go of private anonymous pages, which read as zeros when
touched again (*radix-address-space*(2)'s *forget*). *mremap* shrinks a
mapping; growing or moving one is ENOMEM, and the C libraries' *realloc*
copies instead.

# PROCESSES AND THREADS

A thread's number is the kernel's, and a process's first thread has the
process's number. *clone* makes a thread when it shares memory, files,
the file system's state and signal handlers, and a process when it shares
none of them; *CLONE_SETTLS* and the three thread-number words work as on
Linux. *vfork* gives the child a copy of the memory, and the parent waits
until the child has run *exec* or begun to end.

# SIGNALS

Signals, masks and actions are Linux's numbers and records, and a
handler runs on Linux's *rt_sigframe*, which *rt_sigreturn* restores.

# IDENTITIES

A process acts for the user and the group it was started with, and for
the other groups the account has, and keeps them: Radix has no
set-user-ID. *setuid*, *setresuid*, *setgid*, *setgroups* and the others
succeed when they would change nothing and fail with EPERM when they
would.

# SOCKETS, POLL AND TERMINALS

*AF_UNIX* and *AF_INET*/*AF_INET6* sockets end in the socket's own
methods. The kernel reads a call's records, such as an address, the
descriptors of *SCM_RIGHTS* and a *poll* array, into its own memory and
gives the program's buffers to the socket as they are: the buffers of
one *sendmsg* or *recvmsg* are one send or one receive, so a datagram is
never cut in pieces, and no call maps memory into the program. A Unix
socket's *SCM_CREDENTIALS* is the sender's process number and identity,
and *SO_PEERCRED* is the peer's.

*poll* and *select* wait in the native *poll*; *ppoll* and *pselect6*
take a signal mask for the wait.

A terminal's *ioctl* requests have Linux's numbers and go to the
terminal's native *control*. *TCGETS* and the *TCSETS* calls move the
first 36 bytes of the native settings, which are Linux's *struct
termios*; the speeds follow *c_cflag*'s baud bits.

# NOT YET

Later steps of Phase 18 bring dynamically linked Linux programs and the
Linux view of the file tree (_/proc_, _/sys_, _/dev_), *epoll*, *eventfd*,
*timerfd*, *signalfd*, *memfd*, *pidfd* and the vDSO, seccomp and
namespaces. Until then those calls are ENOSYS or EPERM, as above. The
other gaps are rows of _docs/deferred.md_.

# SEE ALSO

*radix-posix*(7), *radix-processes*(7), *radix-files*(7),
*radix-address-space*(2), *radix-process*(2), *ob*(1)
