<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Linux programs: the personality and the Linux table

This note describes how Radix runs x86-64 Linux programs, as of Phase
18's third step: how `exec` picks the personality, the path of a Linux
call, what the kernel keeps for a Linux program, the files of the
kernel's own and the vDSO, the calls the kernel forwards to linuxd and
linuxd itself, the locks, and how a call is added. The user-facing
description is radix-linux(7), radix-vdso(7), linux(1) and linuxd(8); the
decisions are ADRs 0610 to 0622.

## Crates and files

```text
kernel/lib/linux            radix-linux: Linux's call numbers and names,
                            error numbers (from_status), open, access and
                            other flags, clone's flags (classify), signal
                            numbers and records, struct stat, statx,
                            linux_dirent64, utsname, sockaddr and the other
                            records; epoll, the futex operations and the
                            robust list's rule, eventfd, timerfd, signalfd
                            and pidfd (kernel_files), memfd and inotify
                            (notify) (host-tested, no unsafe)
kernel/lib/vdso             radix-vdso: the vDSO, a C shared object built
                            with clang and LLD and checked as it is built
                            (ADR 0616)
kernel/lib/elf              Dukovo's note, Abi::of: which personality a
                            program gets
kernel/radix/src/user/linux the table:
  mod.rs                    call, dispatch, method, results, errors
  state.rs                  LinuxProcess (on, execed, brk), LinuxThread
                            (the thread-number words, the robust list)
  robust.rs                 the robust list's walk at a thread's end
  paths.rs                  descriptors to slots, paths to Place
  files.rs                  open, read, write, stat, getdents64, cwd
  descriptors.rs            dup, fcntl, pipe, ioctl, vectored I/O, locks
  names.rs                  the path calls: mkdir, unlink, rename, links,
                            modes, owners, times, access
  memory.rs                 mmap, munmap, mprotect, madvise, brk, mremap,
                            memfd_create
  process.rs                ids, clone, fork, vfork, execve, exit, wait4,
                            kill, arch_prctl, the thread words, uname
  signals.rs                sigaction, masks, sigreturn, sigaltstack,
                            the signal waits
  time.rs                   clocks, sleeping, futex, getrandom
  sockets.rs                the socket calls
  poll.rs                   poll, ppoll, select, pselect6
  events.rs                 epoll on the event queue's poll filter
  kernel_files.rs           eventfd, timerfd, signalfd, pidfd
  notify.rs                 inotify
  system.rs                 process groups, sessions, identities, limits,
                            usage, scheduling, prctl, waitid
  forward.rs                the calls forwarded to linuxd, and the Linux
                            call object's methods (ADR 0618)
  magic.rs                  a process's own /proc links (ADR 0620)
  seccomp.rs                a thread's seccomp: its mode, its chain of
                            filters and the calls they always allow, the
                            check before each call, seccomp(2) (ADR 0623)
kernel/lib/bpf              radix-bpf: classic BPF's checks and
                            interpreter, seccomp's rules, actions and the
                            map of calls a filter always allows, with Kani
                            proofs and a fuzz target (host-tested, no
                            unsafe)
kernel/radix/src/user/jit.rs  the jit capability's methods: memory.jit
                            given to a process (ADR 0624)
tools/xtask/src/personality.rs  ob build's check that every program of
                            the image and of each bundle is Dukovo's
userspace/linux/core.c      the linux-programs boot test's program
userspace/linux/view.c      the linux-view boot test's program, dynamic
userspace/linux/suite-run.c the runner of LTP's cases in `ob test
                            linux-suite`, on Linux and Dukovo (ADR 0628)
userspace/posix/linux       seccomp.c, jit.c and seccomp-diff.c, the
                            seccomp, memory.jit and differential programs
userspace/programs/linuxd   linuxd(8): the forwarded calls (serve.rs,
                            call.rs), /proc and /sys over FUSE (fs/), the
                            netlink route sockets (netlink/)
userspace/programs/systools/src/linux
                            linux(1): `linux run`, the sandbox, the
                            supervisor
ports/linux-base            the Linux base bundle, org.dukovo.linux-base
```

## The personality

`exec` reads the main program's headers with `radix-elf`, which records
each `PT_NOTE` segment's file range; `exec` reads each note (up to
`NOTE_MAX` bytes) as it reads `PT_INTERP`, and `Abi::of(interpreter,
marked)` decides. A Linux program's `LinuxProcess` is turned on by
`LinuxProcess::start`, with its break, before the new thread starts;
`fork` copies it (`copy_from`). The stack is built without the start-up
page and with Linux's auxiliary vector (`build_stack`).

## A call's path

```text
syscall entry -> syscall::handle
  user.process.linux.on()?  -- no --> Radix's twelve calls
        | yes
        v
linux::call(me, user, regs)
  dispatch: regs.number -> a function of the table
     the function: Linux's arguments -> a method's words
        method(me, user, slot, label, words)
          objects::find_holding + methods::call   (as cap_call does)
     the answer -> Linux's result or records (copy_out)
  Ok(Return(v))       rax = v
  Err(Errno(e))       rax = -e
  Err(Native(s))      rax = -from_status(s), or, for interrupted,
                      signal::call_failed (restart or EINTR)
  Ok(Restore(regs))   rt_sigreturn's saved registers
  Ok(Exit / ExitThread) the thread ends (ExitThread clears the
                      CLEARTID word after it has left its process)
```

The table writes only rax (and, for `rt_sigreturn`, the whole frame);
the arguments stay in the saved registers, so a call that runs again
after a signal finds them.

When Linux's record is not the method's, the table does the method's
work with the record in kernel memory, through the function that is the
method's body and that the method calls around its caller's records:

```text
paths, directory methods     Paths::Given (user/files.rs, directory_call)
a file's status              status_given
socket names and addresses   socket_name_of, bind_address_of,
                             connect_address_of, send_to_of,
                             receive_from_of
sends and receives           send_of, receive_of, send_files_of,
                             receive_unix_of, send_datagram_given,
                             with the program's iovecs as one Buffers
poll and select              posix::poll::poll_entries
a CPU set, the groups        priority::affinity_of, process::own_process
```

`linux::files::on_object` finds the capability as a method call does;
each body checks its kind and rights. The table maps nothing into the
program for a record of its own.

## What the kernel keeps

- The working directory: `PosixState::directory`, an absolute path.
  `paths::place` joins a relative path to it in a kernel buffer, or takes
  a directory descriptor's slot; `files::at` retries from the root when a
  walk leaves a descriptor's directory. The directory methods take the
  path in kernel memory (`Paths::Given`).
- Descriptors: slots from `SLOT_FD_BASE`. `files::free_descriptor` finds
  the lowest free one below `RLIMIT_NOFILE`, and `files::into_free`
  tries again when another thread took it (`occupied`).
- A directory's listing cursor: the description's offset
  (`FileObject::offset`), held for the length of a `getdents64`.
- The break: `LinuxProcess::brk`.
- A thread's words: `LinuxThread`. The `CLONE_CHILD_SETTID` word is
  written by the new thread itself in `thread_main` (`linux::starting`).

## Files of the kernel's own and the vDSO

eventfd, timerfd, signalfd, pidfd, inotify's watch files and memory files
are files: each a node of a filesystem of the kernel's (the pipes' one, or
a memory filesystem no directory holds), made by a method of the process
in a slot, and recognised by `FileObject::opened_as`, so `read`, `write`,
`poll`, the event queue and `close` reach them through the paths every
file takes (`user/files.rs`'s `Target`, `posix::poll::polled_by` and
`file_events`). Their own methods are radix-file(2)'s `timer_set`,
`timer_get`, `set_signal_mask`, `process_of`, `seal`, `watch_add` and
`watch_remove`. Dukovo's C library makes the same calls, so a fix to one
reaches both kinds of program.

epoll is the event queue's `poll` filter (ADR 0612): the table makes one
change of the queue per `epoll_ctl` and waits with `queue::wait_given`,
writing `struct epoll_event` records into the program's array.

`exec` maps the clock page and the vDSO (`user/vdso.rs`) into every
program and gives `AT_SYSINFO_EHDR`: glibc, musl and Dukovo's C library
read the clocks without a call (ADR 0616).

## seccomp and memory.jit

`linux::call` asks `seccomp::check` before `dispatch`, which answers at
once when the thread's mode is off. A call the chain's map always allows (every
filter's, computed when the filter is installed, as Linux 5.11's cache
does) goes on without running a filter; any other runs the chain, newest
first, and takes the strictest action. `inherit` copies the mode, the
flag and the chain to a new thread before it runs; `TSYNC` copies the
caller's to its process's other threads under the process's lock.

A program given memory.jit (the `jit` capability's `allow`, which `linux
run --allow-jit`, a bundle's launcher or authd's sessions hold) may map or
protect its private anonymous memory writable and executable at once;
`methods::may_jit` and the table's `mmap` and `mprotect` ask the
process's `jit()`, which `allow_jit` sets for good, and a refusal is
noted for `linux run`'s hint (`refuse_jit`). A file's pages and shared
memory never are.

## Forwarded calls and linuxd

A program `linux run` starts has an endpoint in slot `SLOT_LINUX`; one
started otherwise has none, and nothing below happens for it. A call the
table does not know, a `socket` of a family the kernel lacks, and a socket
call on a descriptor that is not a kernel socket go to `forward::forward`,
which sends the call's number and arguments as a call on the endpoint
(label `FORWARDED`, no capability, no grant) and waits for the reply like
any IPC call, so a signal cuts it short only while it is still queued. The
thread keeps a `Forwarding` in `LinuxThread::forwarded` while it waits.

The linuxd thread that received it calls `forwarded_call`, which finds the
caller through the reply it holds (`ipc::held_caller`) and makes, once, a
`LinuxCall` object for that `Forwarding`: a capability with read, write
and control, never copy or transfer. Its methods (`forward::method`) read
and write the caller's memory through `SpaceObject::read_other` and
`write_other`, which walk the caller's page tables under its space's lock
and copy through the direct map a page at a time, bounced through 1 KiB on
the kernel stack, and give and take descriptors. When the caller's wait
ends, `forward` takes the `Forwarding` back and marks the object ended, and
every method then fails with `gone`.

linuxd (`userspace/programs/linuxd`) answers from three worker threads and
serves its FUSE filesystem from a fourth; `linux run` starts it, holds its
endpoint and FUSE connection, and starts it again with `--again` if it
ends. `serve_fuse` (`user/files.rs`) makes linuxd's filesystem a mount in
no tree, whose root the launcher's view attaches at `/proc` and `/sys`.
`memory_filesystem` makes a memory filesystem the same way
(`fs::shm::apart`), which the view attaches at `/dev/shm`: the MAP_FILE
path shares the pages of a memory filesystem's files alone, so a folder of
the person's would refuse `shm_open`'s and LTP's shared writable
mappings with ENODEV. Its pages come from the memory files' room.

## Locks

- `LinuxProcess::brk` (a sleeping mutex) is taken before any lock of the
  address space, and only by `brk`.
- A directory description's offset mutex is held across the listing,
  which may sleep on the filesystem, as a read holds it.
- No table function holds a spin lock across a copy to or from the
  program, an allocation, a method call or a wait.
- The futex calls on two words lock both waiter lists, the one first in
  the table first (ADR 0617).
- `LinuxThread::forwarded` is a spin lock, held only to set, take or read
  the `Forwarding`; the call object is made before it is taken and put in
  under it. A cross-space copy holds the caller's space lock only while it
  walks a page's tables and copies that page through the direct map,
  never across a fault, which it makes with the lock dropped.
- The timer files' list (`fs::timer_file::ARMED`, a sleeping mutex) is
  taken before a timer file's state, a spin lock; the watches' table
  (`fs::notify`) is a spin lock under which nothing is allocated, freed,
  queued or woken (ADR 0615).

## Adding a call

1. Its number, flags and records go in `radix-linux`, with host tests of
   each conversion.
2. The function goes in the file of its group. It finds the slot, builds
   the method's arguments, calls `method` or `results`, and turns the
   answer into Linux's. It follows what Dukovo's C library does for the
   same POSIX function (ports/mlibc/sysdeps/radix/generic/), and Linux's
   raw call where the two differ.
3. The arm goes in `dispatch`, in its group.
4. `userspace/linux/core.c` gets a case if the call has a rule a C
   library hides, and the case runs on Linux first.
5. `ob test linux-suite` runs LTP's cases of the call on Linux and on
   Dukovo: those that now pass on Dukovo lose their skips, and those
   that do not need one, with its row of `docs/deferred.md` (ADR 0628).
