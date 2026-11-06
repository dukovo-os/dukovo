<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Installing

This chapter describes installing Dukovo onto a computer's disk from the
install stick, as of Phase 14a of the roadmap: `dukovo-install`, which
installs from an answers file, a JSON file that answers every question
the installer would ask. The installer's screens, which ask those
questions and write the same file, come next. The full references are
dukovo-install(8) and install-answers(5)
([ADR 0309](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0309-dukovo-install-installs-the-signed-release-as-an-answers-file-says.md)).

Installing **erases the disk it installs onto**.

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

`lsblk` lists the disks, to find the target's name. The guided layout,
the only one so far, gives the disk a BIOS boot partition, an EFI system
partition, the swap asked for and the Dukovo root, and puts GRUB on it
for both UEFI and legacy BIOS, so the disk starts either way.

## Installing

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
