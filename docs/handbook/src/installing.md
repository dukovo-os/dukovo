<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Installing

This chapter describes installing Dukovo onto a computer's disk from the
install stick with `dukovo-install`, whose screens ask a few questions
and install, or which installs from an answers file, a JSON file that
answers every question the screens ask. The full references are
dukovo-install(8) and install-answers(5) ([ADR
0309](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0309-dukovo-install-installs-the-signed-release-as-an-answers-file-says.md),
[ADR
0310](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0310-the-installer-s-screens.md),
[ADR
0311](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0311-the-free-space-and-manual-layouts-and-swap-at-every-start.md),
[ADR
0314](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0314-installing-next-to-windows.md),
[ADR
0318](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0318-the-serial-console-ttys0-and-serial-only-installs.md),
[ADR
0319](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0319-setup-files-from-the-web-over-https-checked-by-sha-256.md)).

The guided layout **erases the disk it installs onto**. The layout next
to Windows shrinks Windows and keeps its files, the free-space layout
keeps what the disk holds, and the manual layout changes only what you
choose.

## Installing with the screens

1. Start the computer from the install stick
   ([Creating USB install media](usb-media.md)). Its first menu entry,
   *Install Dukovo*, starts in a few seconds and shows the installer.
2. Answer the six steps: the keyboard layout, the network and the
   computer's name, the disk, what the machine is for (a profile, and
   features such as File sharing to tick), your account, and the
   review. Tab moves,
   Space chooses, Enter goes on, Esc goes back, and F1 explains the page.
   On the time zone, type the start of its name, such as `america/den`.
3. At the review, check what will be erased or changed, type the
   disk's name in the box, and choose Install.
4. When the Done page shows, remove the stick and press Enter to
   restart, then sign in with the account you made.

To hear the installer, start the stick and wait for a short rising tune
from the computer's speaker: the menu is showing. Press S for *Install
Dukovo with speech*, and the screen reader says each screen as it shows,
in English, through the computer's sound output or a USB headset. Hold
Caps Lock and press H for the reader's keys ([The screen
reader](screen-reader.md)). A computer with no PC speaker plays no tune;
press S a few seconds after starting from the stick.

The first page's *Accessibility* fields take effect at once and are
kept on the installed system: *Screen reader* turns speech on or off,
*Text size* makes the text larger (Ctrl+Alt+Plus and Ctrl+Alt+Minus do
the same anywhere), *Sticky keys* lets Shift, Ctrl and Alt hold for the
next key, and *Slow keys* counts a key only once it is held for a
moment ([Text size and keys](text-size-and-keys.md)). With the reader
on, the installer says each page, the field with the focus and what
each key changes in it; it never says what a password field holds. The
field with the focus is marked with › in the margin as well as by its
colours.

Nothing is written to any disk before the review's Install. *Live
console* on the first page leaves the installer for a shell, where
`dukovo-install` starts it again, and the shell on Alt+F2 works
throughout.

## Choosing the layout

The Disk step offers five layouts:

- **The guided layout** erases the disk and gives it a BIOS boot
  partition, an EFI system partition, the swap asked for and the Dukovo
  root, and puts GRUB on it for both UEFI and legacy BIOS, so the disk
  starts either way.
- **Install next to Windows** shrinks Windows' C: and puts Dukovo in
  the space it gives: an EFI system partition of Dukovo's own, the swap
  and the root. Type the space for Dukovo, at least 20 GiB; Windows
  keeps 10 GiB free beyond what its files use. Before you start the
  installer, in Windows: turn BitLocker off on C:, turn Fast Startup
  off (Control Panel, Power Options, *Choose what the power buttons
  do*), and shut Windows down; the page says which of these is left.
  Start the stick by UEFI, from the firmware's boot menu. At each start
  a menu then offers Dukovo, *Windows Boot Manager* and the firmware's
  settings, and picks the one you chose last. Windows checks C: once at
  its first start after the install.
- **Mirror two disks** erases the disk chosen and a second one, which
  *Mirror onto* picks with Left and Right, and gives each the guided
  layout and GRUB. The two roots are one Btrfs RAID1, with a copy of
  everything on each disk, as big as the smaller disk; the larger one's
  extra space stays free. If a disk fails, the computer starts from the
  other, Dukovo runs as before and the kernel log warns that a disk is
  missing. Rebuilding the mirror onto a new disk is done by hand for
  now: rdisk(8) gives it the same partitions, replace(8) rebuilds the
  root onto it, and rgrub-setup(8) puts GRUB on it. Each disk has its
  own swap.
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

A setup file can also be on a web server, which suits many machines
with a file each. On the *From a setup file* page, choose *Download it
from a web address*, type its `https://` address and, to be sure it is
the file you meant, its SHA-256, which `sha256sum install.json` prints
where you wrote it. The installer downloads it over HTTPS only, so that
nobody can change it on the way, refuses it if its SHA-256 differs, and
the review then says where it came from and whether it was checked.

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
`guided` or `free`, as on the screens; `beside`, next to Windows, with
`space_gib`, the space Dukovo takes from Windows; `mirror`, with
`mirror` naming the second disk; or `manual`, whose
`changes` list each partition taken out, made or formatted, as the rdisk
screen saves them: install-answers(5) describes them.

## Installing over a serial port

A server with no screen installs over its first serial port, COM1, or
over a BMC's serial-over-LAN, which the board shows as COM1:

1. Connect a terminal program on another computer to the port at
   115200 baud, 8 data bits, no parity and one stop bit, as
   `screen /dev/ttyUSB0 115200` or PuTTY does. Make its window at least
   80 columns by 24 rows. The stick's menu shows there as it does on a
   screen.
2. Choose *Install Dukovo (serial console)*, the fourth entry. The
   installer's screens show on the port, the size of the terminal's
   window, and take the same keys.
3. Install as with the screens. The review's settings line ends with
   *serial console ttyS0*: the installed system keeps `console=ttyS0`
   on its command line, so that its sign-in is on the port too.

*Live console* on the first page gives root's shell on the port, where
`dukovo-install` starts the installer again. An answers file's
`serial_console` key makes the same choice for an install from a file
(install-answers(5)). serial-console(8) describes the port's console,
and `console=ttyS0,9600n8` on the command line runs it at another speed.

An install started with another port's `console=`, such as
`console=ttyS1` for a BMC's Serial-over-LAN on COM2 or `console=ttyS4`
for Intel AMT's, keeps that port: the installed system's sign-in is on
the port the installer ran on. [Supported hardware](hardware.md#serial-ports)
lists the ports' names.

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

The answers file can also come from a web server, over HTTPS only,
checked against its SHA-256 if you give it; a file that differs is
refused before anything is installed:

```sh
dukovo-install --answers https://setup.example.org/web01.json --sha256 "$(cat web01.json.sha256)"
```

The server's certificate must be from an authority the system trusts:
any public one, or your own once it is added as dukovo-certificates(7)
says.

The installer checks the whole answers file first, and names each
problem by its key, before it touches the disk. It then checks the
signatures of the release on the stick, partitions the disk, makes the
filesystems, copies the system, checking every file against the signed
list as it writes it, writes the settings and accounts, and puts GRUB
on the disk. It ends with:

```text
dukovo-install: Dukovo 0.15.0 is installed on nvme0n1. Remove the install stick, then restart.
```

Remove the stick and restart: the computer starts the installed system,
and the administrator signs in at the console with the password whose
hash the answers gave.

## Starting the installed system

An installed system starts quietly. The boot menu stays hidden, and
after the firmware's logo the screen shows Dukovo's name and a bar that
fills as the system starts, then the sign-in. To see the menu, hold
Shift as the computer starts, or press Esc in the second after the
firmware's logo; on a computer with Windows the menu always shows. To
see the messages the system prints as it starts, press Esc on the
startup screen. They show by themselves when something goes wrong, and
`dmesg` shows them afterwards (ADR 0426).

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
