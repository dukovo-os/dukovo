netd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

netd - set the network cards up, by DHCP or as their settings say

# SYNOPSIS

*netd*

*netd leases*

# DESCRIPTION

netd gives each network card its addresses, its router and its DNS
servers (ADR 0191). Spark runs it as the *netd* service on every
machine, after the consoles and before the services that use the
network, with *admin.network* (*spark*(8)).

A card gets them by DHCP unless _/system/settings/network_ names it
(*network*(5)), which may give it addresses of its own or have netd
leave it alone. A card that already has an address when netd first
looks, and no lease, which the kernel's *ip=* flag (*dukovo-boot*(7))
or an administrator gave it, is left alone too. netd sets a card up
through the network capability (*radix-network*(2)), as *ip*(8) does,
and changes only the addresses it gave: an administrator's stay beside
them, up to the card's four.

netd's client is DHCPv4's (RFC 2131). It asks for an address by
broadcast from port 68 of the card, waiting 4, 8, 16, 32 and then 64
seconds between tries, a second more or less at random; renews the
lease with its server at half its time and with any server at seven
eighths; and gives the address up when the lease ends, and asks again.
It asks for the address, router, DNS servers, domain name, lease time
and the times to renew and rebind, sends the card's MAC address as its
client identifier, and no host name. A server's answer is used only
for what passes netd's checks: a unicast address, a router on its
network, unicast DNS servers, a domain name of letters, digits, *-* and
*\_*, and a lease of at least a minute.

Each card's IPv6 is on unless its settings say no (ADR 0331): the
kernel makes its link-local address and, from the prefixes its routers
advertise, a stable address (RFC 7217), made from a secret of the
machine's that netd makes once and keeps in
_/system/state/netd/ipv6-secret_, and temporary ones (RFC 8981), which
it sends from, unless the settings say *temporary-addresses = no*. The
kernel takes the routers, DNS servers (RFC 8106) and MTU the adverts
carry; netd gives it the settings' static IPv6 addresses and router.

When a card's routers say that addresses (their adverts' M flag) or
other settings (O) come from DHCPv6, netd asks for them too, as the
settings' *dhcpv6* allows (ADR 0335): its client (RFC 8415) asks for
the DNS servers and domains with INFORMATION-REQUEST, or SOLICITs an
address in an IA_NA and keeps it with RENEW and REBIND. It is known by
its DUID-LL, made from the card's MAC address, and sends from port 546
of the card. The address it gets is given to the card as *ip*(8) gives
one; its DNS servers and domains go after DHCP's.

Bridges and VLANs that *ip*(8) makes (ADR 0333) are set up as cards
are, by their names; a bridge's port is not, while it is one, as its
bridge has the addresses.

The kernel lets each card's socket hold ports 67, 68 and 546 beside the
others', as each is bound to its own card (*radix-posix*(7)). While another socket
holds the port on a card, the card waits for it, and netd tries again
each time it looks at the cards.

# THE DHCP SERVER

A card with an address of its own and a *dhcp-server* range in
_/system/settings/network_ (*network*(5)) gives the computers on its
network their addresses (RFC 2131, ADR 0486). Its server has a thread
and a socket on port 67 of that card, so that a card's server hears
only its own network, and a machine with several networks can serve
each with its own pool. IPv4 only: IPv6 clients keep the addresses
their network's router has them make, and DHCPv6 and router
advertisements are not served.

A client gets, in this order: the address reserved for its MAC address;
the address it had last, kept by its client identifier, or by its MAC
address when it sends none; the address it asks for, if it is free;
the lowest free address of the range. A lease ends after the time the
settings give, and the client renews it at half that time (T1) and
rebinds at seven eighths (T2). An address whose lease ended is given
to another client only once no address that was never given is free,
so that a client that comes back usually finds its own. An address a
client declines, saying another machine uses it, is left alone for ten
minutes. A client that releases its address gives it back at once. A
reserved address is never given to another client, and an address with
a lease is never given to another.

The server tells a client its address, the network's mask and
broadcast address, the lease time and T1 and T2, and, as the settings
and their defaults give them, a router, DNS servers, a domain name and a
time server. When the settings say the connection is metered it tells
the client so by the vendor option that names ANDROID_METERED. A client
that asks only for settings (DHCPINFORM) gets them without a lease.

It answers only what is well formed and comes from an Ethernet client on
the card's network: other packets are ignored, never answered, and a
request made through a relay agent (*giaddr* set) is ignored too, which
netd says once in the log. A client's answer to another server's offer
is not answered. A request for an address that is not the client's, or
not on this network, gets a DHCPNAK, which has the client ask for a new
one; a client the server has no record of, asking for the address it
had before it restarted, is left unanswered, as RFC 2131 has a server
do that may not be the only one. An answer goes to the client's address
if it has one on the network, and otherwise to the broadcast address;
a DHCPNAK always goes to the broadcast address.

Each lease is kept in _/system/state/netd/dhcp-server-_*card* before
the client is told it has it, so that the leases survive a restart of
netd and of the machine. The settings read again give the server its
new pool at once and keep each lease that still fits it.

The server's lines in the log, after *netd:*, are one for each new
lease, decline and release, and for the first time a pool is full:

```
netd: net1: giving addresses 192.168.10.100-192.168.10.199 by DHCP, for 1 day each; 0 leases kept
netd: net1: gave 192.168.10.20 to 52:54:00:12:34:58 (printer) for 1 day
netd: net1: 52:54:00:12:34:56 released 192.168.10.100
netd: net1: the pool is full: 52:54:00:12:34:60 gets no address until a lease ends
```

# THE LEASES

*netd leases* shows the leases the servers have given, a line for each,
and an address that is reserved but has none, with when it ends:

```
$ netd leases
CARD  ADDRESS         MAC                NAME     LEFT
net1  192.168.10.20   52:54:00:12:34:58  printer  reserved
net1  192.168.10.100  52:54:00:12:34:56  laptop   23h 41m
```

It reads _/system/live/netd/leases_, which anyone may read, so it needs
no privilege.

# METERED CONNECTIONS

netd knows which connection costs by the byte: a card's *metered*
setting (*network*(5)) says, and without it the vendor option of its
DHCP lease, which names ANDROID_METERED when a server such as an
Android phone's hotspot says so. netd writes _/system/live/netd/metered_
each time that changes: its first line is *yes* or *no* for the card
that carries the IPv4 default route (else the IPv6 one, else *no*), and
a line follows for each card, as *net0 no*. *updated*(8) reads it and
holds its downloads back (*updates.toml*(5)). netd logs it when the
first line changes, as *netd: net0 is metered: updates wait for another
connection*.

netd keeps each lease in _/system/state/netd_, a file for each card
named by its MAC address. netd started again goes on with the lease the
card has, and its connections go on; after a reboot the card asks for
the address it had, and for any if no server answers.

netd writes the DNS servers, three at most, the settings' first, then
DHCP's, DHCPv6's and those routers advertise, and the leases' domain
names as *search*, into
_/system/live/netd/resolv.conf_, for *resolved*(8), which every program
asks, to forward queries to unless its own settings name servers.

Its service holds only its leases and that folder beyond what every
service reads, and ports 67, 68 and 546 on every card
(*service.toml*(5)): it cannot change the settings.

netd looks at the cards every five seconds: it sets up a card that
came, forgets one that went, and has a card whose link came back ask
for its address again.

What netd does goes to its standard error, which Spark copies into the
system's log after *netd:*:

```
netd: net0: asking for an address by DHCP
netd: net0 has 10.0.2.15/24 from 10.0.2.2 for 1 day, router 10.0.2.2, DNS 10.0.2.3
netd: net0: asking DHCPv6 for its settings
netd: net0 has DNS fec0::3 by DHCPv6
```

# SIGNALS

*SIGHUP*
	Read _/system/settings/network_ again and set each card up anew:
	*service netd reload* (*service*(8)) sends it.

*SIGTERM*
	End, leaving the cards as they are.

# FILES

_/system/settings/network_
	The settings (*network*(5)).

_/system/state/netd/_*52-54-00-12-34-56.lease*
	A card's lease: its address, router, DNS servers, domain, server,
	when it was given (seconds since 1970) and for how long, and when
	to renew and rebind it, and *metered = yes* when its server says
	the connection is metered, a *key = value* a line.

_/system/state/netd/dhcp-server-_*net1*
	The leases the server of that card has given, a line each: the
	client's MAC address, the address, when it ends (seconds since
	1970), the client's identifier and its name.

_/system/live/netd/leases_
	The leases of all the servers, as *netd leases* shows them, a line
	each: the card, the address, the MAC address, the name or *-*, and
	when it ends or *reserved*.

_/system/live/netd/metered_
	Whether the connection is metered, for *updated*(8).

_/system/state/netd/ipv6-secret_
	The machine's IPv6 secret, which its stable addresses are made from:
	a new one gives it new addresses. Only netd reads it.

_/system/live/netd/resolv.conf_
	The DNS servers, which *resolved*(8) asks.

_/system/settings/spark/services/netd.toml_
	Its service (*service.toml*(5)).

# EXIT STATUS

0 when it was ended; 1 for an argument it does not know. *netd leases*
exits 0 after showing the leases and 1 when no card gives addresses by
DHCP.

Not yet: a DHCPv6 server and router advertisements, DHCP relay, DHCPv6
prefix delegation, an ARP probe of an address before it is offered,
and DHCPDECLINE and DHCPRELEASE from its client.

# SEE ALSO

*network*(5), *timed*(5), *updates.toml*(5), *updated*(8), *ip*(8),
*service*(8), *service.toml*(5), *spark*(8), *radix-network*(2),
*systools*(1)
