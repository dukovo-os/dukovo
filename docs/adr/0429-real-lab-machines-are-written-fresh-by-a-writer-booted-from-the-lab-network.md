<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0429. Real lab machines are written fresh by a writer booted from the lab network

- Status: Accepted
- Date: 2026-11-11

## Context

The Dukovo Lab runs its tests on QEMU and on real machines (ADR 0401).
On QEMU, each test that boots afresh starts from a copy of the lab image.
A real machine has one disk, and the test before may have changed it in
any way. A test may have installed another system, filled the disk, or
broken its partition table. The lab's first real machine is a Dell
OptiPlex 7040 with Intel AMT. It is cabled to the lab PC: its network
port to a switch of its own, which the lab PC's second card is on, and
its serial port to a USB serial cable.

dukovo-os/lab's `amt` driver controls the machine through AMT. AMT turns
the machine on and off, and boots it from the network once. The driver
serves the image under test over HTTP and checks what comes back. Its
docs/net-boot.md is the contract. That leaves this side to supply three
things on the lab PC:

- something the machine can boot from the network that writes the image
  to its disk;
- a boot loader to load it;
- a DHCP and TFTP server, on the lab network alone.

Other ways to get a fresh disk were weighed:

- **AMT's storage redirection** (IDE-R). It boots the machine from an
  image on the lab PC, but as a CD or floppy of a few hundred megabytes
  at most, over a protocol with no free implementation to build on.
- **Restoring the disk from the system itself**, through a snapshot or
  a second partition. That trusts the system under test to restore
  itself, which is what the tests may have broken.
- **A person re-imaging the disk** from a stick. That makes every fresh
  boot a Needs you step.

## Decision

- **The writer.** `tools/lab-writer` is a small static Rust program,
  built for Linux. It runs as the init of an initramfs on the pinned
  Linux kernel of `ob bench` (ADR 0290), the same Linux the lab's real
  machines boot as their Linux twin, with no other user space. The
  initramfs holds the modules for the disk controllers and network
  cards of the lab's machines and of most PCs:
  AHCI, NVMe, Intel's e1000, e1000e, igb and igc, and Realtek's r8169
  with its PHY driver. The writer:
  - sets its own address from the kernel's command line, on the card
    with the MAC address given there;
  - reads the plan;
  - finds the disk with the plan's serial number. It writes nothing if
    none has that number, or if two do. No other disk is ever opened for
    writing;
  - zeroes the image's runs of zeros and the disk's last mebibyte, and
    writes the data as it streams in;
  - flushes the disk's cache, reads the image back past the page cache,
    and posts its SHA-256;
  - restarts only when the lab answers `restart`. On any other answer
    or failure it powers the machine off, and never boots into the
    network again.
- **The network GRUB.** The boot loader is the images' own GRUB, built
  from its pinned source (ADR 0306), as an x86-64 UEFI image. It has
  the network modules and a built-in script that sources the machine's
  boot script by its MAC address, `dukovo-lab/boot/<mac>.cfg`. When
  there is no boot script, it leaves for the firmware's next boot entry,
  the disk. It reads no other file.
- **dnsmasq on the lab card alone.** Plain `ob lab` sets the network up
  when dukovo-lab's machine files name a machine with the `amt` driver:
  - It builds the writer's kit and the network GRUB, and installs them
    in `/srv/dukovo-lab`.
  - It writes dnsmasq's configuration from the machine files, and runs
    dnsmasq (from `dnsmasq-base`) as the system service
    `dukovo-lab-dnsmasq`.
  - The lab card is the one card that holds the lab PC's lab address
    with a /24. `ob lab` refuses a card that carries the PC's default
    route, which would be the home network. dnsmasq binds that card
    alone and serves no DNS. It offers no gateway, and gives addresses
    only to the machines in the machine files, by MAC address. It serves
    TFTP from the TFTP folder.
  - Each step that would change nothing is skipped. The steps that
    change something ask for sudo, and the boot scripts' folder is left
    writable by the lab's user.
- **A check in CI.** `ob test lab-writer` runs the whole path in QEMU
  with OVMF. QEMU's own DHCP and TFTP stand in for dnsmasq, on a network
  laid out as the lab's. They serve the network GRUB, which loads the
  writer from a pretend lab, and the writer writes one of two disks.
  The check runs twice: once with a lab that takes the writer's hash,
  and once with a lab that refuses it. CI runs it in the job that holds
  the pinned Linux.

## Consequences

- A fresh boot on a real machine fetches only the image's data and
  zeroes the rest on the disk itself, and a test never runs on a disk
  that does not hold the image.
- The writer, GRUB and dnsmasq setup can be checked without the
  hardware. The hardware itself (its firmware's network boot, its card,
  its disk) is first checked when the lab suite runs on the machine.
- The lab PC needs a second network card for the lab, with the lab
  address and no gateway, and `dnsmasq-base`, which Mint, Ubuntu and
  Debian install with NetworkManager.
- A machine whose disk controller or network card needs a module not in
  the list is not written. The writer says which cards and disks it
  found, and adding the module is a change to `WRITER_MODULES` in
  `tools/xtask/src/lab_net.rs`.
- Secure Boot must be off on a lab machine: neither the network GRUB nor
  the pinned kernel is signed for it.
