cloud-config(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

cloud-config - a cloud machine's user data, as cloud-setup reads it

# SYNOPSIS

_user-data_, _meta-data_ and _network-config_ on a disk labelled
*cidata*, or the user data a cloud is given

# DESCRIPTION

*cloud-setup*(8) sets a cloud machine up from the user data its owner
gives the cloud, in cloud-init's *#cloud-config* format, of which it
reads the keys below (ADR 0328), and from the cloud's meta-data. A file
written for cloud-init works where it uses these keys; each key it does
not apply is named in the log, with why.

The user data is one of:

- *#cloud-config* on its first line, then a YAML mapping of the keys
  below;
- a script: *#!* and its interpreter on its first line, run once, as root,
  after the rest;
- nothing.

User data of another kind cloud-init knows, compressed with gzip, a MIME
multipart, an *#include*, a boot hook or a Jinja template, is left out
whole, saying so.

# YAML

The files are read with a reader of the part of YAML cloud-init's files
use: mappings and lists by their indentation, a list as indented as its
key or more, plain and quoted scalars, literal (*|*) and folded (*>*)
blocks with *-* and *+*, flow lists and mappings (*[a, b]*, *{k: v}*),
comments and a first *---*. Anchors and aliases (*&*, *\**), complex keys
(*?*) and a second document are refused, with the line, not read
wrongly. Indent with spaces: a tab is refused.

# KEYS

*hostname*, *fqdn*
	The machine's name; only its first part, before a dot, is used, as
	*hostname*(5) holds a short name. Without either, the meta-data's
	name is.

*preserve_hostname*
	*true*: leave the machine's name as it is.

*users*
	The accounts to make: a list of *default*, names, and mappings.
	Without *users*, the default account alone is made. An account that
	is there already gets its keys. A mapping's keys:

	*name*
		The account's name, which it needs.

	*gecos*
		Its full name.

	*groups*
		A list, or names with commas between. *sudo*, *wheel*,
		*admin* or *admins* makes it an administrator.

	*sudo*
		A sudo rule, or a list of them: any rule makes it an
		administrator, and one with *NOPASSWD* has it elevate without
		a password (*user*(8)). *false* is none.

	*ssh_authorized_keys*
		Its SSH keys, for _~/.ssh/authorized_keys_.

	*lock_passwd*
		*false* to use *plain_text_passwd*; *true*, the default,
		gives the account a password no one knows, so that it signs
		in with its keys alone.

	*plain_text_passwd*
		Its password, when *lock_passwd* is *false*.

	*passwd* and *hashed_passwd*, a crypt(3) hash, are left out, as
	are *shell*, *uid*, *primary_group* and the other keys of
	cloud-init's.

*user*
	The default account's name, instead of *dukovo*: a name, or a
	mapping of which only *name* is read. Proxmox's cloud-init drive
	gives it.

*ssh_authorized_keys*
	More SSH keys of the default account, after the cloud's own.

*password*
	The default account's password. A crypt(3) hash, as Proxmox gives,
	is left out: set the password with *passwd*(1).

*write_files*
	Files to write, after the accounts are made, each a mapping:

	*path*
		The file's absolute path; the folders on the way are made.

	*content*
		What it holds.

	*encoding*
		*b64* or *base64* if *content* is base64; plain text by
		default. gzip is refused.

	*permissions*
		Its mode, in octal, as *'0640'*; *0644* by default.

	*owner*
		*user* or *user:group*; root's by default.

	*append*
		*true* to add *content* to the file's end.

*runcmd*
	Commands to run, as root, in order: a line, which the shell runs,
	or a list of a program and its arguments. Their output goes to the
	log. Quote a line with a colon and a space in it, as
	*- 'echo "ready: yes"'*: YAML reads it as a mapping otherwise, as
	cloud-init does, and the user data is refused.

*growpart*
	*mode: off* or *mode: false* keeps the root's partition as it is.

*resize_rootfs*
	*false* keeps the root's Btrfs as it is.

*ssh_pwauth*
	Left out: *sshd_config*(5) turns passwords on.

*packages*, *package_update*, *package_upgrade*
	Left out: Dukovo installs apps with *bundle*(1), which *runcmd* may
	run.

# META-DATA

NoCloud's _meta-data_ is YAML (or JSON): *instance-id*, which it needs,
*local-hostname* and *public-keys*, a list of keys or a mapping of names
to them. DigitalOcean's and AWS's come from their metadata services
(*cloud-setup*(8)).

# NETWORK-CONFIG

NoCloud's _network-config_ is cloud-init's version 1 or netplan's
version 2, perhaps under a *network* key. A card given an address of its
own gets a section of _/system/settings/network_ (*network*(5)): by its
MAC address when the file gives one (version 1's *mac_address*, version
2's *match: macaddress*), else by its name, Linux's *ethN* being Dukovo's
*netN*. Its addresses, with their prefix or *netmask*, its router
(*gateway*, *gateway4*, *gateway6*, or a route to *default*), its DNS
servers, and *dhcp = yes* if it asks DHCP too, are written. A card the
file sets up by DHCP alone needs no section. Bonds, bridges, VLANs and
routes to other networks are left out, saying so.

# EXAMPLES

A user data file:

```
#cloud-config
hostname: web01
users:
  - default
  - name: ann
    gecos: Ann Admin
    groups: users, sudo
    ssh_authorized_keys:
      - ssh-ed25519 AAAAC3Nza... ann@laptop
write_files:
  - path: /system/settings/motd
    content: |
      Welcome to web01.
runcmd:
  - service sshd restart
```

A config disk for a QEMU machine, made on Linux:

```
$ printf 'instance-id: web01-1\\nlocal-hostname: web01\\n' > meta-data
$ cp ~/.ssh/id_ed25519.pub keys
$ mkfs.fat -C -n CIDATA cidata.img 2048
$ mcopy -i cidata.img meta-data user-data ::
$ qemu-system-x86_64 ... -drive file=cidata.img,format=raw,if=virtio
```

# SEE ALSO

*cloud-setup*(8), *user*(8), *network*(5), *hostname*(5), *sshd*(8),
*bundle*(1)
