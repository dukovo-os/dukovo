<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Linux programs: the personality and the Linux table

This note describes how Radix runs static x86-64 Linux programs, as of
Phase 18's first step: how `exec` picks the personality, the path of a
Linux call, what the kernel keeps for a Linux program, the locks, and how
a call is added. The user-facing description is radix-linux(7); the
decisions are ADRs 0610 and 0611.

## Crates and files

```text
kernel/lib/linux            radix-linux: Linux's call numbers and names,
                            error numbers (from_status), open, access and
                            other flags, clone's flags (classify), signal
                            numbers and records, struct stat, statx,
                            linux_dirent64, utsname, sockaddr and the other
                            records (host-tested, no unsafe)
kernel/lib/elf              Dukovo's note, Abi::of: which personality a
                            program gets
kernel/radix/src/user/linux the table:
  mod.rs                    call, dispatch, method, results, errors
  state.rs                  LinuxProcess (on, execed, brk), LinuxThread
                            (the thread-number words, the robust list)
  paths.rs                  descriptors to slots, paths to Place
  files.rs                  open, read, write, stat, getdents64, cwd
  descriptors.rs            dup, fcntl, pipe, ioctl, vectored I/O, locks
  names.rs                  the path calls: mkdir, unlink, rename, links,
                            modes, owners, times, access
  memory.rs                 mmap, munmap, mprotect, madvise, brk, mremap
  process.rs                ids, clone, fork, vfork, execve, exit, wait4,
                            kill, arch_prctl, the thread words, uname
  signals.rs                sigaction, masks, sigreturn, sigaltstack,
                            the signal waits
  time.rs                   clocks, sleeping, futex, getrandom
  sockets.rs                the socket calls
  poll.rs                   poll, ppoll, select, pselect6
  system.rs                 process groups, sessions, identities, limits,
                            usage, scheduling, prctl, waitid
tools/xtask/src/personality.rs  ob build's check that every program of
                            the image and of each bundle is Dukovo's
userspace/linux/core.c      the linux-programs boot test's program
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

## Locks

- `LinuxProcess::brk` (a sleeping mutex) is taken before any lock of the
  address space, and only by `brk`.
- A directory description's offset mutex is held across the listing,
  which may sleep on the filesystem, as a read holds it.
- No table function holds a spin lock across a copy to or from the
  program, an allocation, a method call or a wait.

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
