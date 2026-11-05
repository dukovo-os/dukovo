<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Architecture

A one-page map of the repository, for finding your way around before you
change something. Each part has a deeper design document in
[`docs/design/`](docs/design/), and every decision has an ADR in
[`docs/adr/`](docs/adr/).

## The big picture

Dukovo is an operating system built on **Radix**, a capability kernel
written in Rust. The kernel stays small: memory, scheduling, IPC,
capabilities, and the drivers and file systems that must run with its
privileges. Everything else runs as ordinary programs that hold only the
capabilities they were given. A program cannot open a file, a device or a
socket it has no capability for, and no account, root included, gets
around that.

At boot the kernel loads the boot archive and starts **Spark**, the first
program. Spark starts the services (accounts, logging, networking, the
consoles, the SSH server and so on) and hands each one the capabilities its
service file asks for.

## Where things live

| Path | What it holds |
| --- | --- |
| `abi/` | The system call interface, written in RIDL: every system call, object method, error and shared-memory layout. The kernel's and programs' bindings and the `man(2)` pages are generated from it. |
| `kernel/radix/` | The kernel binary: boot, the system call handler (`src/user/`), the kernel debugger, self-tests and the glue that ties the crates below together. |
| `kernel/mm`, `kernel/sched`, `kernel/ipc`, `kernel/cap` | Memory, scheduling, IPC and the capability store. Most of the logic here is plain Rust that is unit-tested on the host. |
| `kernel/arch/` | The CPU-specific parts (x86-64 for now; AArch64 and RISC-V stubs). This is where most `unsafe` code lives. |
| `kernel/hal/` | The traits the kernel uses to reach hardware, so kernel code compiles for every architecture. |
| `kernel/drivers/` | Device drivers: storage (NVMe, AHCI, USB), network cards, virtio, IOMMUs and more. |
| `kernel/fs/` | The VFS and file systems: Btrfs (our own implementation), FAT, ext4, exFAT, NTFS, ISO 9660, UDF, BitLocker. |
| `kernel/net/` | The network stack (on smoltcp), firewall and sockets. |
| `kernel/lib/` | Kernel libraries that need no hardware (ACPI tables, ELF, the POSIX layer, the TTY, `kdb`). They compile for the host and carry most of the unit tests. |
| `userspace/libradix`, `userspace/radix-rt` | The system call library and the runtime (heap, threads, `std` support) every Rust program links. |
| `userspace/spark/` | Spark, the first program and service manager. |
| `userspace/programs/` | The system programs: Nukesh (the shell), `systools` (the everyday commands), `sshd`, `smbd`, `nfsd`, account tools, the catalog and update tools. |
| `userspace/posix/` | C test programs and C services that run on the POSIX layer (mlibc). |
| `ports/` | Recipes for third-party software built for Dukovo (git, curl, OpenSSH, zstd and the rest), with their patches. |
| `tools/xtask/` | `./ob`, the build tool: building, images, QEMU, tests, benchmarks, docs and license checks. |
| `tools/ridlc/` | The RIDL compiler that turns `abi/` into code and man pages. |
| `docs/` | The handbook, man pages, design documents, ADRs, the roadmap and the coding standard. |

## How a system call travels

1. A program calls a method on a capability, for example "read from this
   file". In Rust that goes through `libradix`; in C through mlibc's
   Radix port, which makes the same system calls.
2. The `syscall` instruction enters the kernel in `kernel/arch/x86_64`,
   which saves the registers and calls `kernel/radix/src/user/syscall.rs`.
3. The handler looks the capability up in the calling process's
   capability space (`kernel/cap`). A missing capability, or one without
   the needed rights, fails here.
4. The method runs on the object: a file in `kernel/fs`, a socket in
   `kernel/net`, an endpoint in `kernel/ipc`. If the object is served by
   another program, the call becomes an IPC message to that program.
5. The result goes back in registers, and the program continues.

[`docs/design/ipc.md`](docs/design/ipc.md) and
[`docs/design/capabilities.md`](docs/design/capabilities.md) cover this in
depth.

## Where to start when something breaks

| Symptom | Start here |
| --- | --- |
| The machine does not boot, or stops early | The serial log from `./ob run`; [`docs/design/boot.md`](docs/design/boot.md); the handbook's [debugging chapter](docs/handbook/src/debugging.md) |
| A kernel panic | The panic report on the serial console names the file and line; boot with `kdb` for the kernel debugger |
| A service fails to start | `spark(8)`, the service's log (`log` command), and its service file's `[permissions]` |
| A program gets "permission denied" or "no capability" | The capabilities its service file grants; [`docs/design/capabilities.md`](docs/design/capabilities.md) |
| Disk or file system errors | [`docs/design/storage.md`](docs/design/storage.md); `fsck-btrfs(8)`; the Btrfs boot tests |
| Network problems | The handbook's [networking chapter](docs/handbook/src/networking.md); the `network-cards` boot test |
| Slower than Linux | `./ob bench` and the handbook's notes on tracing |
| A boot test fails in CI | Its log under `out/test/`; run it alone with `./ob test boot --case <name>` |

## Tests

Logic that needs no hardware is unit-tested on the host (`./ob test unit`).
Everything else is tested by booting a real image in QEMU and following
its serial console and screen (`./ob test boot`). The riskiest code also
has Kani proofs and TLA+ models. [CONTRIBUTING.md](CONTRIBUTING.md) lists
which checks to run for which change.
