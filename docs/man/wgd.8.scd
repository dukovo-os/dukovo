wgd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

wgd - the WireGuard tunnels

# SYNOPSIS

*wgd*

# DESCRIPTION

wgd carries WireGuard (ADR 0340): for each file _NAME_*.conf* in
_/system/state/wireguard_ (*wireguard.conf*(5), wg-quick's format) it
brings up the interface _NAME_, a tunnel (ADR 0339), gives it the
file's addresses, and seals what the machine sends through it to the
peer whose AllowedIPs hold its destination, by UDP to that peer's
endpoint. What a peer sends is opened and reaches the interface only if
it comes from an address that peer's AllowedIPs hold, as WireGuard's
cryptokey routing asks. Its noise protocol, sessions, timers and
cookies are boringtun's, Cloudflare's. Spark runs it as the *wgd*
service, after *netd*(8) and *resolved*(8) (*spark*(8)); with no files
it waits, holding no port.

Each interface listens on one UDP port, its file's ListenPort, or one
the kernel chooses, for IPv6 and IPv4 at once. A peer without an
Endpoint is reached once it has sent a packet that opened: its endpoint
is then where that came from, and follows it as it roams. A packet for a
peer with no endpoint is dropped.

The interface's MTU is 1420. Its IPv4 and IPv6 addresses are the file's
Address, given at once, with no link-local address.

*service wgd reload* reads the files again: the interface of a new file
comes up, that of a file gone goes, and each other one is made its
file's, as *wg syncconf* would make it (*wg*(8)): a peer not in the
file is removed, and the sessions of peers whose keys, preshared key and
keepalive did not change go on.

wgd started again, as Spark does when it ends, takes on the tunnels its
last run left, so that their addresses and routes stay.

## Its sockets

*wg*(8) talks to wgd through _/system/live/wireguard/NAME.sock_, one for
each interface, which only the system may connect to (0600). Each takes
WireGuard's cross-platform text protocol (wireguard.com/xplatform):
*get=1* and *set=1* requests, keys in hex, and an *errno=*_N_ line that
ends each answer. What a *set=1* changes lasts until wgd reads the file
again.

Its service holds port 51820, the network, the right to make tunnels
and set them up, and its two folders, beyond what every service holds
(*service.toml*(5)): a file whose ListenPort is another port needs that
port in _network.listen_ too.

# SIGNALS

*SIGHUP*
	Read the files again.

*SIGTERM*
	Take the interfaces away, and end.

# FILES

_/system/state/wireguard/NAME.conf_
	Interface _NAME_'s keys, addresses and peers (*wireguard.conf*(5)).
	wgd makes the folder 0700, and warns of a file others may read.

_/system/live/wireguard/NAME.sock_
	Where *wg*(8) asks about interface _NAME_ and changes it.

_/system/settings/spark/services/wgd.toml_
	Its service (*service.toml*(5)).

# DIAGNOSTICS

*wgd: NAME is up, on UDP port PORT*
	Interface _NAME_ came up.

*wgd: PATH: line N: ...*
	What file _PATH_ says at line _N_ is wrong, and its interface does
	not come up, or keeps what it had; or a setting it passes over.

# EXIT STATUS

0 when it was ended; 1 for an argument, as it takes none, or when its
folders cannot be made.

Not yet: FwMark, which is kept and shown but marks nothing; full
tunnels, whose AllowedIPs of 0.0.0.0/0 wait for the routing table; and
DNS, which waits for *resolved*(8) to take a server for each interface.

# SEE ALSO

*wg*(8), *wireguard.conf*(5), *ip*(8), *netd*(8), *service*(8),
*spark*(8), *systools*(1)
