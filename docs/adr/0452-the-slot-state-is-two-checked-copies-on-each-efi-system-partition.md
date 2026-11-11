<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0452. The slot state is two checked copies on each EFI system partition

- Status: Accepted
- Date: 2026-11-11

## Context

GRUB decides which slot starts (ADR 0451), and it must count each try of
a new slot before starting it, so that a hang, a panic and a power cut
each cost one. ChromeOS's boot loader takes one of a new partition's six
attempts before each try; a count kept after the start, as a boot that
counts once a process runs, misses the starts that never get that far.
So GRUB has to write, and GRUB writes only to plain file systems such as
FAT, in place, a block at a time, through its environment blocks of
1024 bytes. FAT has no journal, and a power cut can fall in the middle
of any write.

The roadmap also carries an item deferred from Phase 14: the menu should
show after a start that did not finish, so that a machine that stopped
on its way up can be started otherwise. And an installed system with the
mirror layout has an EFI system partition on each disk (ADR 0315),
which must start the same slot when one disk is gone.

## Decision

- **The state** is six fields: a generation `slot_gen` (0, 1 or 2), the
  default slot `slot_default`, the previous version `slot_previous`, the
  update being tried `slot_try`, its tries left `slot_tries` (0 to 3),
  and `slot_boot`, the slot whose start is not yet seen through. A
  seventh, `slot_check`, holds the six joined by colons.
- **Two copies on each EFI system partition**, `boot/grub/state1` and
  `boot/grub/state2`, each a whole GRUB environment block. A copy is
  whole when its check matches its fields and each field holds a value
  it can hold. Of two whole copies the newer counts: 1 is newer than 0,
  2 than 1, and 0 than 2, which GRUB's scripts can tell without
  arithmetic.
- **Every writer writes the state that follows over the older copy, or
  over the one that is not whole, with the next generation.** A write
  cut short spoils only the copy it was writing, so the state read
  afterwards is the one from before that write, never a mix. The system
  reads both copies again before it writes.
- **GRUB counts the try, or does not start it.** The Dukovo entry takes
  one try from the update being tried and marks the start as not seen
  through, in one write, before it loads the kernel. If GRUB cannot
  write, the default starts and the try waits. A try that was not counted
  is never started.
- **What starts.** The update being tried, while it has tries left;
  otherwise the default, with `fallback=` on the kernel's command line
  naming the update that ran out, which puts the settings back (ADR
  0454). The kernel's command line always says the slot, `slot=a` or
  `slot=b`.
- **The system writes** when a stage names an update (ADR 0451), when the
  health check proves or fails it, or sees a start through (ADR 0453),
  and for `sysupdate unstage` and `sysupdate rollback`. Before a stage
  removes what the spare slot held, it writes, over both copies, a state
  that no longer names that slot, so that no start goes to a slot being
  rebuilt.
- **The menu shows after a start that was not seen through**, for 10
  seconds, or the configured timeout if longer, then starts the Dukovo
  entry by itself. Otherwise it stays hidden (ADR 0426). Its entries are:
  - The Dukovo entry, titled for what it starts: the version, *(update,
    first try)*, *second try* or *last try*, or *(the update to VERSION
    failed)*.
  - While an update is being tried, the version it updates, *(without
    the update)*, which leaves the tries as they are.
  - The previous version, which starts once without changing the
    default.
  - Windows, startup trouble and the firmware settings, as before.
- **With no whole copy**, which no single cut write leaves, slot A
  starts with slot B offered beside it, the menu shows, and GRUB writes
  nothing; the system writes the state afresh once it has started.
- **Mirrors.** GRUB counts on the partition it started from. The system
  writes every partition's copies, each over its own older copy, and
  goes by the state of the partition whose newest copy says it started
  the running slot, or else the first. The partitions' `grub.cfg` never
  changes after the install, so the two disks' menus stay in step, and
  each disk alone starts the slot the system last chose.
- **A model checks it.** `userspace/programs/slots/tla/SlotState.tla`
  models GRUB's writes, the system's, and a power cut at any point
  between or within them, over three stages. TLC checks that a whole
  copy always remains, that every start is of a slot that holds a
  system, that every try started was counted, and that no slot named in
  the state is being rebuilt. `ob test tla` runs it with the other
  models.

## Consequences

- The model found two flaws in the first draft, both fixed before
  merging: GRUB writing over the newer copy when the older could not be
  written could leave both copies torn, or give a failing update its
  tries back; and a stage that looked only at the newest copy before
  rebuilding the spare slot could leave an older copy that named it.
- A broken update costs at most three starts. A hang costs a try only
  when something restarts the machine: the hardware watchdog (ADR
  0143), or the owner.
- `slots`, a crate without the standard library, holds the format and
  every change to it, for sysupdate and for Spark. The image tests check
  the GRUB scripts against it, and the scripts were run under
  `grub-emu` with whole, torn and missing copies.
- **Not yet:**
  - On a mirror, the tries GRUB counts on one partition are not counted
    on the other until the system next writes the state, so a machine
    that starts from each disk in turn can try an update more than three
    times.
  - When GRUB cannot write, the menu's title still names the try that
    it then does not start.
  - Kani proofs of the copy reader.
