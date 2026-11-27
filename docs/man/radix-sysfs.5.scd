radix-sysfs(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

radix-sysfs - the kernel's view of the hardware and its settings, one
value per file in /kernel

# DESCRIPTION

A *sys* filesystem, which Spark mounts at _/kernel_ (*spark*(8)), with
_/sys_, Linux's name for it, a link to it (*radix-files*(7)), holds the
kernel's view of the hardware and its settings, one value per file, each
ending in a newline. It is read-only. Every file is written when it is
opened, and its size in *stat* is 0 until then, so read a file to its end
rather than by its size.

Wherever Linux has the same thing, the path beneath the folder and the
format are Linux's, so that ported tools find it where they look through
the link: *lspci* reads _/sys/bus/pci/devices_, *lsblk* reads
_/sys/block_. The files at the top, _version_, _cmdline_ and _cpus_, are
Radix's own.

This page describes _/kernel_ as of Phase 11 of the roadmap; later phases
add devices and settings, and settings that can be changed.

# FILES

_/kernel/version_
	The kernel's name and version, and the Dukovo release it is part
	of: *Radix 0.17.0 (Dukovo Server 0.17.0)*. The C library gives the
	second word as the release, and the whole line as the version, in
	uname(3)'s *struct utsname*.

_/kernel/cmdline_
	The kernel command line the boot loader gave (*dukovo-boot*(7)).

_/kernel/boot/times_
	How long starting took before Spark (ADR 0474), in nanoseconds, a line
	each, where it is known: *firmware_ns* and *loader_ns*, from the
	firmware's Basic Boot Performance Table (ACPI's FPDT), how long the
	firmware took to start the boot loader and how long the loader took,
	to the end of the firmware's boot services, which the kernel reads
	only where the UEFI memory map keeps the table for the firmware; and
	*spark_ns*, when the kernel started Spark, by its own clock, since the
	kernel started. A computer whose firmware keeps no such table shows
	the last line alone. *startup*(8) reads it. Linux shows the first two
	at _/sys/firmware/acpi/fpdt/boot/_.

_/kernel/boot/memtest_
	The result of the memory test that *memtest*=once ran at this start
	(*dukovo-boot*(7)), there only when it ran: a line each, *result
	passed* or *result errors*; *complete yes*, or *complete no* when it
	stopped after an hour; *tested* and *total*, the bytes it tested and
	the bytes the kernel manages; *errors*, the words that read back
	wrong; *seconds*; *retired*, the pages it took out of use; then a
	line *bad* _first_ _last_ _errors_ for each range of pages with
	errors, at most 64, the first and last byte in hexadecimal.
	*hardware*(8) records it and raises an alert when it found errors.

_/kernel/cpus_
	The CPUs online.

_/kernel/block/_*name*_/size_
	The size of a disk or partition in 512-byte sectors, whatever its
	block size, as on Linux.

_/kernel/block/_*name*_/ro_
	*1* if the device is read-only, *0* if not.

_/kernel/block/_*name*_/removable_
	For a disk: *1* for a drive whose medium comes and goes, such as a
	card reader's slot, a floppy drive or a USB stick that says its
	medium is removable, *0* for any other, as on Linux (ADR 0430). With
	no medium in it, the drive's _size_ is *0*.

_/kernel/block/_*name*_/media_changes_
	For a drive whose medium comes and goes: how many times a medium
	went in or came out since the drive was found. *volumed*(8) mounts
	and unmounts when it moves. Radix's own: Linux tells udev instead.

_/kernel/block/_*name*_/disc_
	For a CD, DVD or Blu-ray drive (ADR 0431): what is in it. *none*
	for no disc; *unreadable* for a disc the drive cannot read; or the
	disc's kind, as MMC's profiles name it (*cd-rom*, *cd-r*, *cd-rw*,
	*dvd-rom*, *dvd+r*, *dvd-r-dl*, *bd-rom*, *bd-re* and the others),
	and what it holds: *data*, *audio* for a music CD, *mixed* for a CD
	of both, or *blank*. Only a disc of data is read: with any other,
	the drive's _size_ is *0*, and *audiocd*(1) reads the tracks of a
	disc of audio. Radix's own: Linux's udev has it as
	_ID_CDROM_MEDIA_ and its kin.

_/kernel/block/_*name*_/formats_
	For a floppy drive, PC or USB (ADR 0436): the sizes of disk it
	formats, as *floppy*(1) names them, on one line: *1.44M 720K* for a
	3.5-inch high-density drive, *720K* for a double-density one.
	Radix's own.

_/kernel/block/_*name*_/eject_requests_
	For a CD, DVD or Blu-ray drive (ADR 0432): how many times its eject
	button was pressed while its disc was locked in, as the drive says
	in its media events. The drive keeps a mounted disc in when its
	button is pressed; *volumed*(8) unmounts the disc and ejects it when
	the count moves. Radix's own: Linux tells udev of a press as
	_DISK_EJECT_REQUEST_.

_/kernel/block/_*name*_/partition_, _start_
	For a partition: its number in its disk's table, the one its name
	ends in, and its first sector on the disk, in 512-byte sectors. A
	partition has these instead of a _queue_ directory, as on Linux,
	whose tools read the queue of the partition's disk.

_/kernel/block/_*name*_/parttype_
	For a partition: its type, as *lsblk*(8)'s PARTTYPE and *blkid*(8)
	give it: its MBR type byte in hexadecimal (*0x0c*, *0xef*), or its
	GPT type's GUID in lowercase
	(*c12a7328-f81f-11d2-ba4b-00a0c93ec93b* for an EFI system
	partition). Linux has it only through udev; Radix has it here.

_/kernel/block/_*name*_/device/model_, _serial_, _firmware_rev_
	For a SATA, NVMe or USB disk or an SD card: its model, serial number and firmware
	revision, as it gives them, without the padding. A USB disk's model
	is its SCSI vendor and product, its firmware its SCSI revision, and
	its serial number its USB device's, as udev's *ID_SERIAL_SHORT* has
	it, up to 64 bytes. An SD card's model is its product's name, its
	serial number the CID's in hex (_0xdeadbeef_) and its firmware the
	CID's revision (_1.0_), which Linux has as _name_, _serial_ and
	_prv_; an empty slot has none (ADR 0437). Linux has these for
	an NVMe disk; for a SATA disk it cuts the model to 16 characters and
	keeps the serial number elsewhere. A tool that must write to one
	particular disk should find it by its serial number, since disk names
	follow the order the controllers are found in.

_/kernel/block/_*name*_/queue/logical_block_size_, _physical_block_size_
	The sizes in bytes of the device's logical blocks, the least it
	reads or writes (*512*, or *4096* for a disk with 4 KiB sectors),
	and of the blocks it writes without reading first (*radix-block*(2)).
	Offsets and sizes elsewhere stay in 512-byte sectors, as on Linux.

_/kernel/block/_*name*_/queue/minimum_io_size_, _optimal_io_size_
	The smallest write it prefers, its physical block size, and the
	size it does best, *0* as it does not say.

_/kernel/block/_*name*_/queue/rotational_
	*1* for a hard disk, or a disk that does not say; *0* for a solid
	state disk.

_/kernel/block/_*name*_/queue/discard_granularity_, _discard_max_bytes_
	Its logical block size, and the most bytes one discard (TRIM)
	covers, if the device discards; *0* and *0* if not.

_/kernel/block/_*name*_/stat_
	For a disk: what it has done since it was found, as the 17 numbers
	of Linux's _/sys/block/_*name*_/stat_, in its order and widths:
	reads, reads merged, sectors read, milliseconds reading; the same
	four of writes; requests in flight, milliseconds with one or more in
	flight, and milliseconds of every request added up; the same four of
	discards; flushes and milliseconds flushing. Sectors are of 512
	bytes. Radix merges no requests, so those numbers are *0*; a call
	that moves many buffers at once counts as one request. *iostat*(1)
	reads it.

	Disks and partitions are named as Linux names them: _vda_, _vdb_ for
	virtio disks, _sda_, _sdb_ for SATA and USB disks, _nvme0n1_ for the first
	namespace of the first NVMe controller, _mmcblk0_ for the first SD
	card reader's first slot, _vda1_ for the first partition of _vda_ and _nvme0n1p1_ for the
	first of _nvme0n1_. Linux puts a
	partition's directory inside its disk's (_/sys/block/vda/vda1_) and
	lists both in _/sys/class/block_; Radix lists both here.

_/kernel/bus/pci/devices/_*address*_/vendor_, _device_
	The PCI function's vendor and device IDs, in hexadecimal with four
	digits: *0x1af4* and *0x1001* for a virtio block device as QEMU
	offers it (*0x1042* when it has no legacy interface).

_/kernel/bus/pci/devices/_*address*_/class_
	Its class code, the base class, subclass and programming interface,
	in hexadecimal with six digits: *0x010000* for a SCSI-like storage
	controller, *0x010601* for an AHCI one, *0x010802* for an NVMe one.

_/kernel/bus/pci/devices/_*address*_/revision_
	Its revision, in hexadecimal with two digits: *0x02*.

_/kernel/bus/pci/devices/_*address*_/subsystem_vendor_, _subsystem_device_
	The vendor and model of the board around it, in hexadecimal with
	four digits: *0x1af4* and *0x1100* on QEMU. A bridge's are in its
	Subsystem ID capability; *0x0000* for a function that has none.

	A function's address is its PCI segment, bus, device and function,
	as Linux writes it: _0000:00:03.0_. Every function the kernel's scan
	found is listed, whether or not a driver uses it. *lspci*(8) reads
	them.

_/kernel/bus/pci/devices/_*address*_/numa_node_
	The NUMA node the function is closest to, in decimal, as Linux's
	file: *0* or *1* on a two-socket server. The firmware gives each
	PCI root bus a node (the proximity domain of its ACPI host bridge), and every
	function on that bus has it. The file reads *-1* when the node is not
	known: the firmware names none, the machine has no NUMA tables,
	ACPI is off (*aml=off*) or has not started. The scan finds every root
	bus, not only the first, so a second socket's devices are listed
	with the node they belong to.

_/kernel/bus/usb/devices/_*name*_/idVendor_, _idProduct_, _bcdDevice_, _version_
	A USB device's vendor and product IDs and its release, in
	hexadecimal with four digits, and the USB version it speaks, as
	Linux writes it, with a space in front: *046d*, *c52b*, *1211* and
	*2.00*. Its device
	descriptor's other fields are in _bDeviceClass_, _bDeviceSubClass_,
	_bDeviceProtocol_ (two hexadecimal digits), _bMaxPacketSize0_ and
	_bNumConfigurations_; its configuration's in _bConfigurationValue_,
	_bNumInterfaces_, _bmAttributes_ and _bMaxPower_ (*100mA*).

	A device's _name_ is its bus and its ports from the root hub down, as
	Linux names it: _1-3.2_ for port 2 of the hub on root port 3 of bus 1.
	Each controller is a bus, numbered from 1 as the kernel finds them.

_/kernel/bus/usb/devices/_*name*_/busnum_, _devnum_, _devpath_, _speed_, _maxchild_
	Its bus; its number on the bus, from 2, each device set up taking
	the next one free, as on Linux; its ports, _3.2_; its speed in Mb/s,
	*1.5*, *12*, *480*, *5000* or *10000*; and a hub's ports, else *0*.

_/kernel/bus/usb/devices/_*name*_/manufacturer_, _product_, _serial_
	Its strings, read when it was set up, for those it has.

_/kernel/bus/usb/devices/_*name*:*c*.*i*_/bInterfaceNumber_, _bInterfaceClass_, _driver_
	Interface _i_ of its configuration _c_: its descriptor's fields, as
	Linux shows them (_bInterfaceNumber_, _bAlternateSetting_,
	_bNumEndpoints_, _bInterfaceClass_, _bInterfaceSubClass_,
	_bInterfaceProtocol_), and _driver_, a link to the driver that took
	it in _/kernel/bus/usb/drivers_: *hub*, *usbhid*, *usb-storage*,
	*uas*, *cdc\_acm*, *ftdi\_sio*, *cdc\_ether*, *cdc\_ncm*,
	*ax88179\_178a* or *snd-usb-audio*.

_/kernel/bus/usb/devices/_*name*:*c*.*i*_/ttyUSB_*n*, _tty/ttyACM_*n*
	The terminal a serial port's interface is, where Linux shows it: an
	FTDI chip's, _/devices/ttyUSB_*n*, as a folder in the interface's
	own; a CDC-ACM device's, _/devices/ttyACM_*n*, in its _tty_ folder.
	*brailled*(8) finds a braille display's terminal so (ADR 0326).

_/kernel/bus/usb/devices/usb_*n*
	Bus _n_'s root hub: _busnum_; _devnum_ *1*; _devpath_ *0*; _speed_ and
	_version_, *5000* and *3.00* when some of its ports are USB 3's;
	_maxchild_, the controller's root ports; _bDeviceClass_ *09*;
	_product_, *xHCI Host Controller*; and _serial_, the controller's
	PCI address. Linux gives a root hub the Linux Foundation's IDs;
	Radix gives it none. *lsusb*(8) reads all of these (ADR 0230).

_/kernel/bus/i2c/devices/i2c-0/name_
	When the kernel drives the chipset's SMBus controller (ADR 0206),
	its adapter's name, as Linux gives it: *SMBus I801 adapter at 0700*
	for Intel's, with its I/O port in hexadecimal, or *SMBus PIIX4
	adapter port 0 at 0b00* for that of AMD's FCH.

_/kernel/bus/i2c/devices/0-00_*nn*_/name_, _eeprom_
	A memory module's SPD EEPROM on the SMBus, at address *nn*
	(*50* to *57*), found as Linux finds them, by the memory's kind in
	the SMBIOS tables (ADR 0232): _name_ is the kind of EEPROM, as Linux
	names its driver, *spd* (DDR3 and older), *ee1004* (DDR4) or
	*spd5118* (DDR5); _eeprom_ is its bytes, 256, 512 or 1024 of them,
	read from the module the first time the file is opened, which takes
	up to a second, and kept. A read that fails is an I/O error.
	*hwinfo*(8) decodes them.

_/kernel/bus/i2c/devices/i2c-_*n*_/name_
	Each I2C bus of a DesignWare controller, Intel LPSS's or AMD's,
	numbered from *1* in the order the kernel found them (ADR 0234):
	*Synopsys DesignWare I2C adapter*, as Linux names it.

_/kernel/bus/i2c/devices/i2c-_*HID*:*nn*_/name_
	Each device the firmware names on one of those buses, by its ACPI
	name, as Linux names it: its _\_HID_ and its place among the devices
	of that _\_HID_ on the buses, in hexadecimal, as *PNP0C50:00*.

_/kernel/firmware/microcode_
	What became of the CPU microcode the boot loader gave the kernel,
	there only when it gave some (ADR 0468), a line each: *size*, the
	module's bytes; *updates*, how many updates it holds; *result*:
	*applied*, *current* (the processor runs that revision or a newer
	one), *unmatched* (no update is for this processor), *unsupported*
	(not an Intel processor), *hypervisor* (the update did not take
	effect under a hypervisor, which keeps its own), *damaged* (data that
	failed its checks; none of it is used) or *refused* (the processor
	did not take it); *revision*, the revision the processor runs; and
	with *refused*, *refused*, the revision it was asked to take. The
	health check of *sysupdate*(8) reads it.

_/kernel/firmware/memmap/_*n*_/start_, _end_, _type_
	The firmware's memory map, as the boot loader gave it to the kernel,
	a directory a range numbered from *0* in its order, as Linux's
	_/sys/firmware/memmap_: the range's first and last byte, in
	hexadecimal (*0x0* and *0x9fbff*), and its type, in Linux's names:
	*System RAM*, *Reserved*, *ACPI Tables*, *ACPI Non-volatile Storage*
	or *Unusable memory*. *lsmem*(1) reads it.

_/kernel/fs/btrfs/_*uuid*_/label_, _disks_, _missing_, _behind_, _profiles_, _devices_
	A directory for each Btrfs filesystem mounted, named by its ID as
	Linux's _/sys/fs/btrfs_ names it (*2f3c9a61-0b7e-4d2a-9c11-5e8f0a7d4b23*),
	and read anew each time a file is opened (ADR 0484): its label, or an
	empty line; how many disks it has; how many of them are missing,
	missing at the mount or failed since (ADR 0153), so that it holds a
	second copy of nothing they held until each is replaced; how many
	missed changes, which are read last until a scrub catches them up;
	how its block groups keep their copies; and the disks and partitions
	it is on now, a name of _/devices_ a line, as Linux's _devices_
	folder has them, so that anyone may tell which disks a volume uses
	without opening them. *metricsd*(8) watches the counts, and raises an
	alert when a disk is missing. *storage*(8) reads all of them.

	*profiles* has a line for each kind of block group the filesystem
	has: *data*, *metadata* and *system*, in that order, and after the
	kind the profile of its chunks: *single* (one copy), *dup* (two
	copies on one disk), *raid0*, *raid1*, *raid1c3*, *raid1c4*, *raid10*,
	*raid5* or *raid6*, as *btrfs filesystem df* names them. While a
	balance (*balance*(8)) converts a kind from one profile to another, some
	of its chunks have the old profile and some the new, and the line has
	both: always in the order *single*, *raid0*, *raid1*, *dup*,
	*raid10*, *raid5*, *raid6*, *raid1c3*, *raid1c4*, which is not the
	order of the conversion. A kind with no chunk has no line.

```
data raid1
metadata raid1
system raid1
```

```
data single raid1
metadata dup
system dup
```

_/kernel/fs/space_
	How full each filesystem mounted is (ADR 0484), a line each in the
	order of _/processes/mounts_: where it is mounted, escaped as there;
	the size of its blocks in bytes; how many blocks it has and how many
	are free; and how many files it can hold and how many more, *0* for a
	filesystem that keeps no count, such as FAT. The kernel's own
	filesystems and FUSE ones are left out. A service's view holds
	_/kernel_ but not every mount, so *metricsd*(8) reads the space of
	each here rather than by *statvfs*(3).

	```
	/ 4096 7864320 5242880 0 0
	/volumes/backup 4096 488378646 401235117 0 0
	```

_/kernel/class/dmi/id/sys_vendor_, _product_name_, _product_version_, _product_family_, _product_sku_
	Where the firmware has SMBIOS tables (ADR 0231), what it calls the
	machine: who made it, its product name, version, family and SKU, as
	Linux's _/sys/class/dmi/id_ has them. A file the firmware gives no
	string for is not there. The machine's serial number and UUID, which
	Linux shows to root alone, are not here: *hwinfo*(8) reads them
	through the platform capability (*radix-platform*(2)).

_/kernel/class/dmi/id/board_vendor_, _board_name_, _board_version_, _board_asset_tag_
	Its main board's maker, name, version and asset tag.

_/kernel/class/dmi/id/chassis_type_, _chassis_vendor_, _chassis_version_, _chassis_asset_tag_
	Its chassis's type, a number of the specification's (DSP0134's
	7.4.1: *3* a desktop, *10* a notebook), maker, version and asset tag.

_/kernel/class/dmi/id/bios_vendor_, _bios_version_, _bios_date_, _bios_release_, _ec_firmware_release_
	Its firmware's vendor, version and date (_MM/DD/YYYY_), and the
	releases of the firmware and of the embedded controller's, as
	_major.minor_.

_/kernel/class/watchdog/watchdog0/identity_
	The hardware watchdog the kernel drives, if any (*spark*(8)'s
	*WATCHDOG*): _the Intel 6300ESB watchdog_, _the Intel ICH9 TCO
	timer_, _the Intel ICH10 TCO timer_, _the Intel PCH TCO timer_ or _the
	AMD FCH watchdog_. Without one, a battery or AC adapter, or SMBIOS
	tables, _/kernel/class_ is not there.

_/kernel/class/watchdog/watchdog0/state_
	*active* while it runs, else *inactive*.

_/kernel/class/watchdog/watchdog0/timeout_, _min_timeout_, _max_timeout_
	Its timeout in seconds, as last set (0 before it first runs), and the
	shortest and longest it takes.

_/kernel/class/watchdog/watchdog0/bootstatus_
	*32*, Linux's WDIOF_CARDRESET, if it restarted the machine at the end
	of the boot before this one; *0* if not.

_/kernel/class/tty/ttyS_*n*_/port_, _console_
	Each serial port the kernel found, by its terminal's name, as
	Linux's _/sys/class/tty_ shows them (ADR 0480): COM1 to COM4 are
	*ttyS0* to *ttyS3*, and the ports of PCI cards and Intel AMT follow
	from *ttyS4*. _port_ is its first I/O port, as *0x2F8*; _console_ is
	*Y* if it carries the kernel log, *kdb* and a panic report, as the
	kernel flag *serial-log=* chooses, and *N* if not (*dukovo-boot*(7)).
	Without a serial port, _/kernel/class/tty_ is not there.

_/kernel/class/tty/ttyS_*n*_/kind_, _pci_, _interrupt_
	Radix's own. _kind_ is *com1* to *com4*; *amt-sol* for Intel AMT's
	Serial-over-LAN port; *ax99100* for a port of an ASIX AX99100 card;
	*qemu* for QEMU's *pci-serial*; or *pci* for any other PCI 16550.
	_pci_, only for a port on PCI, is its function's address, as
	*0000:00:16.3*. _interrupt_ is how typing reaches the kernel: *isa 3*
	or *isa 4*, the ISA interrupt; *msi*; or *polled*, every 10 ms while
	the terminal is open, for COM3, whose interrupt is COM1's, and a PCI
	port without MSI.

_/kernel/class/tty/ttyS_*n*_/firmware_console_
	Only on the port the firmware sends its console to, as the ACPI
	table SPCR names it (ADR 0481; *uart*(4)): the line the firmware
	gives it, as Linux's console options and a newline, *57600n8*. It is
	there whatever the kernel's command line says, and whether or not
	the port is a console; *serial*(8) and *serial-console*(8) read it.

_/kernel/class/power_supply/_*name*_/type_, _online_
	For each AC adapter of the firmware's (_ACPI0003_, through uACPI;
	*dukovo-boot*(7)'s *ACPI*), by its name in the ACPI namespace, as
	*ADP1*: *Mains*, and *1* while it is plugged in, else *0*.

_/kernel/class/power_supply/_*name*_/type_, _present_, _status_
	For each battery of the firmware's (_PNP0C0A_), by its name, as
	*BAT0*, as Linux's ACPI battery shows it: *Battery*; *1* while it is
	in its bay, else *0*; and *Charging*, *Discharging*, *Full* or *Not
	charging*. Its state is read again when a file is read a second
	after the last read, or after the firmware notifies a change.

_/kernel/class/power_supply/_*name*_/capacity_, _capacity_level_
	What it has left, in percent of its last full charge (or, without
	one, of its design capacity), and *Critical*, *Low*, *Full* or
	*Normal*.

_/kernel/class/power_supply/_*name*_/energy_full_design_, _energy_full_, _energy_now_, _power_now_
	For a battery that counts in watts: its design capacity, its last
	full charge and what it has left, in µWh, and the rate it charges or
	discharges at, in µW. A battery that counts in amperes has
	_charge_full_design_, _charge_full_ and _charge_now_ (µAh) and
	_current_now_ (µA) instead.

_/kernel/class/power_supply/_*name*_/voltage_min_design_, _voltage_now_, _cycle_count_, _technology_
	Its design and present voltage, in µV; how many times it has been
	charged; and *Li-ion*, *Li-poly*, *NiMH*, *NiCd* or *Unknown*.

_/kernel/class/power_supply/_*name*_/model_name_, _manufacturer_, _serial_number_
	What the battery says it is.

	A value the battery does not know, and any but _type_ and _present_
	while its bay is empty, fails to read with EIO, as Linux's fail with
	ENODEV.

_/kernel/class/typec/port_*n*_/data_role_, _power_role_, _port_type_, _power_operation_mode_
	For each USB-C port the firmware's UCSI device (_USBC000_, through
	uACPI) runs, from *port0*, as Linux's _/sys/class/typec_ shows them:
	the data role, *[host] device* or *host [device]*; the power role,
	*[source] sink* or *source [sink]*; what the port can be, as *[dual]
	source sink*; and the power agreed on it, *default*, *1.5A*, *3.0A*,
	*5.0A* or *usb_power_delivery*. They are read again when the firmware
	says the port changed. Radix sets none of them, so none can be
	written.

_/kernel/class/power_supply/ucsi-source-psy-USBC000:_*nnn*_/type_, _online_, _voltage_now_, _current_now_, _current_max_
	For each of those ports, from *001*, as Linux's UCSI driver names
	them: *USB*; *1* while the machine takes power through it, else *0*;
	and, while it does, the voltage agreed, in µV, the current agreed and
	the most the charger offers at that voltage, in µA (else *0*).

_/kernel/class/power_supply/ups_*n*_/type_, _scope_, _status_, _online_
	For each UPS plugged in by USB (ADR 0741), from *ups0*, while it is
	plugged in: *UPS*; *System*; what its battery is doing, *Charging*,
	*Discharging*, *Full* or *Not charging*; and *1* while the mains is
	there, else *0*. The USB controller's thread reads the UPS's reports
	every 2 seconds; each file says what it last read. A UPS's directory
	is listed while it is plugged in, and the directory
	_/kernel/class/power_supply_ is there on every machine, as a UPS may
	be plugged in at any time.

_/kernel/class/power_supply/ups_*n*_/capacity_, _capacity_level_, _time_to_empty_now_, _load_
	Its charge in percent; *Critical* when the machine shuts down for
	it, *Low* at or under _capacity_alert_min_, *Full* or *Normal*; how
	long the charge lasts at the load now, in seconds; and what the load
	draws, in percent of what the UPS gives (Dukovo's own).

_/kernel/class/power_supply/ups_*n*_/capacity_alert_min_, _time_to_empty_alert_min_, _shuts_down_
	The charge in percent, and the run time in seconds (Dukovo's own),
	at or under which its battery is low: the limits the UPS sets, else
	10% and 120 seconds. While the UPS runs the machine on its battery
	and it is that low, or the UPS says itself that it is low or that it
	will turn its outlets off, two reads in a row, Spark is asked to
	power the machine off, as for the power button, and the kernel
	powers it off itself 60 seconds later if it is still on.
	_shuts_down_ is *1*, or *0* when the kernel was started with
	*ups=show* (*dukovo-boot*(7)), which only shows the UPS.

_/kernel/class/power_supply/ups_*n*_/health_, _model_name_, _manufacturer_
	*Dead* when the UPS says its battery should be replaced, else
	*Good*; and its USB product and manufacturer strings.

	A value the UPS does not say, and every one until its reports are
	first read, fails to read with EIO; so does every file of a UPS
	unplugged since its directory was opened.

_/kernel/class/thermal/thermal_zone_*n*_/type_, _temp_
	For each thermal zone of the firmware's (*dukovo-boot*(7)'s *ACPI*),
	in the order the namespace has them, as Linux's ACPI thermal driver
	shows it: *acpitz*, and its temperature in thousandths of a degree
	Celsius, read from the zone's _\_TMP_ when the file is opened. It
	fails to read with EIO if _\_TMP_ fails. The sensors program of
	lm-sensors and the desktops' widgets read these.

_/kernel/class/thermal/thermal_zone_*n*_/mode_, _policy_, _available_policies_
	*enabled*, and *step_wise*, the only governor of the passive
	cooling.

_/kernel/class/thermal/thermal_zone_*n*_/trip_point_*k*_type_, _trip_point_*k*_temp_, _trip_point_*k*_hyst_
	Its trip points, in Linux's order: *critical*, *hot* and *passive*,
	each where the zone has one; its temperature, in thousandths of a
	degree Celsius, as the firmware last said it; and *0*.

_/kernel/class/thermal/cooling_device_*n*_/type_, _max_state_, _cur_state_
	Where the CPUs pick their own speed (*dukovo-boot*(7)'s *CPU
	FREQUENCY*), one for each: *Processor*; *3*; and how many steps,
	each a fifth of their highest level, passive cooling slows them by.

_/kernel/class/hwmon/hwmon_*n*_/name_
	The temperature sensors, as Linux's hwmon class shows them, numbered
	in this order: *acpitz*, the thermal zones, if the firmware has any;
	*k10temp*, the sensors of AMD's Zen processors; a *coretemp* for
	each package of Intel's processors (*dukovo-boot*(7)'s *SENSORS*);
	and *cros_ec*, the fans and sensors of Framework's embedded
	controller (*dukovo-boot*(7)'s *EMBEDDED CONTROLLER*). The sensors
	program of lm-sensors reads these.

_/kernel/class/hwmon/hwmon_*n*_/temp_*k*_input_, _temp_*k*_label_
	A sensor's temperature, in thousandths of a degree Celsius, read
	when the file is opened, and its name. For *acpitz*, the _k_-th
	zone's, without a label. For *k10temp*, *Tctl* (_temp1_), the
	temperature the processor's cooling acts on, and on models whose
	dies' registers Linux knows *Tccd1*, *Tccd2*... (from _temp3_), each
	core complex die's. For *coretemp*, *Package id* _p_ (_temp1_) and
	*Core* _c_ (_temp_*c+2*), read on a CPU of that package or core at
	most once a second. For *cros_ec*, each sensor the EC's memory map
	has, _temp1_ for its first place, with the name the EC gives it
	(*power-f75303*, *amd-apu*...). A sensor that gives no reading, as
	Intel's below the temperature they can measure, fails to read with
	EIO.

_/kernel/class/hwmon/hwmon_*n*_/temp_*k*_fault_, _fan_*k*_input_, _fan_*k*_fault_
	For *cros_ec*: *1* while the EC says the sensor failed, is not
	powered or not calibrated, else *0*; each fan's speed, in
	revolutions a minute, *0* when it is stopped or stalled; and *1*
	while an older EC says the fan stalled, else *0*.

_/kernel/class/hwmon/hwmon_*n*_/temp_*k*_crit_, _temp_*k*_max_, _temp_*k*_crit_alarm_
	For *acpitz*, a zone's critical trip point, where it has one. For
	*coretemp*: TjMax, where the processor's thermal control starts (100
	°C where it does not say); where it gives one, the temperature the
	control aims to keep below; and *1* once the critical temperature
	was reached, else *0*.

_/kernel/class/net/_*name*_/ifindex_, _iflink_
	The network interfaces, *lo* and the cards (*net0*, *net1*...),
	then the bridges and VLANs *ip*(8) made and the tunnels *wgd*(8)
	made, each a directory of its
	name, with its number as Linux numbers interfaces: *lo* is 1 and the
	cards follow from 2, in the order the kernel found them (ADR 0190).
	_iflink_ is the same, but for a VLAN, whose _iflink_ is its
	parent's number, as on Linux. The C library's *if_nametoindex* and
	*if_nameindex* read these, and *ip*(8) the rest.

_/kernel/class/net/_*name*_/address_, _broadcast_, _addr_len_
	Its MAC address, as _52:54:00:12:34:56_, and the one that reaches
	every card on its network, all zeros for *lo*; and 6, their length.
	A tunnel has none: both are empty, and _addr_len_ 0.

_/kernel/class/net/_*name*_/mtu_, _type_, _flags_
	The longest packet it sends; *1* (ARPHRD_ETHER) for a card, *772*
	(ARPHRD_LOOPBACK) for *lo* or *65534* (ARPHRD_NONE) for a tunnel, as
	Linux numbers a tun; and Linux's flags, in hexadecimal: IFF_UP, with
	IFF_BROADCAST and IFF_MULTICAST for a card, IFF_LOOPBACK for *lo*, or
	IFF_POINTOPOINT and IFF_NOARP for a tunnel (ADR 0339).

_/kernel/class/net/_*name*_/operstate_, _carrier_
	*up*, *down* for a card whose driver says its link is down, as with
	no cable or the other end off, a card the kernel stopped after an
	error, or a tunnel no program has open, or *unknown* for *lo* and an
	open tunnel, which have no link; and *1* while it is not down, else
	*0*.

_/kernel/class/net/_*name*_/ipv4_
	Radix's own: its IPv4 addresses with their prefixes, a line each,
	as _10.0.2.15/24_, the first the one it sends from; nothing for a
	card not set up. *lo* shows 127.0.0.1/8, though it answers each
	card's addresses too. Linux gives these only over netlink; the C
	library's *getifaddrs* and Go's net package read them here.

_/kernel/class/net/_*name*_/ipv6_
	Radix's own: what its IPv6 has (ADR 0331), a line each: each
	address with its prefix, what it is (*link-local*, *stable*,
	*temporary* or *given*) and how many seconds it is preferred and
	valid for, *tentative* at the end while it is tested
	(_address 2001:db8::5/64 temporary preferred 86000 valid 172000_);
	the routers that advertised themselves (_router fe80::1 lifetime
	1800_) and the one it was given (_router fe80::1 given_); the DNS
	servers they advertised (_dns 2001:db8::53 lifetime 600_); the MTU
	(_mtu 1500_); and whether DHCPv6 is to give addresses (_dhcp
	managed_) or other settings (_dhcp other_). *ip*(8), *netd*(8), the
	C library's *getifaddrs* and Go's net package read it.

_/kernel/class/net/_*name*_/link_
	Radix's own: what the interface is (ADR 0333), on its first line:
	*card*, *loopback*, *bridge*, *vlan 7 on net0* or *tunnel*. For a
	bridge, a
	line for each port (_port net0_) and one for each machine it heard
	in the last five minutes, the port and how many seconds ago
	(_heard 52:54:00:12:34:56 on net0 12_); for a bridge's port, its
	bridge (_master br0_); and how many frames were lost to its full
	queues, if any (_lost 3_).

_/kernel/class/net/_*name*_/statistics/rx_bytes_, _rx_packets_, _tx_bytes_, _tx_packets_, _tx_dropped_
	What it received and sent since boot, and what it could not send
	for want of room; _rx_errors_, _rx_dropped_ and _tx_errors_ are 0,
	as the stack counts none.

_/kernel/class/net/_*name*_/speed_, _duplex_
	What the card negotiated, as Linux's: its speed in megabits a
	second, or *-1* while its link is down or the card does not say, as
	a virtio card does not, and *full*, *half* or *unknown*; *-1* and
	*unknown* for a bridge, a VLAN, a tunnel and *lo*, which have no wire
	(ADR 0341). *nic*(8) reads them.

_/kernel/class/net/_*name*_/card_
	Radix's own: what the kernel knows of a card it drives, a line each,
	as ethtool's _-i_ and _-g_ show Linux's (ADR 0341): its driver
	(_driver radix-intel-net_), its model (_model Intel 82574L_), its PCI
	address (_bus 0000:00:03.0_), how many frames it holds each way
	(_receive_slots 256_, _transmit_slots 256_), how it interrupts
	(_interrupt MSI vector 0x31_), and how many received frames its
	driver dropped, damaged, too short or with no slot for them
	(_dropped 0_), when the driver counts them. For a card a program
	drives, only _driver program_, and for a USB adapter _driver
	radix-usb_; nothing for the others.

_/kernel/class/display/state_
	Radix's own: what the display layer has, written when it is read.
	The first line says who has the screens: _front log_ while the
	kernel's log does, _front process_ and the number of the process
	that made the last change of the program in front, _front holder_
	for a program that has opened the screens and made no change yet,
	and _front last_ while the last picture of a program that ended
	stays. Then a line for each graphics card, each of its screens with
	the modes it offers, the one it prefers first, and each of its
	pipes, which make the frames: what the line is of, that thing's
	number in the layer, and _key=value_ words. A value that may hold
	spaces, a card's model and a monitor's name, is the last of its
	line and runs to the line's end.

	```
	front process 41
	card 1 name=virtio0 kind=virtio hot_plug=1 mirrors=0 sets_modes=1 shared_pages=0 wakes=977 model=Virtio graphics card
	screen 6 card=1 index=0 plugged=1 failed=0 hdr=0 variable_refresh=0 pipe=2 depth=8 width_mm=527 height_mm=296 refresh_min_mhz=0 refresh_max_mhz=0 luminance_max=0 product=4660 maker=EXM name=Example 24
	mode 6 width=1920 height=1080 refresh_mhz=60000 preferred=1
	pipe 2 card=1 index=0 active=1 width=1920 height=1080 refresh_mhz=60000 beat_ns=16666667 beat_count=58211 mode_sets=1
	```

	A card's _kind_ is *firmware*, *virtio* or *other*, and _wakes_
	counts how often its thread has woken, which a screen that stands
	still does not add to. A screen's _pipe_ is the pipe that feeds it,
	or 0, and the mode it shows is that pipe's; a number the monitor
	does not say is 0. A pipe's _beat_ns_ is the nanoseconds from one
	of its frames to the next, or 0 while it makes none, _beat_count_
	the number of its latest, and _mode_sets_ how often its card was
	asked to give it another mode, to start it or to stop it: screens
	handed from one program to another in the mode they show add
	nothing to it. A monitor's serial number is not here, so that
	everyone may read the file and its words can go in a report as
	they are; the program that has the screens reads it from the layer
	itself.
	*hardware*(8)'s *screens* reads the file. With no graphics card it
	is its first line alone.

_/kernel/devices/system/cpu/possible_, _present_, _online_
	The CPUs the machine has, and those that run threads, as Linux's
	lists: *0-7*, which util-linux's lscpu reads.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/scaling_driver_
	Where CPU _n_ picks its own speed (*dukovo-boot*(7)'s *CPU
	FREQUENCY*): the Linux driver for its control, *intel_pstate* for
	Intel's HWP or *amd-pstate-epp* for AMD's CPPC. Each CPU has a
	policy, as with those drivers, and _/kernel/devices/system/cpu/cpu_*n*_/cpufreq_
	is a link to it. Linux's cpupower and powertop read these.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/affected_cpus_, _related_cpus_
	The CPU, *n*.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/cpuinfo_min_freq_, _cpuinfo_max_freq_
	Its lowest and highest frequency, in kHz.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/scaling_min_freq_, _scaling_max_freq_
	Those it is asked to stay between, in kHz: by default, all of them.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/scaling_cur_freq_
	The frequency it runs at, in kHz, measured over 10 ms when the file
	is read (or in the last 100 ms), from its APERF and MPERF counters;
	the base frequency if it slept throughout.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/scaling_governor_, _scaling_available_governors_
	*powersave*: the CPU picks its speed within the range itself, which
	Linux's drivers call so; *performance* while the CPUs are held at
	the top of the range (*cpu.speed* *highest*, ADR 0557).
	_scaling_available_governors_ lists *performance powersave*, as
	Linux's intel_pstate does.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/energy_performance_preference_
	Where the CPU takes one, the energy preference it is asked for:
	*performance*, *balance_performance*, *balance_power* or *power*, or
	the number from 0 to 255 for another (*dukovo-boot*(7)'s *epp*).
	_energy_performance_available_preferences_ lists the names, and
	*default*, as Linux's.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/base_frequency_
	Intel's: the frequency it can keep up, in kHz.

_/kernel/devices/system/cpu/cpufreq/policy_*n*_/amd_pstate_highest_perf_, _amd_pstate_max_freq_, _amd_pstate_lowest_nonlinear_freq_
	AMD's: its highest level, its highest frequency, and its most
	efficient frequency (kHz).

	The files are read-only: the range and the preference are set at
	boot.

_/kernel/devices/system/edac/mc/mc0/ce_count_, _ue_count_
	While the CPUs' machine-check banks are read (*dukovo-boot*(7)'s
	*MACHINE CHECKS*): how many memory errors the memory controllers
	corrected, and how many they did not, since boot, as Linux's EDAC
	counts them. Radix shows them as one memory controller, whose DIMMs
	are the banks' channels.

_/kernel/devices/system/edac/mc/mc0/ce_noinfo_count_, _ue_noinfo_count_
	Those whose bank said no channel.

_/kernel/devices/system/edac/mc/mc0/mc_name_, _seconds_since_reset_
	*Machine check banks*, and the seconds since the counts began: since
	boot.

_/kernel/devices/system/edac/mc/mc0/dimm_*n*_/dimm_label_, _dimm_location_
	A bank and channel that had an error, such as _bank 7 channel 1_,
	numbered from 0 in the order they first did. Linux's
	*ras-mc-ctl --error-count* and *edac-util* read these.

_/kernel/devices/system/edac/mc/mc0/dimm_*n*_/dimm_ce_count_, _dimm_ue_count_
	Its corrected and uncorrected errors.

_/kernel/settings/_*name*
	The value of one of the kernel's settings (*radix-kernel-settings*(2),
	ADR 0476), one file for each, named as the setting is: *cpu.energy_preference*
	(*firmware*, *performance*, *balance_performance*, *balance_power* or
	*power*), *cpu.boost* (*on* or *off*: whether the CPUs may run above
	their base frequency), *cpu.speed* (*chosen* or *highest*: whether
	each CPU picks its speed or is held at the top of its range),
	*sched.slice_us* (the fair scheduling class's base slice in
	microseconds), *disk.read_ahead_kib* (how far ahead the page cache
	reads, in KiB) and *net.congestion* (*cubic* or *reno*, for new TCP
	sockets). Everyone reads them. Radix's own, and, as everything of
	_/kernel_, read-only here: a setting is changed with the kernel
	settings capability, which *tune*(8) uses.

# EXAMPLES

In the *storage* boot test (*ob*(1)), the 32 MiB disk _vdb_ has
_/kernel/block/vdb/size_ of *65536*, and the boot test's program finds a
virtio function for each of its four disks in _/kernel/bus/pci/devices_.

# SEE ALSO

*radix-proc*(5), *radix-block*(2), *radix-kernel-settings*(2), *radix-files*(7),
*dukovo-boot*(7), *ob*(1), *tune*(8), *spark*(8), *watchdog*(8), *lspci*(8), *lsmem*(1), *ip*(8)
