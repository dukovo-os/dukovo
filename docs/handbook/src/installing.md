<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Installing

This chapter describes installing Dukovo onto a computer's disk from the
install stick, as of Phase 14a of the roadmap: `dukovo-install`, whose
screens ask a few questions and install, or which installs from an
answers file, a JSON file that answers every question the screens ask.
The full references are dukovo-install(8) and install-answers(5)
([ADR 0309](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0309-dukovo-install-installs-the-signed-release-as-an-answers-file-says.md),
[ADR 0310](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0310-the-installer-s-screens.md),
[ADR 0311](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0311-the-free-space-and-manual-layouts-and-swap-at-every-start.md)).

The guided layout **erases the disk it installs onto**. The free-space
layout keeps what the disk holds, and the manual layout changes only
what you choose.

## Installing with the screens

1. Start the computer from the install stick
   ([Creating USB install media](usb-media.md)). Its first menu entry,
   *Install Dukovo*, starts in a few seconds and shows the installer.
2. Answer the five steps: the keyboard layout, the network and the
   computer's name, the disk, your account, and the review. Tab moves,
   Space chooses, Enter goes on, Esc goes back, and F1 explains the page.
   On the time zone, type the start of its name, such as `america/den`.
3. At the review, check what will be erased or changed, type the
   disk's name in the box, and choose Install.
4. When the Done page shows, remove the stick and press Enter to
   restart, then sign in with the account you made.

Nothing is written to any disk before the review's Install. *Live
console* on the first page leaves the installer for a shell, where
`dukovo-install` starts it again, and the shell on Alt+F2 works
throughout.

## Choosing the layout

The Disk step offers three layouts:

- **The guided layout** erases the disk and gives it a BIOS boot
  partition, an EFI system partition, the swap asked for and the Dukovo
  root, and puts GRUB on it for both UEFI and legacy BIOS, so the disk
  starts either way.
- **Use the free space** keeps every partition on the disk and makes the
  same four in its largest free space. It is for a disk of data: one
  that starts a system already, with an EFI system partition or a BIOS
  boot partition as a Windows or Linux disk has, needs Manual, and the
  page says why it cannot be chosen.
- **Manual** opens the rdisk screen, which lists each disk's partitions
  as they are and as they will be. Choose a partition with Up and Down,
  then: N makes one in the free space, D takes one out (D again keeps
  it), F formats one, G lays out the guided partitions on the disk, and
  U undoes the last change; Tab shows the next disk. Each new or
  formatted partition has a use: the Dukovo root, the EFI system
  partition, swap, BIOS boot, data on Btrfs, exFAT or FAT32, or empty.
  Dukovo needs a root and an EFI system partition on the same GPT disk;
  without a BIOS boot partition, the disk starts by UEFI alone, and
  with one, the EFI system partition must be partition 2, as the screen
  numbers a new one where it can.

Swap partitions on the disk that holds Dukovo are turned on at every
start, and every data partition is mounted under `/volumes`, named for
its partition, such as `/volumes/nvme0n1p5`.

The Done page can also save your answers on the stick as a setup file,
`install.json`, without the passwords unless you asked to keep their
hashes. On the next computer, choose *From a setup file* on the first
page: the installer goes straight to the review.

## Writing the answers

An answers file names the disk, the machine's name, its time zone and
keyboard, any network cards with fixed addresses, and the accounts:

```json
{
  "version": 1,
  "keyboard": "us",
  "disk": { "layout": "guided", "target": "/dev/nvme0n1", "swap_gib": 8 },
  "hostname": "web01",
  "time_zone": "America/Denver",
  "admin": {
    "name": "Alice Liddell",
    "user": "alice",
    "password_hash": "$argon2id$v=19$m=65536,t=3,p=4$..."
  }
}
```

The file holds the password's hash, never the password. Make the hash on
any Dukovo system, the live system included, without the password being
shown or kept:

```sh
read -s pw; echo "$pw" | dukovo-install --hash-password; unset pw
```

`lsblk` lists the disks, to find the target's name. The layout is
`guided` or `free`, as on the screens, or `manual`, whose `changes` list
each partition taken out, made or formatted, as the rdisk screen saves
them: install-answers(5) describes them.

## Installing from an answers file

1. Start the computer from the install stick
   ([Creating USB install media](usb-media.md)) and choose *Try Dukovo
   (live console)*.
2. Put the answers file where the live system can read it: a second USB
   stick plugged in once the live system has started appears in
   `/volumes`, or type the file with `cat > answers.json`.
3. Install:

   ```sh
   dukovo-install --answers /volumes/MYSTICK/answers.json
   ```

The installer checks the whole answers file first, and names each
problem by its key, before it touches the disk. It then checks the
signatures of the release on the stick, partitions the disk, makes the
filesystems, copies the system, checking every file against the signed
list as it writes it, writes the settings and accounts, and puts GRUB
on the disk. It ends with:

```text
dukovo-install: Dukovo 0.14.0 is installed on nvme0n1. Remove the install stick, then restart.
```

Remove the stick and restart: the computer starts the installed system,
and the administrator signs in at the console with the password whose
hash the answers gave.

## When something goes wrong

Every step, every command the installer ran and what it printed are in
the install log, `/tmp/install.log`; read it from its end, from the
shell on another console (Alt+F2). A stick that fails the release's
check, or a file that does not match its signed size and SHA-256, stops
the install: write the stick again.

The installed system keeps the log as `/system/state/log/install.log`,
and its answers, without the passwords' hashes, as
`/system/settings/install/answers.json`: copy that file, change the
host name, and it installs the next machine the same way.
