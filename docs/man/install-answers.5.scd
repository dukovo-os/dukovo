install-answers(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

install-answers - the answers to the installer's questions, in a file

# DESCRIPTION

An answers file answers every question the installer asks, in JSON, so
that *dukovo-install --answers* installs by itself (*dukovo-install*(8)).
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
	start a system already; or *manual*, the partitions *changes*
	lists, as the installer's rdisk screen chooses them. *mirror*,
	two disks as one Btrfs RAID1, and *beside*, Dukovo next to
	Windows, come later.

	*target*: the disk's name, as *lsblk*(8) lists it, with or
	without _/dev/_ in front: *vda*, */dev/nvme0n1*. For *manual*,
	the disk that holds the root.

	*mirror*: the second disk, for the mirror layout only.

	*swap_gib*: the swap partition's size in GiB, up to 64; *0*, or
	left out, for none. Not for *manual*, whose swap partition, if
	any, is in *changes*.

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

Server installs will add the keys *profile*, *features*, *web_console*,
*ssh*, *updates* and *firmware*, which this installer does not take yet.

# SEE ALSO

*dukovo-install*(8), *rdisk-layout*(5), *hostname*(5), *network*(5)
