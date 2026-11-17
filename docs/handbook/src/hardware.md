<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Supported hardware

Dukovo 1.0 runs on 64-bit Intel and AMD PCs, servers, virtual machines
and clouds. Disks, wired networks and USB work. Wi-Fi, graphics
acceleration and Bluetooth come in release 1.6, and full audio in 1.7.

<div class="hwkinds">
<div><span class="hwpill hw-ok">Works</span><h3>Virtual machines and clouds</h3><p>QEMU and KVM, VMware, Hyper-V and Azure, AWS, Google Cloud and VirtualBox. Every paravirtual disk and network card they offer has a driver.</p></div>
<div><span class="hwpill hw-ok">Works</span><h3>Desktops and servers on a wired network</h3><p>NVMe and SATA disks, Intel, Realtek, Broadcom and Killer Ethernet up to 10 Gb/s, and every kind of USB port. The screen is a text console until the desktop arrives in 1.1.</p></div>
<div><span class="hwpill hw-part">Partly</span><h3>Laptops</h3><p>Everything a desktop has, plus SD card readers and USB-C port status. Connect with an Ethernet port, a USB Ethernet adapter or a phone's USB tethering until Wi-Fi arrives in 1.6.</p></div>
<div><span class="hwpill hw-plan">1.6</span><h3>Gaming and graphics work</h3><p>Every graphics card shows a picture through the firmware's framebuffer. Acceleration for AMD, Intel and NVIDIA, several monitors and HDR come in 1.6.</p></div>
</div>

## Check your machine

Start the computer from any Linux live USB, such as Linux Mint or
Ubuntu, open a terminal and run:

```
lspci -nn
```

Paste everything it prints into the box below. The check runs in your
browser; nothing you paste is sent anywhere. It knows the PCI IDs of
every network card the drivers take, and the kind of every other part.

<div class="hwcheck" data-hwcheck></div>

On Dukovo itself, `dmesg | grep PCI` lists every part the kernel found,
and each driver logs the card it took.

The web console's **Hardware** page shows what any machine is and how it
fares, whatever its make: its model and processor, the last check, the
temperatures, the disks, the memory and the devices with the driver that took
each. See [Hardware](web-console.md#hardware).

## Hardware by category

Each status is one of: **Works**; **Partly**, which says what is
missing; **Planned**, with the release that brings it; or **Not yet**,
for hardware no release plans for so far. The driver's manual page lists
the exact chips.

<div class="hwfilter" data-hwfilter></div>

### Storage

| Device | Status | Driver |
| --- | --- | --- |
| NVMe SSDs, every drive, with a queue and interrupt for each CPU | Works | nvme |
| SATA disks and SSDs on every AHCI controller | Works | ahci |
| Intel VMD (Intel RST, "RAID On" on recent laptops): its NVMe and SATA disks, with no firmware change | Works | vmd |
| IDE and legacy SATA mode, on older machines | Works | ide |
| USB sticks and disks: bulk-only, UAS and USB floppy drives | Works | usb-storage |
| SD card readers on PCI and in chipsets, and Realtek's RTS5209, RTS5227, RTS5229, RTS522A, RTS5249, RTS525A, RTL8402, RTL8411 and RTL8411B | Works | sdhci, rtsx |
| PC floppy drives | Works | fdc |
| Hardware RAID and SAS HBAs: Broadcom MegaRAID and mpt3sas, Microchip SmartPQI | Planned, 1.12 | |
| UFS storage, in a few laptops from 2022 | Not yet | |

### Wired network

| Device | Status | Driver |
| --- | --- | --- |
| Intel 82540 to 82547 and 82571 to 82583 gigabit, and the chipset Ethernet from the 6 series to Nova Lake (82579, I217, I218, I219) | Works | e1000(4) |
| Intel I210, I211, I350, 82576 and 82580 | Works | igb(4) |
| Intel I225 and I226, 2.5 Gb/s | Works | igc(4) |
| Intel 82599, X540 and X550, 10 Gb/s | Works | ixgbe(4) |
| Realtek RTL8111, RTL8168, RTL8169 and RTL810x | Works | re(4) |
| Realtek RTL8125, RTL8126 and RTL8127, 2.5, 5 and 10 Gb/s, and Killer E3000 and E5000 | Works | rge(4) |
| Atheros AR813x, AR815x, AR816x and AR817x, Killer E2200, E2400 and E2500 | Works | alc(4) |
| Broadcom NetXtreme: BCM5717 to BCM5720 in servers, BCM5761, BCM5762, the BCM57765 family and BCM57780 | Works | bge(4) |
| USB Ethernet adapters and docks (CDC ECM and NCM), ASIX AX88179 and AX88179A, and phones' USB tethering | Works | cdc_ether(4) |
| Intel X710 and E810, Broadcom NetXtreme-E, Mellanox ConnectX: 25 to 100 Gb/s | Planned, 1.12 | |

### Wireless

| Device | Status | Driver |
| --- | --- | --- |
| Wi-Fi: Intel, Realtek, MediaTek (Wi-Fi 7 included) and Qualcomm chips | Planned, 1.6 | |
| Bluetooth: Intel, Realtek, MediaTek and Qualcomm controllers | Planned, 1.6 | |

### USB and ports

| Device | Status | Driver |
| --- | --- | --- |
| USB 3 and USB-C ports (xHCI) | Works | xhci |
| USB 2 and USB 1.1 ports (EHCI, OHCI, UHCI), with Intel's switch of ports to xHCI | Works | usbhc |
| USB keyboards and mice | Works | usb-hid |
| USB serial: CDC-ACM boards such as Arduino and Raspberry Pi Pico, FTDI chips | Works | usb-serial |
| USB-C port status: what is connected and its power role (UCSI) | Works | ucsi |
| USB4 and Thunderbolt docks: displays, PCIe and network through the cable | Planned, 1.6 | |
| Webcams (USB Video Class) and the Camera app | Planned, 1.6 | |

### Input

| Device | Status | Driver |
| --- | --- | --- |
| Built-in PS/2 keyboards | Works | i8042 |
| Touchpads and touchscreens: I2C-HID, Synaptics, Elantech and ALPS, TrackPoints | Planned, 1.1 | |
| Fingerprint readers and the Fingerprint app | Planned, 1.6 | |

### Graphics and displays

| Device | Status | Driver |
| --- | --- | --- |
| Any graphics card: one screen at its native resolution through the firmware's framebuffer, without acceleration | Partly | |
| AMD Radeon, every GCN and RDNA card and APU, with Vulkan | Planned, 1.6 | |
| Intel graphics from Haswell, with Vulkan and OpenGL | Planned, 1.6 | |
| NVIDIA RTX 20 series and later, and older cards through NVK | Planned, 1.6 | |
| Several monitors and HDR | Planned, 1.6 | |

### Audio

| Device | Status | Driver |
| --- | --- | --- |
| Built-in audio (HD Audio) on Intel and AMD chipsets: stereo output to speakers, headphones and line out | Partly | hda |
| USB headsets, speakers and DACs (USB Audio Class 1 and 2): output | Partly | usb-audio |
| Microphones, HDMI and DisplayPort audio, laptops' smart amplifiers | Planned, 1.7 | |

### Printing and power

| Device | Status | Driver |
| --- | --- | --- |
| Printers and scanners: driverless IPP Everywhere, AirPrint and eSCL, and drivers for older models | Planned, 1.7 | |
| Hibernation, battery health and charge limits | Planned, 1.7 | |

### Platform

| Device | Status | Driver |
| --- | --- | --- |
| 64-bit Intel and AMD processors, every core, with microcode updates | Works | |
| IOMMU: Intel VT-d and AMD-Vi, which keep every device to its own memory | Works | vtd, amdvi |
| Serial ports: every 16550, PCI serial cards, Intel AMT Serial-over-LAN | Works | uart(4) |
| Watchdogs (Intel TCO, AMD FCH), SMBus, GPIO and I2C controllers | Works | watchdog, smbus, i2c |
| Framework and ChromeOS embedded controllers: fans, LEDs and sensors | Works | crosec |
| 64-bit ARM | Planned, 1.17 | |
| RISC-V | Planned, 1.18 | |

### Virtual machines and clouds

| Device | Status | Driver |
| --- | --- | --- |
| QEMU and KVM: virtio disk, network, SCSI, console and sound | Works | virtio |
| VMware: VMXNET3, PVSCSI and the VMware Tools features | Works | vmxnet3(4), pvscsi(4), vmware(4) |
| Hyper-V and Azure: VMBus network and storage, Azure MANA | Works | hyperv, mana(4) |
| Amazon Web Services: ENA network and NVMe disks | Works | ena(4) |
| Google Cloud: gVNIC network, NVMe and virtio disks | Works | gve(4) |
| VirtualBox: its Intel network card and AHCI disks | Works | e1000(4) |

## How drivers reach a machine

The drivers that start the computer, for disks, USB, the keyboard and
wired networks, are built into Dukovo and never downloaded. From release
1.6, other drivers come from the Dukovo catalog, reviewed and signed:
Settings › Devices › Drivers lists each device and offers a missing
driver with one click, showing its license and what it can reach.
Drivers are matched on the machine, so no hardware ID is ever sent. Some
chips need firmware from their maker, which comes in the optional
firmware package.

## Common questions

**My laptop has only Wi-Fi. Can I use Dukovo now?** Yes, with a wired
connection: a USB Ethernet adapter (most use the standard CDC protocols
or an ASIX chip, which work), a dock, or a phone's USB tethering.

**Does my graphics card work?** Every graphics card shows the console
and, from release 1.1, the desktop, through the picture the firmware sets
up: one screen at its native resolution, with no 3D acceleration. Native
drivers come in 1.6.

**My disks are in "RAID On" or Intel RST mode.** Intel VMD, the mode
recent laptops ship in, works as it is. On older machines, set SATA
Operation to AHCI in the firmware setup, as for the Dell below. Hardware
RAID cards come in 1.12; until then Btrfs RAID across plain disks covers
most uses.

**Do I need to turn off Secure Boot?** Yes, for 1.0. Signed boot comes in
release 1.2. Cloud images boot without it.

**A part of mine is not listed.** Open an issue with the output of
`lspci -nn` and `lsusb`, as [Reporting a problem](reporting-a-problem.md)
describes. What people report decides which driver comes next.

## Reference machines

Three systems are tested on every change or release:

- **QEMU** (q35 machine, UEFI through OVMF and BIOS through SeaBIOS), in
  every CI run;
- the **Dell Inspiron 15 5570**, the reference laptop, before every
  release;
- the **Framework Desktop**, the reference desktop and bare-metal server
  (ADR 0079), before every release once it is set up.

### Dell Inspiron 15 5570

This is the reference machine as it is: Dell's parts list for its
service tag, with the two SSDs it has now in place of the factory disks.

| Component | Part | Dukovo support |
| --- | --- | --- |
| Processor | Intel Core i7-8550U (4 cores, 8 threads) | All 8 threads; early microcode with the optional firmware |
| Memory | 8 GB DDR4-2400, one module | Managed by the frame allocator and kernel heap (one NUMA node) |
| Graphics | Intel UHD Graphics 620 | Firmware framebuffer console; no native GPU driver in 1.0 |
| SSD | 128 GB Samsung PM871b, M.2 SATA (the factory part was an SK hynix SC311) | AHCI driver; Dukovo installs here |
| Second SSD | 512 GB TEAM T253, 2.5" SATA (in place of the factory 1 TB hard disk) | AHCI driver; kept for `/shared` |
| USB | Intel xHCI | xHCI driver |
| Keyboard | Built-in, PS/2 (i8042) | i8042 driver |
| Touchpad | I2C-HID (Intel Serial IO) | With the window manager, in release 1.1; a USB mouse works until then |
| Wired network | Realtek RTL810xE Fast Ethernet (PCI ID `10ec:8136`) | Realtek driver |
| Wi-Fi and Bluetooth | Qualcomm QCA9377 (Dell Wireless 1810) | After 1.0; needs closed firmware |
| Audio | Realtek ALC3246 | HDA driver in 1.0 |
| Serial port | None | Kernel output goes to the screen, and to the crash log for the next boot |

#### Firmware settings

Open the firmware setup by pressing **F2** at the Dell logo, and set:

- **System Configuration > SATA Operation: AHCI.** The laptop ships with
  "RAID On" for Intel Rapid Storage, which hides the disks from standard
  AHCI drivers. Windows on the SSD will not boot after this change unless
  it was prepared for it; Dukovo replaces Windows on this machine anyway.
- **Secure Boot > Secure Boot Enable: off.** Dukovo images are not signed
  for Secure Boot before 1.0.
- To test legacy BIOS boot as well, enable **Legacy Option ROMs** and the
  legacy boot list under **General > Boot Sequence**, if your firmware
  version offers them. UEFI is the normal way to boot Dukovo.

Press **F12** at the Dell logo for the one-time boot menu.

#### Smoke test

The latest USB image is booted on the laptop at the end of Phases 1, 2, 6
and 9, and of every later phase. Each run does these steps, with the
checks of every phase so far:

1. Build the image, with the laptop's microcode if the build host has
   Intel's (the `intel-microcode` package on Debian and Ubuntu), and write
   it to a USB stick as in [Creating USB install media](usb-media.md):

   ```sh
   ./ob image --microcode /lib/firmware/intel-ucode
   ```

2. Boot it from the F12 menu in UEFI mode. The boot log must appear on the
   screen and end with `Boot complete.` Check, and photograph:
   - `Firmware: UEFI` and a framebuffer of the panel's resolution, drawn
     with the 12x24 font;
   - the memory map, about 8 GB usable;
   - `CPUs: 8 of 8 online`, and the timer and IPI lines saying every CPU
     answered;
   - the `Time:` line (TSC rate, and which timer measured it);
   - the `microcode:` line: updated, or already current;
   - the `APIC:` line: x2APIC mode is expected on this processor.

   From Phase 2, also:
   - the `Random:` line: seeded from RDSEED, RDRAND and timing jitter
     (the i7-8550U has both instructions), with nothing failing its
     health test;
   - the `KASLR:` line: the kernel moved, and a different `Layout:` line
     on each boot;
   - `Paging: W^X holds`, and `SMEP on, SMAP on, PCID on; direct map in
     1 GiB pages` (the i7-8550U has all four);
   - the frame allocator's zones: one node, a `DMA32` zone below 4 GiB and
     a `Normal` zone above, about 7.8 GB free in all.
3. If the firmware offers legacy boot, repeat in BIOS mode.
4. Panic test: boot again with the `selftest=page-fault` flag. At the GRUB
   menu, press an arrow key to stop the countdown, press **e**, add
   ` selftest=page-fault` to the end of the `multiboot2` line, and press
   **Ctrl-X**. (Or build a second image with
   `./ob image --cmdline selftest=page-fault`.) The screen must show the
   panic report with a backtrace naming `radix::selftest::fault`.
5. Press a key to restart, press **F12** again and boot the stick, since
   the restart otherwise boots the internal disk. This boot must start by
   showing the panic of the previous one from the crash log.
6. From Phase 2: boot with `selftest=mm-stress` the same way. With 8
   CPUs it allocates and frees about 50 million objects, and the log must
   end with `selftest mm-stress: passed` before `Boot complete.` Note the
   time it reports.
7. From Phase 3: boot with `selftest=caps` the same way. The log must end
   with `selftest caps: passed` before `Boot complete.`, and the line
   before it must say how many derivations the 7 other CPUs tried during
   the revoke.
8. From Phase 4: check that the first `Scheduler:` line says `idle with
   MWAIT` (the i7-8550U has it) and the second `8 CPUs run threads, on 1
   NUMA node`. Then boot with `selftest=sched-rt`, then with
   `selftest=sched-balance`. Each log must end with `passed` before `Boot
   complete.`; note the timer latency `sched-rt` reports.
9. From Phase 5: check that the `User mode:` line says `SMEP on, SMAP on`.
   Then boot with `selftest=user`, then with `selftest=ipc`. Each log must
   end with `passed` before `Boot complete.` Then boot with
   `selftest=bench` and note the `BENCH` lines, which are the first
   numbers from real hardware.
10. In Phases 6 to 8: the log must end with Spark's lines, from `spark:
    Spark v1 is process 1` to `spark: nothing more to do; waiting`, with
    `hello` exiting with code `0x0` and all of its memory coming back.
11. From Phase 7: Spark must say `built the hierarchy of 10 directories
    and mounted 10 filesystems`. Photograph the `PCI:` lines, which list
    every PCI function with its vendor and device IDs and class: they
    name the SATA controller (class `0106xx`) and the Ethernet controller
    (class `0200xx`) that Phases 10 and 12 drive. No `Disk:` line is
    expected, since the laptop has no virtio disks and its SATA drives
    wait for the AHCI driver, so `/volumes` stays empty.
12. From Phase 9: the log gives way to the first virtual console, which
    shows `Dukovo` and `(tty1)` and a `#` prompt. Type `echo hello` on
    the built-in keyboard and check that `hello` comes back; press Caps
    Lock and check its light; press Alt+F2 for tty2 and Alt+F1 to come
    back. Run `dmesg | grep Keyboard`, which must say `PS/2 keyboard on
    the i8042 controller`, and `service --status-all`, which must list
    `[ + ]  consoled`. Then run `poweroff`: the screen shows the log
    again, ending with `Power: powering off (ACPI S5`, and the laptop
    must turn itself off.

Record the result in the table below, with the commit tested, and open an
issue for anything that fails, with the photos.

| Phase | Date | Commit | UEFI | Legacy BIOS | Notes |
| --- | --- | --- | --- | --- | --- |
| 6 | 2026-10-22 | `daa2682` | not recorded | not recorded | It booted without issue. Which firmware mode, and the individual checks above, were not recorded. |

#### Disk check

From Phase 10, each sub-phase also ends with the disk check on the
machine's disks (ADR 0122), which also measures the benchmarks of
`ob bench` on Dukovo and on Linux in turn (ADR 0124). It writes over the
disk it checks, so it checks only the disk whose serial number it is
given, and the image is built for that one disk. On this laptop both disks may be checked, one
at a time. Two USB sticks are needed: one for Dukovo, and one with a
Linux live system, Linux Mint or Ubuntu 22.04 or later.

1. Boot the Linux live stick and list the disks with their serial
   numbers:

   ```sh
   lsblk -d -o NAME,MODEL,SERIAL,SIZE
   ```

2. Build the image for the disk to check, and write it to the Dukovo
   stick as in [Creating USB install media](usb-media.md):

   ```sh
   ./ob image --disk-check SERIAL
   ```

3. Boot the Dukovo stick from the F12 menu and choose **Dukovo disk
   check: erases the disk with serial SERIAL**. The kernel's benchmarks
   run first, then the check, then the POSIX benchmarks: a few minutes
   in all. Photograph the `disktest:` lines. The last one must say
   `disktest: passed; the report is saved on the disk`.
4. Restart into the Linux live system with the Dukovo stick plugged in.
   Linux measures with the mitigations of CPU vulnerabilities off, as
   Dukovo has none yet: at the live stick's boot menu, press **e**, add
   ` mitigations=off` to the end of the line that starts with `linux`,
   and press **Ctrl-X** (or **F10**).
5. Open a terminal and run Linux's side, which reads back what Dukovo
   wrote, measures the same transfers on Linux, and asks before it
   writes:

   ```sh
   sudo sh /media/$USER/DUKOVO_*/crosscheck/run.sh
   ```

   It prints the comparison and saves it as `dukovo-disk-check.md` in
   the terminal's directory. Then it measures the benchmarks on Linux,
   and saves both systems' numbers as `dukovo-bench.txt` beside it. Send
   both files with the photos. While it measures, Linux runs as Dukovo
   does (ADR 0124): the processor's turbo off, no idle state deeper than
   C1, and the SATA links at full power; it puts each setting back at the
   end.
6. On the build machine, the benchmarks' report comes from that file:

   ```sh
   ./ob bench --from dukovo-bench.txt
   ```

### Framework Desktop

The second reference machine, from Framework's published specifications.
The parts get confirmed with `lspci -nn` and `lsusb` from a Linux live
USB when the machine is set up.

| Component | Part | Dukovo support |
| --- | --- | --- |
| Processor | AMD Ryzen AI Max+ 395 (Zen 5, 16 cores, 32 threads) | All 32 threads; AMD-Vi and machine checks |
| Memory | 128 GB LPDDR5x-8000, shared with the GPU | Frame allocator and kernel heap |
| Storage | Two M.2 NVMe PCIe 4.0 slots | NVMe driver, with a queue and an interrupt for each of up to 16 CPUs; Btrfs RAID1 across both |
| Wired network | Realtek RTL8126 5 GbE | Realtek driver |
| Platform | AMD FCH, SMU sensors, Framework embedded controller (open source) | Fans, LEDs, sensors and the watchdog |
| USB | Two USB4 Type-C, two USB-A 3.2 Gen 2 | xHCI; USB4 tunneling after 1.0 |
| Graphics | Radeon 8060S (RDNA 3.5) | Firmware framebuffer console; GPU driver after 1.0 |
| Audio | Realtek ALC623 | HDA driver in 1.0; full audio in release 1.7 |
| Wi-Fi and Bluetooth | MediaTek MT7925 | After 1.0; needs closed firmware |
| Firmware | Insyde UEFI, UEFI boot only | Turn Secure Boot off; boot the USB stick from the F12 menu |
| Serial port | None | Screen, crash log, and later netlog and the xHCI debug console |

This machine serves other work until Dukovo runs containers (Phase 18),
so Phase 10's checks run on the Dell. From then, the
[disk check](#disk-check) runs on this machine too, on the one NVMe drive
agreed for testing; the other drive is written only by agreement.

## Serial ports

Every 16550-compatible serial port is a terminal, named as Linux names
it, so a `console=` line from a server's manual or a Linux system works
unchanged
([ADR 0480](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0480-every-16550-is-a-terminal-with-linux-s-name-and-serial-log-chooses-the-log-s-ports.md)):

| Port | Name | Where it is found |
| --- | --- | --- |
| COM1 | `ttyS0` | I/O port 0x3F8, ISA interrupt 4 |
| COM2 | `ttyS1` | I/O port 0x2F8, ISA interrupt 3; most servers' BMC Serial-over-LAN |
| COM3 | `ttyS2` | I/O port 0x3E8; read every 10 ms, as its interrupt is COM1's |
| COM4 | `ttyS3` | I/O port 0x2E8, ISA interrupt 3, shared with COM2 |
| PCI cards and Intel AMT's Serial-over-LAN | `ttyS4` on | PCI, in the order found; MSI where the card has it |

`ls /kernel/class/tty` lists the ports a machine has, and each port's
folder says where it is and how it is read (radix-sysfs(5)). The
kernel's log, `kdb` and a panic report go to every port unless
`serial-log=` names the ones to use: `serial-log=ttyS0` keeps a UPS or a
modem on COM2 free of the kernel's text. `console=ttyS1,57600n8` runs a
port at another speed from boot and gives it a sign-in; it may be given
for more than one port. uart(4) and serial-console(8) have the details.

`serial` lists the ports with what each does at this start: its name, what
it is, whether it carries the kernel log, and its console. It also sets what
the next start does, so that no file on the boot partition is edited:

```sh
serial                           # the ports, and what the next start does
elevate serial console ttyS1,57600   # a BMC on COM2 at 57600 baud, also the boot menu's port
elevate serial console ttyS0 ttyS4   # COM1 and AMT's port, the last is the primary console
elevate serial log ttyS0,ttyS4       # keep the kernel's text off COM2
elevate serial console auto          # the firmware's console port again
```

A server whose firmware redirects its console to a serial port, as most
BMCs' settings do, says so in an ACPI table, SPCR. With no `console=` the
kernel takes that port as a console at the firmware's speed, so the
sign-in is there with no setting, and `serial` names the port. A
`console=` of any kind, or `spcr=off`, keeps the firmware's port from
being a console. GRUB's boot menu shows on a computer's serial ports as
the settings say: on the install stick on COM1 to COM4, AMT's port and
the firmware's port; on an installed system on the ports `serial console`
names, at their speeds, which GRUB runs only on a PCI card's first port
and only at 8 data bits and no parity. A change takes effect at the next
start ([ADR 0481](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0481-the-boot-menu-follows-the-serial-console-and-the-stick-installs-on-any-port.md)).
serial(8) has the details.

## Disk temperatures

`hardware sensors` lists every temperature the machine reads: each sensor of
the hardware monitors, each thermal zone of the firmware and each disk, with
its limits where it has them. diskhealth looks at each disk
every 10 minutes, and metricsd keeps the reading as the series
`temp{disk/NAME}`, so that `metrics show 'temp{disk/NAME}' --since 30d`
shows how hot a disk ran.

A look never wakes a disk. Before the system reads a SATA disk's health
record, it asks the disk which power mode it is in, with a command that no
disk spins up for, and reads nothing from a disk in standby. A disk that
sleeps keeps what was last read of it, and `hardware sensors` says so,
**sdb asleep since 08:15, not read**; its series has a gap for the time and
never a made-up point. A disk that gives no temperature says that.

A disk's limits are its own where it says them: an NVMe drive's come from its
health log, and a SATA disk's critical limit from the threshold of its airflow
temperature attribute where that counts down as the disk warms. Where it gives
none, the usual for its kind is used: a hard disk is warm at 50 °C and hot at
60 °C, a solid-state disk at 65 °C and 70 °C, and an NVMe drive at 70 °C and
80 °C. metricsd warns of a disk at its warning temperature after 5 readings in
a row ([MET-108](../../errors/MET-108.md)), the same way it warns of a hot
sensor, and the warning is critical at the second limit. It clears when the
disk is 3 degrees below the line it was raised at. The alert is of the kind
Hardware (see "Which alerts go where" in [Alerts: email, ntfy and
webhooks](alerts.md#which-alerts-go-where)). The web console shows each disk's
temperature, its highest in 30 days and its limit under **Temperatures and
fans** on the Hardware page (see [Temperatures and
fans](web-console.md#temperatures-and-fans)).
