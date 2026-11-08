install-answers(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

install-answers - the answers to the installer's questions, in a file

# DESCRIPTION

An answers file answers every question the installer asks, in JSON, so
that *dukovo-install --answers* installs by itself (*dukovo-install*(8)),
from a file or from an _https://_ address, checked against the SHA-256
*--sha256* gives (ADR 0319). Saved as _install.json_ at the top of the
install stick's EFI system partition, it is a setup file, which the
installer's screens offer; they also download one from the web.
Every install saves its answers in the installed system as
_/system/settings/install/answers.json_, so a system can be installed
again the same way, or copied to the next machine with a new host name.

```
{
  "version": 1,
  "keyboard": "us",
  "disk": { "layout": "guided", "target": "/dev/nvme0n1", "swap_gib": 8 },
  "hostname": "web01",
  "time_zone": "America/Denver",
  "profile": "server",
  "features": { "add": ["file-sharing"] },
  "network": {
    "net0": { "address": "10.0.0.20/24", "gateway": "10.0.0.1", "dns": ["10.0.0.1"] }
  },
  "admin": {
    "name": "Alice Liddell",
    "user": "alice",
    "password_hash": "$argon2id$v=19$m=65536,t=3,p=4$..."
  },
  "users": [{ "name": "Bob", "user": "bob", "password_hash": "" }]
}
```

The installer checks the whole file before it touches a disk, and names
each problem by its key, such as _users[0].user_ or _network.net0_. A key
it does not know is a problem, so that a mistyped one is not missed.

# KEYS

*version*
	*1*, the version of the file this installer reads.

*keyboard*
	The keyboard layout, as *keymap*(1) names it; *us* if not given.

*language*
	The language, as a BCP 47 tag; only *en-US* for now, and *en-US*
	if not given.

*disk*
	The disk and how it is laid out:

	*layout*: *guided*, which erases the disk and gives it a BIOS
	boot partition, an EFI system partition, swap and the Dukovo
	root; *free*, which keeps the disk's partitions and makes the
	same four in its largest free space, on a GPT disk that does not
	start a system already; *beside*, next to Windows, which shrinks
	Windows' volume, C:, by *space_gib* and makes in the space it
	gives an EFI system partition of Dukovo's own, swap and the root,
	keeping Windows' partitions, on a GPT disk whose Windows starts by
	UEFI, with the install stick started by UEFI too; *mirror*, which
	erases *target* and the disk *mirror* names, gives each the guided
	layout's partitions and GRUB, and makes their two roots one Btrfs
	RAID1, as big as the smaller disk allows, so that either disk
	starts the system alone; or *manual*, the partitions *changes*
	lists, as the installer's rdisk screen chooses them.

	*target*: the disk's name, as *lsblk*(8) lists it, with or
	without _/dev/_ in front: *vda*, */dev/nvme0n1*. For *manual*,
	the disk that holds the root.

	*mirror*: for *mirror* only, and needed there: the second disk's
	name, as *target*'s, another disk than *target*.

	*swap_gib*: the swap partition's size in GiB, up to 64; *0*, or
	left out, for none. *mirror* makes one on each disk. Not for
	*manual*, whose swap partition, if any, is in *changes*.

	*space_gib*: for *beside* only, and needed there: the space
	Dukovo takes from Windows, in GiB. It is at least 20, and leaves
	Windows 10 GiB free beyond what its files use; the root gets at
	least 8 GiB beyond the swap. Windows that BitLocker encrypts, that
	hibernated or shut down with Fast Startup, or whose volume is
	marked to be checked is refused, with what to do in Windows.

	*changes*: for *manual* only, what becomes of each disk changed,
	in a list (see *CHANGES* below).

*hostname*
	The machine's name (*hostname*(5)): letters, digits and *-*, not
	at its start or end, 63 at most; *dukovo* if not given.

*time_zone*
	The time zone, a name in the tz database such as
	*America/Denver*, as *timedatectl list-timezones* lists them;
	*UTC* if not given.

*network*
	How each network card is set up, by its name (*network*(5)): the
	word *"dhcp"*, or a fixed address with its prefix, *address*, and
	the router, *gateway*, and up to three DNS servers, *dns*. A card
	not named asks DHCP.

*admin*
	The administrator's account: the person's full name, *name*, the
	account's name, *user*, and the password's Argon2id hash,
	*password_hash*, which *dukovo-install --hash-password* makes, or
	*""* for no password. An answers file never holds a password
	itself.

*users*
	Other people's accounts, ordinary users', each as *admin* is
	written.

*keep_password_hashes*
	*true* keeps the passwords' hashes in the saved answers, which are
	then readable by root alone, so that installing again sets the
	same passwords; left out, the saved answers have none, and anyone
	may read them.

*profile*
	What the machine is for: *minimal*, the base system with nothing
	listening; *server*, Minimal and the web console; or *cloud*,
	Server set up at first boot from the cloud's data, with the web
	console off. *minimal* if not given. A profile is a starting set
	of features; each of this release's starts with none, and offers
	them all.

*features*
	The features added to the profile's, by their IDs in *add*, and
	those of the profile left out, in *remove* (*feature*(8),
	*feature.toml*(5)): ++
_"features": { "add": ["file-sharing"] }_ ++
The install turns on each service a feature lists, in the new
system's settings, so that they start at its first boot. A feature
that installs bundles is added after the first boot, with *feature
add*, as the new system reaches a catalog only once it runs; the
installer refuses it, as it refuses an ID the release lacks.

*web_console*
	*true* turns Dukovo Admin, the web console, on; *false* leaves it
	off; left out, the profile says. Dukovo Admin arrives in release
	0.16, which turns it on if the saved answers say so.

*serial_console*
	*true* keeps a console on the first serial port on the installed
	system: its boot menu starts the kernel with *console=ttyS0*, so that
	*serial-console*(8) runs the sign-in there as well as on the screen's
	consoles, and the startup-trouble menu shows there too; *false* keeps
	none. Left out, it is as the installer runs: true when it runs on
	the serial port, as the stick's *Install Dukovo (serial console)*
	entry starts it, with that entry's speed (ADR 0318). The screens
	save it as they ran.

# CHANGES

Each entry of *changes* changes one disk:

```
"changes": [{
  "disk": "vda",
  "delete": [2],
  "partitions": [
    { "number": 2, "first": 526336, "sectors": 1048576,
      "name": "EFI system", "use": "efi" },
    { "number": 3, "use": "data", "format": "exfat" },
    { "number": 4, "first": 2099200, "sectors": 4194304, "use": "root" }
  ]
}]
```

*disk*
	The disk's name, as *target* is written.

*new_table*
	*true* gives the disk a new, empty GPT first: every partition on
	it is gone. Not with *delete*.

*delete*
	The partitions taken out, by number.

*partitions*
	The partitions made, and those formatted, each by its *number*.
	A partition made has its *first* sector and its size in
	*sectors*, both in the disk's own sectors, and a *name* in the
	GPT if wanted; one formatted has neither, and is one the disk
	has. Its *use* is what it holds:

	*root*: the Dukovo root, Btrfs.++
*efi*: the EFI system partition, FAT32.++
*swap*: swap, which Spark turns on at every start (*spark*(8)).++
*bios-boot*: GRUB for a BIOS, which then starts the disk too.++
*data*: data, with *format* *btrfs*, *exfat* or *fat32*, or no
*format* to leave a new one empty. Spark mounts it at
_/volumes/<partition>_.

	A partition of another use than *data* gets that use's type; a
	data partition keeps the type it has.

The changes must leave one root and one EFI system partition, on the
same GPT disk, the target, which also holds the swap partition, if any.
A target with a BIOS boot partition needs its EFI system partition
numbered 2, where the BIOS's GRUB finds its files; without one, it
starts by UEFI alone.
Nothing is written to any disk before the whole file is checked.

Later installers will add the keys *ssh*, *updates* and *firmware*,
which this installer does not take yet.

# SEE ALSO

*dukovo-install*(8), *feature*(8), *feature.toml*(5), *rdisk-layout*(5), *hostname*(5), *network*(5), *serial-console*(8)
