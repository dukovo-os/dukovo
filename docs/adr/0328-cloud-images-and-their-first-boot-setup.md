<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0328. Cloud images and their first-boot setup

- Status: Accepted
- Date: 2026-11-10

## Context

A cloud machine starts from an image its owner uploads or picks, copied
to a disk larger than the image, and has no one at its console: the
person who made it signs in over SSH with a key they gave the cloud.
Linux distributions ship such images with cloud-init, which at the first
boot reads what the cloud says of the machine and sets it up: its name,
its network cards, an account with the owner's keys, and whatever the
owner's *user data* asks. The user data is a file the owner writes once
and gives every cloud, so its format, cloud-init's `#cloud-config`, is
what people already have.

Where the data comes from differs by cloud. Proxmox, libvirt, QEMU's
users and many smaller clouds attach a *config disk*, labelled `cidata`,
with NoCloud's files: `meta-data` (the instance's ID, the machine's name,
the owner's keys), `user-data` and `network-config` (cloud-init's version
1 or netplan's version 2). AWS and DigitalOcean answer at
169.254.169.254 instead: AWS's instance metadata service, whose version
2 gives a token to a PUT and wants it on every request, and
DigitalOcean's `/metadata/v1.json`. Both set the network up by DHCP for
an image of one's own. DigitalOcean starts such images by BIOS only.

The files are YAML. cloud-init's own files use a small part of it:
mappings, lists, quoted and block scalars, flow lists; never anchors or
several documents.

Each cloud takes its own disk format: raw for AWS's snapshot import,
qcow2 for KVM, Proxmox, OpenStack and DigitalOcean, VMDK for VMware
(VirtualBox reads it too), VHDX for Hyper-V and Azure.

## Decision

- **One cloud image, in four formats.** `ob image --cloud` makes the disk
  of an installed system, as `--installed` does (ADR 0307), whose
  `@settings` turns two services on with override files: cloud-setup and
  sshd. It is a hybrid image, starting under UEFI and BIOS. The host's
  `qemu-img` writes it as qcow2 (compressed), VMDK (monolithic sparse)
  and VHDX (dynamic) beside the raw `.img`; qemu-img is GPL-2.0, so it
  runs on the build machine alone and is never shipped. The same image
  serves every cloud: what differs is found at boot.
- **cloud-setup(8), a service off but in cloud images.** It runs at each
  boot and ends. It finds the data: a mounted volume labelled `cidata`,
  in any case, FAT or ISO 9660, waiting up to 30 seconds for Spark or
  volumed(8) to mount one the kernel found; else, if the firmware's
  tables name DigitalOcean or Amazon EC2 (or an `amazon` firmware, as
  AWS's older Xen machines have), that cloud's metadata service, which
  it waits up to 90 seconds to reach while DHCP sets the card up. A
  machine with neither is left as it is, without waiting. The service is
  off on every other system, as a stick labelled `cidata` would
  otherwise set up any computer it is put in at boot.
- **Once for each instance.** The instance's ID is kept in
  `/system/state/cloud-setup/instance`. With a new ID, as a machine made
  from another's disk has, cloud-setup names the machine (`hostname` or
  `fqdn` of the user data, else the cloud's name, short), writes the
  network(5) file from `network-config` and has netd read it again,
  makes the accounts through authd(8), writes `write_files`, and runs
  `runcmd`, or user data that is a script. At every boot it grows the
  root's partition to its disk's end and the root's Btrfs over it, with
  `rdisk --grow` and `device resize max` (ADR 0327), unless `growpart`'s
  mode is off or `resize_rootfs` is false.
- **The accounts.** `users` lists `default` and accounts in full; without
  `users`, only the default account. The default account is `dukovo`, an
  administrator, with the cloud's keys and the user data's
  `ssh_authorized_keys`, as Ubuntu's images make `ubuntu`; the user data's
  `user` names it otherwise, as Proxmox's cloud-init drive does. An account's
  `groups` naming `sudo`, `wheel`, `admin` or `admins`, or a `sudo` rule,
  makes it an administrator. Its password is `plain_text_passwd` when
  `lock_passwd` is false, the default account's the user data's
  `password`; otherwise it gets 32 random bytes no one knows, so it signs
  in with its keys alone, as cloud-init's locked password does. A
  `passwd` hash, crypt(3)'s, cannot become an Argon2id one, and is left
  out, saying so, as is a `password` that is one, as Proxmox gives it.
  Keys go to the end of `~/.ssh/authorized_keys`, those
  not there, the file and folder the account's own and closed to others.
- **Elevating without a password.** An account with a key and no
  password could not elevate. A new group, `elevate-without-password`
  (number 11), lets its administrators elevate without typing one, as a
  `NOPASSWD` sudo rule does on Linux; authd asks no password of them and
  audits "no password needed". The default account and an account whose
  `sudo` rule has `NOPASSWD` are put in it. `user set
  --elevate-without-password` and `--elevate-with-password` change it
  through authd's `set` request, which gained the setting.
- **A YAML reader of its own.** cloud-setup reads YAML with a small reader
  in systools of the part cloud-init's files use, which says by line what
  it does not read (anchors, aliases, complex keys, a second document)
  rather than reading it wrongly. The YAML crates for Rust bring a hash
  crate under a license the project's list does not have, and much more
  than these files use.
- **Saying what is left out.** Each key of the user data cloud-setup does
  not apply is named in the log with why: `packages` (Dukovo installs
  apps with bundle(1)), `ssh_pwauth` (sshd_config(5) turns passwords
  on), and any other. A file that is not `#cloud-config` or a script
  (gzip, MIME multipart, `#include`, a boot hook, Jinja) is left out
  whole, saying what it is.

## Consequences

- `cloud-config` boots a cloud image whose disk has 256 MiB more than its
  root, with a config disk: the root grows, the machine is named, the
  card takes an address by its MAC, three accounts are made, files are
  written with their owners, modes and base64, commands run, and the
  host's ssh signs in as the default account and as an administrator
  with their keys, each elevating without a password. Run again,
  cloud-setup finds the instance set up.
- `cloud-metadata` boots one whose firmware names Amazon EC2, on a
  network where the test's server answers as AWS's service, version 2,
  does, tokens and all.
- The default services number 23, of which 19 are on, as before.
- Azure and Hyper-V take the VHDX, but Dukovo starts there only once it
  drives VMBus (Phase 14d.2). Google Cloud's metadata service, Azure's,
  OpenStack's config drive (`config-2`) and NoCloud from the kernel's
  command line or the SMBIOS serial are left for later; so are
  `bootcmd`, `timezone`, `ssh_pwauth`, `chpasswd`'s lists and the
  network's bonds, bridges and VLANs, which the log names when given.
- A real cloud's checks are the lab's: an image uploaded to AWS and to
  DigitalOcean, a Proxmox machine with its cloud-init drive.
