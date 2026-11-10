<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Cloud and virtual machines

This chapter describes running Dukovo as a cloud machine: on AWS,
DigitalOcean, Proxmox, OpenStack, VMware, VirtualBox or plain QEMU. One
cloud image serves them all. At its first boot it sets itself up from
what the cloud says of the machine, as Linux images do with cloud-init:
its name, its network, an account with the owner's SSH keys, and
whatever the owner's user data asks. The full references are
cloud-setup(8) and cloud-config(5)
([ADR 0328](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0328-cloud-images-and-their-first-boot-setup.md)).

## Making the image

```
$ ./ob image --cloud
```

This writes the disk of an installed system, with cloud-setup and sshd
turned on, in four formats:

| File | For |
| --- | --- |
| `out/dukovo-0.13.0-x86_64-cloud.img` | raw: AWS, and any cloud that takes a raw disk |
| `out/dukovo-0.13.0-x86_64-cloud.qcow2` | KVM, Proxmox, OpenStack and DigitalOcean |
| `out/dukovo-0.13.0-x86_64-cloud.vmdk` | VMware and VirtualBox |
| `out/dukovo-0.13.0-x86_64-cloud.vhdx` | Hyper-V and Azure, once Dukovo drives Hyper-V's devices |

The three last are written by the build machine's `qemu-img` (the
`qemu-utils` package on Debian and Ubuntu, `qemu-img` on Fedora), which
the image does not carry. The image starts under UEFI and under BIOS,
as clouds use either.

Give the machine a disk as large as you like: at each boot, the root's
partition and its Btrfs grow to the disk's end (see
[Files](files.md) for the root's layout).

## The first boot

At its first boot, cloud-setup looks for the machine's data, in this
order:

1. **A config disk**, a disk or disc labelled `cidata` with NoCloud's
   files, `meta-data`, `user-data` and `network-config`. Proxmox's
   cloud-init drive is one, as is the disk cloud-localds or libvirt's
   `virt-install --cloud-init` makes.
2. **DigitalOcean's metadata service**, when the firmware names the
   machine's maker DigitalOcean.
3. **AWS's instance metadata service**, version 2, when it names Amazon
   EC2.

It names the machine, sets the network cards up as the network
configuration says (by DHCP when it says nothing), and makes the
account `dukovo`, an administrator, with the cloud's SSH keys. Then it
makes the accounts the user data lists, writes its files and runs its
commands. All this is done once for each instance: a machine made from
a copy of another's disk is set up as a new one, and a machine that
restarts is not set up again.

Sign in as `dukovo` with the key you gave the cloud:

```
$ ssh dukovo@203.0.113.10
dukovo@web01:~$ sudo service list
```

`dukovo` has a password no one knows, so it elevates without one: it
is in the `elevate-without-password` group, as Ubuntu's `ubuntu` has a
`NOPASSWD` sudo rule. Anyone who signs in as it can do anything as
root, so keep its keys safe. To have it type a password, give it one
and take it out of the group:

```
$ sudo passwd dukovo
$ sudo user set --elevate-with-password dukovo
```

## User data

User data is a file you give the cloud when you make the machine, in
cloud-init's `#cloud-config` format, or a script. cloud-setup reads the
keys most files use; cloud-config(5) lists them. For example:

```yaml
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

`default` keeps the `dukovo` account beside ann's. Naming `sudo`,
`wheel` or `admin` among an account's groups, or giving it a `sudo`
rule, makes it an administrator; a rule with `NOPASSWD` lets it elevate
without a password. Accounts get a password no one knows unless the
user data gives `lock_passwd: false` and `plain_text_passwd`: a
`passwd` hash is crypt(3)'s, which Dukovo's Argon2id passwords cannot
take.

A key cloud-setup does not apply, such as `packages`, is named in the
log with why; Dukovo installs apps with `bundle`, which `runcmd` may run.

## On each cloud

### Proxmox, libvirt and QEMU

Import the `.qcow2` as the machine's disk and add a cloud-init drive.
Proxmox's has its own page of the machine's settings, with the account,
the SSH keys and the network: the account it names is made instead of
`dukovo`, an administrator with those keys. Proxmox gives its password
as a crypt(3) hash, which Dukovo cannot take, so sign in with the key
and set one with `passwd` if you want one. With QEMU alone, make a
config disk:

```
$ cat > meta-data <<EOF
instance-id: web01-1
local-hostname: web01
EOF
$ mkfs.fat -C -n CIDATA cidata.img 2048
$ mcopy -i cidata.img meta-data user-data ::
$ qemu-system-x86_64 -machine q35 -m 2G -enable-kvm \
    -drive file=dukovo-0.13.0-x86_64-cloud.qcow2,if=virtio \
    -drive file=cidata.img,format=raw,if=virtio \
    -nic user,model=virtio-net-pci,hostfwd=tcp::2222-:22
```

A new `instance-id` sets the machine up again with the new files.

**The QEMU guest agent.** Every Dukovo system runs guest-agent(8),
which answers the host as Linux's qemu-ga does, on the virtio port
`org.qemu.guest_agent.0`
([ADR 0329](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0329-the-qemu-guest-agent-and-virtualbox-in-ci.md)).
In Proxmox, turn on *QEMU Guest Agent* in the machine's *Options*: its
summary then shows the machine's addresses, and *Shutdown* stops its
services and powers it off cleanly. libvirt's `virsh domifaddr --source
agent` and `virsh shutdown --mode agent` use it too. It does not run
commands or copy files for the host, and does not freeze filesystems
for Proxmox's backups, which then back the machine up as they would one
without an agent.

### AWS

AWS imports the raw `.img` as a snapshot, from an S3 bucket, then
registers it as an AMI that boots by UEFI:

```
$ aws s3 cp out/dukovo-0.13.0-x86_64-cloud.img s3://my-bucket/
$ aws ec2 import-snapshot --disk-container \
    'Format=RAW,UserBucket={S3Bucket=my-bucket,S3Key=dukovo-0.13.0-x86_64-cloud.img}'
$ aws ec2 register-image --name dukovo-0.13.0 --architecture x86_64 \
    --boot-mode uefi --ena-support --virtualization-type hvm \
    --root-device-name /dev/xvda \
    --block-device-mappings 'DeviceName=/dev/xvda,Ebs={SnapshotId=snap-...}'
```

`import-snapshot` needs AWS's `vmimport` role, which its VM Import
guide shows how to make. Start an instance from the AMI with a key pair:
cloud-setup takes the key, the instance's name and the user data from
the instance metadata service, with the token its version 2 asks for.

### DigitalOcean

Upload the `.qcow2` as a custom image, from the control panel's
*Backups & Snapshots* page or by its web address, and make a droplet
from it with your SSH key. DigitalOcean starts custom images by BIOS,
which the image does too. cloud-setup reads the droplet's name, keys
and user data from DigitalOcean's metadata service. DigitalOcean's
guide to custom images names ext3 and ext4 for their root; Dukovo's is
Btrfs, which DigitalOcean need not read, as everything it gives comes
from the metadata service.

### VMware and VirtualBox

Use the `.vmdk` as the machine's disk, with a config disk or ISO
labelled `cidata` for the account and keys. An ISO is made with
`genisoimage -output cidata.iso -volid cidata -joliet -rock meta-data
user-data`, and goes in the machine's disc drive. CI boots the image in
VirtualBox so, under UEFI, on its SATA controller with an Intel
PRO/1000 card on NAT. With none, cloud-setup
finds nothing to set up and only grows the root, and the machine has no
account to sign in with, unless the image was made with one:
`./ob image --cloud --user alice` makes an administrator's account as
[Users and administration](users.md) shows.

### Hyper-V and Azure

Both take the `.vhdx`, but Dukovo does not drive Hyper-V's VMBus
devices yet, so it does not start there.

## When something goes wrong

cloud-setup logs each step, and each thing it leaves out:

```
$ log -s cloud-setup
cloud-setup: instance i-0123456789abcdef0 from AWS's instance metadata service
cloud-setup: naming the machine web01
cloud-setup: making an administrator's account, dukovo, with 1 SSH key(s)
cloud-setup: instance i-0123456789abcdef0 set up
```

To check user data before giving it to a cloud, put it in a folder with
a `meta-data` file and try it on any Dukovo machine:

```
$ cloud-setup --dry-run --source ./my-config
```

`--dry-run` says what would be done, the network file whole, and
changes nothing. To set a machine up again from its data, remove
`/system/state/cloud-setup/instance` and restart it.
