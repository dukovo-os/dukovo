cloud-setup(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

cloud-setup - set a cloud machine up at its first boot

# SYNOPSIS

*cloud-setup* [*--source* _folder_] [*--dry-run*]

# DESCRIPTION

*cloud-setup* sets a cloud machine up from what its cloud says of it, in
the style of cloud-init (ADR 0328). The *cloud-setup* service runs it at
each boot of a cloud image, which *ob image --cloud* makes; on every
other system the service is off (*service*(8)).

It looks for the machine's data in this order:

. A volume labelled *cidata*, in any case, with NoCloud's files:
  _meta-data_, which it needs, _user-data_ and _network-config_. Proxmox,
  libvirt and QEMU's users attach such a disk or disc. One the kernel
  found and nothing mounted yet is waited for, 30 seconds at most.
. If the firmware's tables name the machine's maker DigitalOcean, its
  metadata service's _/metadata/v1.json_.
. If they name Amazon EC2, or the firmware is Amazon's, AWS's instance
  metadata service: a token first, as its version 2 asks, then the
  instance's ID, name and the owner's keys, and the user data.

Each service is at 169.254.169.254, which it waits up to 90 seconds to
reach, as the card may still be asking DHCP. A machine with no config
disk on neither cloud is not kept waiting: there is nothing to set up.

At every boot, it grows the partition of the root, Dukovo's Btrfs, to its
disk's end with *rdisk --grow*, and the Btrfs over it with *device resize
max*, as a cloud's disk is larger than the image copied to it; the user
data's *growpart* and *resize_rootfs* can say not to.

Once for each instance, by the ID the meta-data gives, it then:

- names the machine (*hostname*(5));
- writes _/system/settings/network_ from _network-config_ and has
  *netd*(8) read it again, when the configuration gives a card an
  address of its own;
- makes the accounts the user data lists, the default one, *dukovo*,
  if it lists none, through *authd*(8), and adds their SSH keys to their
  _~/.ssh/authorized_keys_;
- writes the files of *write_files*;
- runs the commands of *runcmd*, or the user data if it is a script.

*cloud-config*(5) says which keys of the user data it reads and what
each does. A key it does not apply is named in the log with why, as is
user data of a kind it does not read.

The ID of the instance set up is kept in
_/system/state/cloud-setup/instance_: at a later boot of the same
machine, only the root grows. A machine made from a copy of its disk has
an instance of its own, and is set up again.

# OPTIONS

*--source* _folder_
	Read _meta-data_, _user-data_ and _network-config_ from _folder_,
	not from a config disk or a cloud: to try a NoCloud folder on a
	machine as it is.

*-n*, *--dry-run*
	Say what would be done, the network file whole, and change nothing.

# THE DEFAULT ACCOUNT

The account a cloud machine is made with is *dukovo*, or the name the
user data's *user* gives (*cloud-config*(5)): an administrator
with the cloud's SSH keys and the user data's *ssh_authorized_keys*,
who elevates without a password (*user*(8)'s
*--elevate-without-password*), as Ubuntu's *ubuntu* uses sudo. Its
password is the user data's *password* if it gives one, and otherwise
one no one knows: it signs in with its keys alone.

# LOG

Each line it writes starts with *cloud-setup:*, as

```
cloud-setup: instance iid-local01 from /volumes/CIDATA
cloud-setup: left out packages: Dukovo installs apps with bundle(1)
cloud-setup: naming the machine web01
cloud-setup: making an administrator's account, dukovo, with 1 SSH key(s)
cloud-setup: instance iid-local01 set up
```

*log -s cloud-setup* shows them (*log*(1)).

# EXIT STATUS

0 if everything was done; 1 if a step failed, which the log names, or
for a mistake in the command line. A step that fails does not stop the
others.

# FILES

_/system/state/cloud-setup/instance_
	The ID of the instance set up last.

_/system/state/cloud-setup/user-data-script_
	User data that is a script, as it was run.

_/system/settings/spark/services/cloud-setup.toml_
	The service, off unless an image turns it on.

# DIFFERENCES FROM CLOUD-INIT

- It reads part of cloud-init's user data, listed in *cloud-config*(5),
  and NoCloud, AWS and DigitalOcean; not cloud-init's other sources,
  such as Google Cloud's, Azure's or OpenStack's config drive.
- Its modules have no stages or frequencies of their own: the root grows
  at every boot, and the rest is done once for each instance.
- A crypt(3) hash of a password cannot be Dukovo's Argon2id: give
  *plain_text_passwd*, or set the password with *passwd*(1).
- There is no *cloud-init clean*: to set a machine up again, remove
  _/system/state/cloud-setup/instance_ and restart it.

# SEE ALSO

*cloud-config*(5), *rdisk*(8), *device*(8), *user*(8), *elevate*(8),
*authd*(8), *netd*(8), *network*(5), *hostname*(5), *service*(8),
*sshd*(8), *log*(1), *ob*(1)
