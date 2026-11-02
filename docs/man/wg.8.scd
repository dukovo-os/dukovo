wg(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

wg - WireGuard's keys, and what its interfaces have

# SYNOPSIS

*wg* [*show* [_interface_ | *all* | *interfaces*] [_field_]]

*wg showconf* _interface_

*wg set* _interface_ [*listen-port* _port_] [*fwmark* _mark_]
\[*private-key* _file_] [*peer* _key_ [*remove*] [*preshared-key* _file_]
\[*endpoint* _host_*:*_port_] [*persistent-keepalive* _seconds_|*off*]
\[*allowed-ips* _prefix_[*,*_prefix_]...]]...

*wg* *setconf* | *addconf* | *syncconf* _interface_ _file_

*wg* *genkey* | *genpsk* | *pubkey*

# DESCRIPTION

wg is wireguard-tools' wg, for the interfaces *wgd*(8) carries (ADR
0340): it takes the same commands and says what they say, in the same
words and formats, so that scripts written for Linux's work. It talks
to wgd through _/system/live/wireguard/NAME.sock_, which only the system
may use: run it as an administrator (*elevate*(8)). The interfaces come
from the files in _/system/state/wireguard_ (*wireguard.conf*(5)); what
wg changes lasts until wgd reads them again, at *service wgd reload*.

*show* [_interface_ | *all*]
	Each interface's public key, port and peers, or the one named's:
	for each peer, its endpoint, AllowedIPs, latest handshake, what it
	sent and received, and its keepalive. Peers are shown by their
	latest handshake, the latest first. On a terminal its words are
	coloured, as *WG_COLOR_MODE* (*always* or *never*) may say
	otherwise. Without a word, *all*.

*show interfaces*
	The interfaces' names, on one line.

*show* _interface_ | *all* _field_
	One field, a line for each of what it holds, by tabs, with the
	interface's name first for *all*: *public-key*, *private-key*,
	*listen-port*, *fwmark*, *peers*, *preshared-keys*, *endpoints*,
	*allowed-ips*, *latest-handshakes* (seconds since 1970, 0 for
	never), *transfer* (bytes received, then sent),
	*persistent-keepalive* or *dump*, which is every field of the
	interface on its first line and of each peer on its own.

*showconf* _interface_
	The interface's settings and peers, as a file *setconf* takes.

*set* _interface_ ...
	Change what the interface has: its port, mark or private key (read
	from _file_), and each *peer* named by its public key: *remove* it,
	or give it a preshared key (from _file_; an empty one for none), an
	endpoint, a keepalive, or AllowedIPs in place of its own. A peer
	not there is added. A prefix given to one peer moves from
	another, as on Linux.

*setconf* _interface_ _file_
	Give the interface _file_'s settings and peers in place of all it
	has. _file_ is *wireguard.conf*(5)'s, whose Address and other
	wg-quick settings are passed over, with a word.

*addconf* _interface_ _file_
	Add _file_'s settings and peers to the interface's.

*syncconf* _interface_ _file_
	Make the interface's settings and peers _file_'s, keeping the
	sessions of the peers that did not change.

*genkey*
	A new private key, in base64: Curve25519's, clamped. It warns if
	standard output is a file others may read.

*genpsk*
	A new preshared key: 32 random bytes, in base64.

*pubkey*
	The public key of the private key on standard input.

# EXAMPLES

Make a key pair, kept from others:

```
# umask 077
# wg genkey > private
# wg pubkey < private > public
```

Show what *wg0* has, and its peers' latest handshakes:

```
# wg show wg0
# wg show wg0 latest-handshakes
```

# EXIT STATUS

0 when it did what was asked; 1 when it could not, as when wgd is not
running or the interface is not there, saying why.

# SEE ALSO

*wgd*(8), *wireguard.conf*(5), *ip*(8), *elevate*(8), *service*(8),
*systools*(1)
