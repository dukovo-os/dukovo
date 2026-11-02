wireguard.conf(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

wireguard.conf - a WireGuard interface's keys, addresses and peers

# SYNOPSIS

_/system/state/wireguard/NAME.conf_

# DESCRIPTION

Each file _NAME_*.conf* gives *wgd*(8) the WireGuard interface _NAME_,
1 to 15 letters, digits and *\_=+.-* (ADR 0340). Its format is
wg-quick's, as Linux keeps in _/etc/wireguard_, so that a file from a
Linux machine, a router or a VPN provider works as it is. The folder is
the system's alone (0700), as the files hold private keys: write them
as an administrator (*elevate*(8)), with a umask of 077, and then

```
# service wgd reload
```

A line is _Key_ *=* _value_, a key in any case; *#* begins a remark. A
mistake names its line, and the interface does not come up, or keeps
what it had.

## [Interface]

*PrivateKey* = _key_
	The interface's private key, in base64, as *wg genkey* makes it
	(*wg*(8)).

*ListenPort* = _port_
	The UDP port it listens on, for IPv6 and IPv4. Without it, a port
	the kernel chooses. Another port than 51820 needs a line in wgd's
	service too (*service.toml*(5)).

*Address* = _address_*/*_length_[*,* ...]
	Its IPv4 and IPv6 addresses, as *10.9.0.1/24, fd09::1/64*.

*FwMark* = _mark_ | *off*
	Kept and shown, but marks nothing yet.

*MTU* = _bytes_
	Always 1420, which leaves room in a 1500-byte frame for
	WireGuard's 32 bytes, UDP's and IPv6's: another is warned of.

*DNS*, *Table*, *PreUp*, *PostUp*, *PreDown*, *PostDown*, *SaveConfig*
	wg-quick's, warned of and passed over. Name servers are set in
	*network*(5).

## [Peer]

One for each machine it reaches.

*PublicKey* = _key_
	The peer's public key, in base64. Each [Peer] needs one.

*PresharedKey* = _key_
	A key both share, as *wg genpsk* makes it, mixed into each
	handshake.

*AllowedIPs* = _address_*/*_length_[*,* ...]
	The addresses it may send from, and that what the machine sends to
	goes to it: the longest prefix that holds an address chooses the
	peer. A prefix in two peers' is the last's.

*Endpoint* = _host_*:*_port_
	Where it is, as *vpn.example.org:51820* or *[2001:db8::1]:51820*;
	a name is looked up when the file is read. Without it, where its
	packets come from.

*PersistentKeepalive* = _seconds_ | *off*
	Sends it a packet each _seconds_ when nothing else has gone, so
	that a NAT between them keeps the way open; *25* is the usual.

# EXAMPLE

A laptop, *wg0*, reaching a server that knows its key:

```
[Interface]
PrivateKey = <the laptop's private key>
Address = 10.9.0.2/24, fd09::2/64

[Peer]
PublicKey = <the server's public key>
AllowedIPs = 10.9.0.0/24, fd09::/64
Endpoint = vpn.example.org:51820
PersistentKeepalive = 25
```

Not yet: AllowedIPs of 0.0.0.0/0 or ::/0, a full tunnel, wait for the
routing table, as the kernel routes by subnet (ADR 0339).

# SEE ALSO

*wgd*(8), *wg*(8), *network*(5), *service.toml*(5)
