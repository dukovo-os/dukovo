guest-agent(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

guest-agent - answer a virtual machine's host as QEMU's guest agent does

# SYNOPSIS

*guest-agent* [*--port* _path_]

# DESCRIPTION

*guest-agent* answers the host of a virtual machine in the protocol of
QEMU's guest agent, as qemu-ga does on Linux (ADR 0329), so that Proxmox,
libvirt and OpenStack show the machine's addresses and system and can
shut it down cleanly. The *guest-agent* service runs it at each boot
(*service*(8)).

The host talks over the virtio console port that QEMU names
*org.qemu.guest_agent.0*, which the kernel links from
_/devices/virtio-ports/org.qemu.guest_agent.0_ once the host has named
it. On a machine with a virtio console it waits up to 10 seconds for
that name, which can come after the service starts; on a machine without
one, as a computer, it ends at once, saying there is nothing to answer.
When the host names no such port in that time, it ends saying which
ports the host did name, so that a channel set up under another name
shows in the log.

Each request is a JSON object, *{"execute": "guest-ping"}*, with
*"arguments"* if the command takes any and an *"id"* that the answer
repeats. Each answer is one line of JSON: *{"return": ...}*, or
*{"error": {"class": ..., "desc": ...}}* with QEMU's class:
*CommandNotFound* for a command it does not answer, *GenericError* for
the rest. A 0xFF byte throws away what came before it, as clients send
to begin afresh.

# COMMANDS

*guest-sync* {"id": _n_}, *guest-sync-delimited* {"id": _n_}
	Answer _n_, the second after a 0xFF byte, so that the host can skip
	what an earlier client left unread.

*guest-ping*
	Answers nothing but that it is there: *{"return": {}}*.

*guest-info*
	Its version and the commands it answers.

*guest-get-osinfo*
	The system: *id* *dukovo*, its name and version, the kernel's
	release and version, and the machine, *x86_64*.

*guest-get-host-name*
	The machine's name (*hostname*(5)).

*guest-get-time*
	The time, in nanoseconds since 1970.

*guest-network-get-interfaces*
	Each network interface, with its hardware address, its IPv4 and
	IPv6 addresses and their prefixes, and its counts of bytes,
	packets, errors and drops, as *ip*(8) shows them.

*guest-shutdown* [{"mode": "powerdown" | "reboot" | "halt"}]
	Powers the machine off, the default, restarts it or halts it,
	stopping the services first as *shutdown*(8) does. It answers
	nothing, as qemu-ga does not.

Commands that run programs, read and write files, set passwords or
freeze filesystems (*guest-exec*, *guest-file-open*,
*guest-set-user-password*, *guest-fsfreeze-freeze* and the others) are
not answered: they are left for later.

# OPTIONS

*--port* _path_
	Answer on _path_ instead.

# PROXMOX AND LIBVIRT

In Proxmox, turn on *QEMU Guest Agent* in the machine's *Options*; its
summary then shows the machine's addresses, and *Shutdown* ends it
cleanly. libvirt adds the channel with

```
<channel type='unix'>
  <target type='virtio' name='org.qemu.guest_agent.0'/>
</channel>
```

and *virsh domifaddr --source agent* and *virsh shutdown --mode agent*
use it. With QEMU alone:

```
-device virtio-serial-pci \\
-chardev socket,path=/tmp/qga.sock,server=on,wait=off,id=qga0 \\
-device virtserialport,chardev=qga0,name=org.qemu.guest_agent.0
```

then, on the host, *echo '{"execute":"guest-ping"}' | socat -
unix-connect:/tmp/qga.sock*.

# LOG

Its lines start with *guest-agent:*; *log -s guest-agent* shows them
(*log*(1)).

# EXIT STATUS

0 when there is no port to answer on; 1 when the port goes or cannot be
opened, after which the service starts it again.

# SEE ALSO

*cloud-setup*(8), *service*(8), *shutdown*(8), *ip*(8), *hostname*(5),
*log*(1)
