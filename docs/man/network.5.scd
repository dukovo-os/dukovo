network(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

network - how netd sets the network cards up

# SYNOPSIS

_/system/settings/network_

# DESCRIPTION

*netd*(8) gives every network card its address, router and DNS servers
by DHCP, unless this file names the card (ADR 0191). The default file
says nothing but comments, so every card asks DHCP. *service netd
reload* has netd read the file again and set each card up anew.

A card with an address of its own can in turn give the other computers
on its network their addresses by DHCP, with the *dhcp-server* settings
below (ADR 0486).

A section starts with the card's name in brackets, *[net0]*, or its MAC
address, *[52:54:00:12:34:56]*, which stays the card's when cards come
in another order; a bridge or VLAN that *ip*(8) makes is named by its
name, *[br0]* or *[net0.7]* (ADR 0333); upper and lower case are the same. The first section
that names a card is the one it takes. The settings after it, one a
line, are _setting_ *=* _value_. A line that starts with *#* is a
comment.

# SETTINGS

*address =* _address_[/_prefix_]
	An address of the card's own, without a prefix a /32; up to four,
	the first the one it sends from. An IPv6 one, without a prefix a
	/128, is given to the card beside those it makes itself (ADR 0331),
	up to the card's limit; a link-local one is not. A card with addresses does not
	ask DHCP unless *dhcp = yes* says to, and then has its lease's
	address first and these after it.

*router =* _address_
	The card's router, its default route. It must be on the network
	of one of the card's addresses. With *dhcp = yes*, the lease's
	router comes first. An IPv6 one is the card's IPv6 default route,
	beside those routers advertise.

*dns =* _address_
	A DNS server, IPv4 or IPv6, up to three; netd writes them into
	_/etc/resolv.conf_ before those DHCP, DHCPv6 and routers give.

*dhcp =* *yes* | *no*
	Whether the card asks DHCP. The default is *yes* for a card with no
	*address*, else *no*. *dhcp = no* with no *address* has netd leave
	the card alone, as the *ip=* flag (*dukovo-boot*(7)) or *ip*(8)
	sets it up.

*ipv6 =* *yes* | *no*
	Whether the card has IPv6: its link-local address, and the
	addresses it makes from the prefixes its routers advertise. The
	default is *yes*.

*temporary-addresses =* *yes* | *no*
	Whether it makes temporary addresses too (RFC 8981), which it sends
	from, so that what it connects to cannot tell one day's connections
	from the next. The default is *yes*; its stable addresses (RFC
	7217) it always makes.

*dhcpv6 =* *auto* | *settings* | *address* | *no*
	When it asks DHCPv6 (ADR 0335): *auto*, the default, as its routers
	say, for an address and the settings when they set the M flag, the
	settings alone when they set O; *settings*, always for the DNS
	servers and domains; *address*, always for an address too; *no*,
	never.

*metered =* *yes* | *no* | *auto*
	Whether the connection through this card costs by the byte, which
	*updated*(8) takes into account (*updates.toml*(5)). *auto*, the
	default, is *yes* when the server of the card's DHCP lease says so
	(its vendor option names ANDROID_METERED, as an Android phone's
	hotspot does) and *no* otherwise. A card with addresses of its own
	and no lease is *no* unless this says *yes*.

*dhcp-server =* _first_*-*_last_
	Turns the card's DHCP server on: it gives the clients on the card's
	network the addresses from _first_ to _last_, both IPv4 ones. The
	card needs an address of its own, which the range lies inside the
	network of and does not hold, nor the network's own address or its
	broadcast address, and *dhcp = yes* cannot go with it. At most
	65536 addresses. A server that cannot run is left off, and the log
	says why. Every setting below belongs to it.

*dhcp-server-lease =* _time_
	How long a lease lasts: a number and *s*, *m*, *h* or *d*, from
	*1m* to *30d*; *12h* by default. A client asks for more at half of
	it.

*dhcp-server-router =* _address_ | *none*
	The router the clients are told of, on the card's network. By
	default the card's own *router*, if it is on that network, and
	otherwise none. *none* tells them of no router.

*dhcp-server-dns =* _address_[ _address_...] | *none*
	The DNS servers the clients are told of, IPv4 ones, three at most
	in all, on one line or on several. By default the IPv4 DNS servers
	that are not loopback addresses in the _resolv.conf_ netd writes
	(*netd*(8)), which the cards' settings and leases gave. *none*
	tells them of none.

*dhcp-server-domain =* _name_
	The domain name the clients are told of: labels of letters,
	digits, *-* and *\_*, with dots between them, up to 253
	characters. Without it they are told of none.

*dhcp-server-time =* _address_ | *none*
	The time server the clients are told of (DHCP option 42), which
	they ask in plain NTP. By default the card's own address while
	*timed*(8) answers plain NTP from the clients' network (*serve =
	nts+ntp* in *timed*(5), with *serve-allow* letting them in), and
	otherwise none. *none* tells them of none.

*dhcp-server-metered =* *yes* | *no*
	With *yes* the clients are told that the connection is metered, by
	the vendor option that names ANDROID_METERED, so that a client that
	takes it into account, such as *updated*(8) on another Dukovo
	machine, waits to download. For a machine whose own uplink is
	metered. *no* is the default.

*reserve =* _mac_ _address_[ _name_]
	Always gives the client with the MAC address _mac_ the address
	_address_ and never gives it to another; the address lies on the
	card's network, and may lie outside the range. _name_ is how
	*netd leases* lists it: up to 63 letters, digits, *-*, *\_* and
	*.*. A MAC address and an address are in at most one *reserve*,
	and a card has up to 1024. The line may come any number of times.

netd says what is wrong in the file in the system's log, with the line's
number, and leaves out that line: a setting it does not know, an
address that is not a unicast one, a fifth address or a fourth DNS
server, a second router of a version, or a second *dhcp*, *ipv6*,
*temporary-addresses*, *dhcpv6*, *metered* or *dhcp-server* setting, a
router on none of the card's networks, a value that is not what the
setting takes, a reservation that is not a client's address on the
card's network, or a card named twice.

# EXAMPLES

Every card by DHCP but *net1*, which has an address of its own, and a
card that is set up by hand:

```
[net1]
address = 192.168.1.10/24
router = 192.168.1.1
dns = 192.168.1.1

[52:54:00:ab:cd:ef]
dhcp = no

[br0.7]
dhcp = no
address = 192.168.7.1/24
address = fd07::1/64
temporary-addresses = no
```

A card that gives the computers on its network their addresses, for a
day each, with a router, two DNS servers and a domain, and keeps the
address of the printer:

```
[net1]
dhcp = no
address = 192.168.10.1/24
dhcp-server = 192.168.10.100-192.168.10.199
dhcp-server-lease = 1d
dhcp-server-router = 192.168.10.254
dhcp-server-dns = 192.168.10.254 9.9.9.9
dhcp-server-domain = office.example
reserve = 52:54:00:12:34:58 192.168.10.20 printer
```

# FILES

_/system/settings/network_
	This file; Spark copies the default one there when there is none
	(*spark*(8)).

# SEE ALSO

*netd*(8), *timed*(5), *updates.toml*(5), *ip*(8), *spark*(8)
