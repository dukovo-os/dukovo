<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Cloud and virtual machines

This chapter describes running Dukovo as a cloud machine: on AWS,
Azure, DigitalOcean, Proxmox, OpenStack, Hyper-V, VMware, VirtualBox or
plain QEMU. One
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
turned on, in five formats:

| File | For |
| --- | --- |
| `out/dukovo-0.13.0-x86_64-cloud.img` | raw: AWS, and any cloud that takes a raw disk |
| `out/dukovo-0.13.0-x86_64-cloud.qcow2` | KVM, Proxmox, OpenStack and DigitalOcean |
| `out/dukovo-0.13.0-x86_64-cloud.vmdk` | VMware and VirtualBox |
| `out/dukovo-0.13.0-x86_64-cloud.vhdx` | Hyper-V |
| `out/dukovo-0.13.0-x86_64-cloud.vhd` | Azure: a fixed VHD, as large as the disk |

The four last are written by the build machine's `qemu-img` (the
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
4. **Azure's instance metadata service**, when it names Microsoft, with
   the chassis asset tag Azure gives its machines.

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
user-data`, and goes in the machine's disc drive. In VirtualBox, set
the machine's chipset to ICH9 (Settings, System, Motherboard): Dukovo
finds PCI devices through PCI Express's configuration space, which
VirtualBox's default chipset, the PIIX3, does not describe, and with it
the machine finds no disks. CI boots the image in VirtualBox so, under
UEFI, on its SATA controller with an Intel PRO/1000 card on NAT. With
no config disk, cloud-setup
finds nothing to set up and only grows the root, and the machine has no
account to sign in with, unless the image was made with one:
`./ob image --cloud --user alice` makes an administrator's account as
[Users and administration](users.md) shows.

Under VMware's hypervisors, the disk may be on any of the machine's
controllers, VMware's paravirtual one (PVSCSI) included, and the card
either the E1000 or VMware's VMXNET3, the faster pair. The kernel runs
the guest's side of VMware's tools itself (vmware(4)): the host shows
the tools as running, so its Shut Down Guest and Restart Guest work,
and shows the machine's IP address and uptime. Nothing is installed for
them, and the host never offers to upgrade them.

### Hyper-V

Make a generation 2 machine, which starts by UEFI, on the `.vhdx`, and
turn its Secure Boot off (Settings, Security), as Dukovo images are not
signed for Secure Boot before 1.0:

```
PS> New-VM -Name dukovo -Generation 2 -MemoryStartupBytes 4GB `
      -VHDPath .\dukovo-0.13.0-x86_64-cloud.vhdx -SwitchName "Default Switch"
PS> Set-VMFirmware dukovo -EnableSecureBoot Off
PS> Start-VM dukovo
```

Dukovo drives Hyper-V's own devices over its VMBus: the SCSI
controller's disks, the network card, the keyboard, and the services
that shut the machine down from Hyper-V Manager, keep its clock and
report it alive (its heartbeat). A generation 2 machine has no other
disk or network devices. Give it a config disk, as for VMware below,
for the account and keys: Hyper-V has no metadata service of its own.

Not driven yet: Hyper-V's mouse, its video device past the firmware's
screen, dynamic memory, and the key-value exchange that shows the
machine's addresses in Hyper-V Manager.

### Azure

Azure takes the fixed `.vhd`, uploaded as a page blob, and makes a
generation 2 image from it, as its guide to uploading a generalized VHD
shows:

```
$ az storage blob upload --type page --account-name mystorage \
    --container-name images --name dukovo.vhd \
    --file out/dukovo-0.13.0-x86_64-cloud.vhd
$ az image create --resource-group mygroup --name dukovo-0.13.0 \
    --os-type Linux --hyper-v-generation V2 \
    --source https://mystorage.blob.core.windows.net/images/dukovo.vhd
$ az vm create --resource-group mygroup --name web01 --image dukovo-0.13.0 \
    --security-type Standard --admin-username azureuser \
    --ssh-key-values ~/.ssh/id_ed25519.pub --user-data cloud.yaml
```

The machine needs its security type Standard, as trusted launch starts
it with Secure Boot on. cloud-setup reads the machine's name, the admin
account's name and keys, and the user data from Azure's instance
metadata service, and makes the default account with the admin
account's name, `azureuser` here. At each boot it then tells Azure's
wire server the machine is ready, as Azure's agent does: until a new
machine does so, Azure takes its deployment to be still under way.

Give the configuration as user data (`--user-data`): Azure's custom data
(`--custom-data`) comes only on its provisioning disk, which cloud-setup
does not read. Azure's accelerated networking, its MANA cards, is not
driven yet: make the machine without it (the default for `az vm
create` on most sizes), and its network card is Hyper-V's own.

## When something goes wrong

cloud-setup logs each step, and each thing it leaves out:

```
$ log -s cloud-setup
cloud-setup: instance i-0123456789abcdef0 from AWS's instance metadata service
cloud-setup: naming the machine web01
cloud-setup: making an administrator's account, dukovo, with 1 SSH key(s)
cloud-setup: instance i-0123456789abcdef0 set up
```

On Hyper-V and Azure, the kernel's log (dmesg(1)) says which of
Hyper-V's devices it found and drives, in its lines starting `Hyper-V:`.

To check user data before giving it to a cloud, put it in a folder with
a `meta-data` file and try it on any Dukovo machine:

```
$ cloud-setup --dry-run --source ./my-config
```

`--dry-run` says what would be done, the network file whole, and
changes nothing. To set a machine up again from its data, remove
`/system/state/cloud-setup/instance` and restart it.
