<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Creating USB install media

`./ob image` builds `out/dukovo-<version>-x86_64.img`. Written to a USB
stick, it boots on UEFI and legacy BIOS computers. Its menu starts the
live system, which runs from memory and leaves the machine's disks
alone, with the installer on the first console ([Installing](installing.md))
or a shell on every console, or boots the machine's first disk. The stick also carries the
signed release of the system that the installer installs; to check that
a stick was written whole, run `system-release check` on its release
from the live system:

```sh
system-release check /volumes/sda3/release
```

[Installing](installing.md) installs it onto a disk.

## Writing the image

Any USB stick works; writing the image **erases everything on it**.

### With dd on Linux

1. Build the image:

   ```sh
   ./ob image
   ```

2. Plug in the stick and find its device name. `TRAN` shows `usb` for USB
   devices; make sure the size and model match your stick.

   ```sh
   lsblk -d -o NAME,SIZE,MODEL,TRAN
   ```

3. Unmount anything your desktop mounted from it, then write the image,
   replacing `sdX` with the name from step 2:

   ```sh
   sudo umount /dev/sdX?* 2>/dev/null
   sudo dd if=out/dukovo-0.15.0-x86_64.img of=/dev/sdX bs=4M conv=fsync status=progress
   ```

`conv=fsync` makes `dd` wait until the data is on the stick, so it is safe
to unplug when the command returns.

### With a graphical tool

These tools write the same image and check the target is removable:

- **balenaEtcher** on Linux, macOS and Windows: choose "Flash from file".
- **GNOME Disks**: select the stick, open the menu, choose "Restore Disk
  Image".
- **Rufus** on Windows: when asked, choose "Write in DD Image mode".

## Booting from the stick

1. Plug the stick in and switch the computer on.
2. Open the one-time boot menu (F12 on Dell computers) and choose the USB
   stick. It may be listed twice, once for UEFI and once for legacy boot.
3. GRUB shows a menu: *Install Dukovo*, which it starts after a few
   seconds, *Try Dukovo (live console)* and *Boot from first disk*.

The screen shows the kernel's log, under UEFI and legacy BIOS boot
alike, then the installer, or a shell with *Try Dukovo*. Switch the
computer off with the power button, or `poweroff` at a shell. Where the kernel hears the button, the log
says "Power: the ACPI power button asks Spark to power the machine off",
and a press stops the services and powers the machine off cleanly;
elsewhere the firmware handles the button. If the kernel panics
instead, the screen shows a report; [Debugging the kernel](debugging.md)
explains it, and what to include when you report it.

To include the processor's microcode updates, which Dukovo does not ship
itself, build the image with the build host's copy, for example from
Debian's or Ubuntu's `intel-microcode` package:

```sh
./ob image --microcode /lib/firmware/intel-ucode
```

Machine-specific settings, such as turning Secure Boot off, are in
[Supported hardware](hardware.md).

## The install DVD

For a computer that starts from a DVD drive, `./ob image --iso` builds
`out/dukovo-<version>-x86_64.iso`: the same menu, live system and
installer, and in its `release` directory the same signed release, on a
disc labelled `DUKOVO_LIVE` ([ADR
0433](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0433-the-install-dvd-carries-the-signed-release-and-is-found-by-its-label.md)).
Burn it to a blank DVD or CD with any burning program's "burn image"
choice, on Dukovo with `burn` ([ADR
0449](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0449-discs-are-burned-by-a-program-through-mmc-commands-the-kernel-passes.md)),
or on Linux with `xorriso`, replacing `sr0` with the drive's name:

```sh
burn dukovo-0.15.0-x86_64.iso
xorriso -as cdrecord -v dev=/dev/sr0 out/dukovo-0.15.0-x86_64.iso
```

Start the computer from the DVD drive as from the stick above. The live
system mounts the disc read-only at `/volumes/` and the drive's name,
such as `cd0`, and the installer installs from it as from the stick. To
check that the disc was burned whole:

```sh
system-release check /volumes/cd0/release
```

## Trying the image without a stick

`./ob run` boots the same image in QEMU, attached as a virtual USB stick on
an xHCI controller, which is close to what a real computer sees.
