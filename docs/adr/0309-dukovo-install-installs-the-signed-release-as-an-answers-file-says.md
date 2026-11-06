<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0309. dukovo-install installs the signed release as an answers file says

- Status: Accepted
- Date: 2026-11-06

## Context

Phase 14a, the third step of its third part. The plan's installer has
screens, guided, free-space and manual layouts, setup files from the
stick, the EFI system partition or `--answers`, an `answers.json` for
repeatable installs, and an install log; its exit test installs onto a
blank disk in QEMU under UEFI and a BIOS, restarts, and signs in. The
approved installer design lists the steps an install runs, shows each
step's commands in the log, and gives the answers file's keys: version,
keyboard, language, disk (layout, target, mirror, swap), profile,
features, host name, time zone, network cards, web console, SSH,
updates, firmware, the administrator and the other accounts.

The stick now carries a signed release of the system (ADR 0308), and an
installed system starts from its slot, `@system.a`, with Spark alone
(ADR 0307). What is missing is the program that puts the one onto a
disk as the other.

## Decision

- **The engine comes first, the screens next.** `dukovo-install
  --answers FILE`, a new program, installs from an answers file with no
  questions asked, from the live system or any Dukovo system with the
  stick plugged in. The screens of the next part ask the same questions,
  write the same file and run the same steps, so a screen install and a
  scripted one cannot drift apart, and the boot tests test both.
- **The answers file is the approved design's, version 1**
  (install-answers(5)). The installer checks the whole file before it
  touches a disk and names each problem by its key
  (`users[0].user: …`); a key it does not know is a problem, so that a
  typo is not silently ignored. The keys of later parts, `profile`,
  `features`, `web_console`, `ssh`, `updates` and `firmware`, are
  refused with a message saying so until those parts take them. Only
  `en-US` is taken until the installer's screens are translated. Only
  the guided layout is taken now: free space and manual come with the
  screens, mirror and next to Windows later in Phase 14.
- **An answers file holds passwords' hashes, never passwords**: an
  Argon2id PHC string, which `dukovo-install --hash-password` prints,
  or `""` for no password. The installed system keeps its answers as
  `/system/settings/install/answers.json` without the hashes, readable
  by anyone, unless `keep_password_hashes` asks, which makes it
  readable by root alone.
- **Each step runs the system's own tools**: `rdisk --script`,
  `mkfs-fat`, `mkswap`, `mkfs-btrfs`, `mount`, `subvolume` and
  `rgrub-setup`, each a child process whose command line and output go
  into the install log. The log then shows exactly what to type to look
  at, redo or finish a failed step, as the approved design's log does,
  and the installer has no second copy of the disk code. The log is
  `/tmp/install.log` as the install runs, readable from the rescue
  shell on another console, and `/system/state/log/install.log` in the
  installed system.
- **The release is checked twice and read once.** Its signatures are
  checked against the root the running system trusts before anything is
  written (`system-release check`'s first half). Then each file is
  checked against the signed list's size and SHA-256 as it is copied:
  a file longer than listed is refused as soon as it is, and one that
  does not match is removed and stops the install, naming the file and
  saying to write the stick again. A separate pass over the files first
  would read the stick twice for nothing.
- **The slot is made as updates will make one**: the system is copied
  into `@system.new`, kept as the read-only snapshot `@system.a`, and
  `@system.new` is deleted. Phase 15's updates make `@system.b` the
  same way, beside the slot that runs.
- **The guided layout** erases the disk and gives it a GPT with a 1 MiB
  BIOS boot partition, a 512 MiB EFI system partition, the swap asked
  for, and the Dukovo root in the rest: the layout of
  `ob image --installed` with swap. Every installed disk carries GRUB
  for both firmwares, `rgrub-setup DISK` for a BIOS and
  `rgrub-setup --efi PARTITION` for UEFI, so a disk moved to another
  machine starts there too.
- **The boot menu is the image's**: `ob` puts `installed.cfg`, the
  installed system's menu rendered with this build's timeout and
  command line, in `/system/boot/grub` beside GRUB's images, with
  `@ROOT_UUID@` left for the installer, which fills it in from the new
  root's Btrfs superblock and writes `boot/grub/grub.cfg` on the EFI
  system partition.
- **Settings are written as the system's own tools write them**: the
  host name (hostname(5)), the keyboard layout, the time zone as a link
  as timedatectl makes it, the network cards the answers name
  (network(5)), and the accounts, added to the default `passwd` and
  `group` as authd adds them, with the passwords' hashes in
  `/system/state/authd/passwords`, mode 0600. Spark adds every other
  default setting at the first start and keeps these.
- **The target is refused** unless it is a whole disk, not the stick's,
  with room for the layout, the release and 1 GiB more.
- **`mount -o compress` without `-t` tries only Btrfs**, the one
  filesystem that compresses, as Linux's mount finds the filesystem
  first and gives it the option. It tried FAT first, which the kernel
  refuses with the option, and gave up. The installer names each
  filesystem's kind anyway, so that its log reads plainly.
- **The boot tests `install-uefi` and `install-bios`** boot the stick
  under each firmware beside a blank 4 GiB virtio disk and run
  `dukovo-install --answers` with an answers file in the test image,
  `/system/tests/install/answers.json`, whose administrator's password
  is hashed at the tests' low cost; `mount -o compress` then finds the
  new root's subvolumes, the slot among them. On the host the disk must hold the
  guided layout, GRUB's `boot.img`, an EFI system partition dosfstools'
  `fsck.fat` finds sound with UEFI's GRUB and a menu naming the root,
  and a root Linux's `btrfs check` finds sound. The second boot is from
  that disk alone, as a SATA disk now, so the root is found by its UUID
  and not its name: the administrator signs in with her password and
  finds the host name, time zone, keyboard, accounts, slot, saved
  answers (without the hash) and log the install left. The harness
  gains the disk kind `INSTALL_TARGET` for it, which the second boot
  starts from without the stick.

## Consequences

- One answers file installs the same system on every machine it is
  given to, and every installed system says how it was installed.
- A damaged stick stops the install at the first bad file, before the
  system is half there and starts, and the log names the file.
- The swap partition is made and formatted but not yet turned on when
  the system starts: Dukovo has no `/etc/fstab` and nothing turns on
  swap at boot. Spark turning on the swap partitions of the root's disk,
  as systemd's GPT auto-generator does on Linux, comes with the next
  part.
- The install copies no CPU microcode to the EFI system partition yet;
  `ob image --installed` carries none either.
- *Boot from first disk* (ADR 0308) is still untested: the next part,
  which adds *Install Dukovo* and *Install with serial console* to the
  stick's menu, also boots the stick beside an installed disk and
  chooses it.
- Radix frees what a process held when the reaper takes its last
  thread, a moment after its parent has seen it end, where Linux closes
  a process's files before `wait` returns. Deleting `@system.new` right
  after `subvolume snapshot` read it is then refused now and then as in
  use, so the installer tries a busy delete or unmount again for up to
  5 seconds. Freeing a process's handles before its parent is told it
  ended is the kernel's fix, for later.
- The boot archive carries the tests' answers file in `/system/tests`,
  beside the other tests' programs and files, so every image has it; its
  hash opens nothing on a system not installed with it.
