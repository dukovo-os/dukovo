wireguard(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

wireguard - set up private access to this machine, and add, list and remove its devices

# SYNOPSIS

*wireguard* [*show*] [*--json*]

*wireguard serve* [*--endpoint* _host_] [*--port* _port_] [*--address* _address_*/*_length_]

*wireguard add* _name_ [*--json*]

*wireguard remove* _name_

# DESCRIPTION

wireguard gives a machine private access over WireGuard: a phone or a
laptop away from home reaches this machine through one tunnel. A device
reaches the server's address on the tunnel, and so the web console and
every other service of the machine that answers there; it does not reach
the other machines of the server's networks, as Dukovo does not route
between its network cards. It manages the interface *wg0*, whose file is
_/system/state/wireguard/wg0.conf_ (*wireguard.conf*(5)) and which *wgd*(8)
carries; each device is a *[Peer]* of it, and a name for the person to
know it by. *wg*(8) stays for everything else: other interfaces, keys,
and the settings of a running interface. The web console's Security page
reads *show* as JSON and makes its changes with *serve*, *add* and
*remove*.

All of it is root's, as the file holds a private key and *wgd*'s socket
is the system's alone: run it with *elevate*(8). *show* prints public
keys and addresses only, never a private key.

*wireguard --help* prints a summary. *--json* goes with *show* and *add*,
and may come before the words or after them.

# COMMANDS

With no command, *wireguard* does *show*.

*show* [*--json*]
	The interface, and the devices it has: each with its name, its
	addresses, and what *wgd* says of it, its latest handshake and the
	bytes received and sent. With no _wg0.conf_ it says that private
	access is not set up, or prints no interface in JSON. Where *wgd*
	does not answer, the interface is not *up*, and what only *wgd*
	says of its devices is left as a dash, or null in JSON.

*serve* [*--endpoint* _host_] [*--port* _port_] [*--address* _address_*/*_length_]
	Sets the server up. With no _wg0.conf_ it makes the server's key
	and writes the file with mode 0600. With one it changes only what
	is given. Then it has *wgd* read the file: a *wgd* that runs reads it
	again (*service wgd reload*), which keeps the devices' sessions;
	one that does not run is enabled and started, as *service wgd
	enable* does. The settings are:

	- *--endpoint* _host_, what the devices are told to reach: a name,
	  of 1 to 253 bytes, in letters, digits, *-* and *.*, or an address.
	  It is kept in the file as a remark, *# endpoint:*. By default the
	  machine's name.
	- *--port* _port_, the UDP port, 1024 to 65535. By default 51820. It
	  is *ListenPort* in the file; a port other than 51820 needs a line
	  in *wgd*'s service file too (*wireguard.conf*(5)), which *serve*
	  says.
	- *--address* _address_*/*_length_, the server's address on the
	  tunnel's network, which gives the network the devices' addresses are
	  taken from: IPv4 with a length of 16 to 30, or IPv6 of 48 to 120.
	  Not the network's own address, nor, for IPv4, its broadcast one,
	  nor a loopback or multicast one. By default *10.8.0.1/24*. It is *Address* in the file, which has one
	  such line afterwards. A network that leaves a device's address
	  outside it is refused: remove the device first.

	It does not open the firewall: *serve* says to run *firewall allow
	udp* _port_ (*firewall*(8)), which the web console runs after it.
	Run again with the values the file has, it changes nothing.

*add* _name_ [*--json*]
	Adds a device called _name_: 1 to 32 letters, digits, *-* and *\_*,
	not starting with *-*. It makes the device's key pair, gives the
	device the first address of the tunnel's network that no one has
	(not the network's own, nor an IPv4 network's broadcast, nor the
	server's, nor one a device's *AllowedIPs* holds), and adds a
	*# name:* remark and a *[Peer]* with the device's public key and that
	address alone, after a blank line, at the end of the file. Then it
	has *wgd* read the file again, as *serve* does, and prints the
	device's file: its private key and address, and the server's public
	key, endpoint and port, with the server's address on the tunnel as
	its one *AllowedIPs*. The WireGuard app on a phone or a laptop reads
	it as it is. The private key is printed once and kept nowhere on the
	machine: a device whose file is lost is removed and added again.

	A name another device has is *NET-185*, and a machine without
	private access *NET-184*. A file with no *Address*, *ListenPort*,
	*PrivateKey* or *# endpoint:* is refused, with the *serve* option
	that sets it, and so is a network with no address left; nothing is
	written. When Spark cannot have *wgd* read the file, the device is
	added all the same and its file printed, and what Spark said goes
	to standard error.

*remove* _name_
	Removes the device _name_: its *# name:* remark, its *[Peer]* and
	the blank line above them, and has *wgd* read the file again, so
	that the device's tunnel closes. A device that still has its file cannot reach the machine any
	more, since its key is gone from here. Every other line stays. It
	finds a device by the name its remark gives, or, for a peer that has
	none, by the first 8 characters of its key, as *show* names it. There
	is no such device, or no private access to remove it from, for
	*NET-184*. Two devices that have the same name, which only an edit by
	hand makes, are refused: remove one of them by hand.

# JSON

With *--json*, *show* prints one line with an object of these fields, in
this order:

*format*
	1. A reader that finds another number should stop.

*interfaces*
	An array: empty with no _wg0.conf_, or with one object for *wg0*.

An interface has these fields, in this order:

*name*
	*wg0*.

*address*
	Its address and length, as *10.8.0.1/24*, the first IPv4 one the file
	has, or the first; null with none.

*port*
	Its *ListenPort*, null without one.

*endpoint*
	The *# endpoint:* remark, what the devices are told to reach; null
	without one.

*public_key*
	The key a device is told the server has, in base64, from the file's
	private key; null if the file holds none.

*up*
	Whether *wgd* has the interface and answers for it.

*devices*
	An array with an object for each *[Peer]* of the file, in its order.

A device has these fields, in this order:

*name*
	Its *# name:* remark, or the first 8 characters of its key.

*public_key*
	Its key, in base64.

*addresses*
	Its *AllowedIPs*, as *10.8.0.2/32*.

*endpoint*
	Where it came from last, as *198.51.100.7:40122*; null if it has
	not, or *wgd* does not answer.

*latest_handshake_ms*
	The time of its latest handshake, in Unix milliseconds; null if it
	has had none, or *wgd* does not answer.

*received*, *sent*
	The bytes received from it and sent to it since *wgd* started, or null
	where *wgd* does not answer.

With *--json*, *add* prints one line with an object of these fields, in
this order: *name*, the device's name; *address*, its address alone, as
*10.8.0.2/32*; *config*, the device's file, whose lines end in *\\n*;
and *live*, whether *wgd* read the file again, which is false when *wgd*
is not running or Spark refused, and *wgd* reads it when it starts.

```
{"format":1,"interfaces":[{"name":"wg0","address":"10.8.0.1/24","port":51820,
"endpoint":"home.example.org","public_key":"fMtEzEcMQNiQKKqKnUEzaD0B8XGE/K84Ir0F1p6oW5o=",
"up":true,"devices":[{"name":"phone","public_key":"1XMqslk22jUpKIFRxgd08xF4Can0h7n5Z1RT3D+Ekjs=",
"addresses":["10.8.0.2/32"],"endpoint":"198.51.100.7:40122",
"latest_handshake_ms":1763120000000,"received":1234,"sent":5678}]}]}
```

# FILES

_/system/state/wireguard/wg0.conf_
	The interface's file, mode 0600, in the folder only the system
	opens. *wireguard.conf*(5) describes it, with the two remarks this
	command reads and writes, *# endpoint:* and *# name:*.

_/system/live/wireguard/wg0.sock_
	*wgd*'s socket for the interface, which *show* asks for what only
	*wgd* knows.

# EXIT STATUS

0 when it showed what there is, set up or found set up, added the device
and printed its file, or removed the device. 1 for a device that is not
there or private access that is not set up (*NET-184*), a name in use
(*NET-185*), a file that does not read, a value out of its range, a
command that is not run as root, and a refusal from Spark, with what it
said; 1 for a mistake in the command line, which also prints its usage.

# EXAMPLES

```
$ elevate wireguard serve --endpoint home.example.org
$ elevate firewall allow udp 51820
$ elevate wireguard add phone
$ elevate wireguard show
$ elevate wireguard remove phone
```

# SEE ALSO

*wg*(8), *wgd*(8), *wireguard.conf*(5), *firewall*(8), *service*(8),
*elevate*(8), *systools*(1)
