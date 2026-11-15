network(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

network - show the network cards, and change the settings netd gives them

# SYNOPSIS

*network* [*show*] [_card_] [*--json*]

*network set* _card_ *dhcp* [*metered* _when_] [*--try* _seconds_]

*network set* _card_ *fixed* _address_/_prefix_... [*router* _address_] [*dns*
_address_...] [*metered* _when_] [*--try* _seconds_]

*network keep* | *undo*

# DESCRIPTION

network shows each network card with what it has now, what *network*(5)
says of it, and the DHCP lease it holds (ADR 0518). It is the one command
that puts the kernel's view of a card (*ip*(8)) and *netd*(8)'s settings
side by side, which the web console's Networking page reads as JSON.

A card is every network card, bridge and VLAN that *netd* sets up: not
the loopback, not a tunnel such as a WireGuard interface, whose addresses
its own program gives, and not a bridge's port, whose bridge has the
addresses.

*show* runs as anyone. It reads what everyone may: the kernel's list of
the cards, the settings file, the name servers *netd* writes, *netd*'s
lease files, and Spark's answer to *show*. What it cannot read is left
out and does not stop it: a lease it may not read is shown as none.

*set* changes how *netd* sets one card up: by DHCP, or with addresses,
a router and name servers of its own. It writes the card's section of
_/system/settings/network_ and has *netd* read the file again, which
keeps the connections on the addresses that stay. With *--try*, Spark
puts the file back by itself unless *network keep* keeps the change in
time, so that a change that cuts the machine off from the network comes
undone without anyone at it (*spark*(8), ADR 0472). *set*, *keep* and
*undo* need root, whose the file and Spark's control socket are: run
them with *elevate*.

# COMMANDS

With no command, *network* does *show*. A word that is no command is the
name of a card, so *network net0* is *network show net0*. *network --help*
prints a summary.

*show* [_card_] [*--json*]
	Each card, or the one named _card_ by its name or its MAC address (in
	upper or lower case), in the order of the kernel's numbers. For each: its
	name, MAC address, whether its link is up and its speed when the card
	says; its addresses, each with where it came from; its routers and
	the name servers; its settings; and its lease. When the *network*
	settings are being tried, the time left before they are undone is
	shown last.

*set* _card_ *dhcp* [*metered* _when_]
	The card asks DHCP for its address, router and name servers.
	A card that gives addresses by DHCP itself (*dhcp-server* in
	*network*(5)) needs an address of its own, so it cannot (*NET-178*).

*set* _card_ *fixed* _address_/_prefix_... [*router* _address_] [*dns* _address_...] [*metered* _when_]
	The card has the addresses given, one to four, IPv4 or IPv6, each
	with its prefix (1 to 32, or 1 to 128), none of them link-local or
	given twice, and asks no DHCP. The router must be on the network of
	one of them; an IPv6 router may be link-local. There are up to three
	name servers. A card that gives addresses by DHCP keeps an address on
	the network of its range (*NET-178*). The keys after the addresses
	may come in any order.

	_when_ is *auto*, *yes* or *no*: whether the connection costs by the
	byte (*network*(5)); without *metered*, it stays as it was.

	*set* edits only the card's section: the first that names the card,
	by its name or its MAC address, as *netd* reads the file, or a new one
	named by its MAC address at the end of the file, with a comment that
	says so. It takes out the section's *address*, *router*, *dns* and
	*dhcp* lines, and *metered* when it is given, and writes the new ones
	where the first was. Every other line, comment and section stays as
	it was. Before it writes, it reads the new file as *netd* will, and
	writes nothing unless *netd* would set the card up as asked and find
	nothing new to leave out. It refuses while the network settings are
	being tried (*NET-176*), since the trial's undo would put back the
	file as it was before it.

*--try* _seconds_
	With *set*: try the change for 10 to 3600 seconds. Spark keeps a copy
	of the file first, then the new file is put in place and *netd* reads
	it; unless *network keep* keeps the change before the time is up,
	Spark puts the copy back and has *netd* read it, and so it does when
	the machine starts again before the change was kept. A change *netd*
	refuses is undone at once.

*keep*
	Keeps the change being tried: Spark drops its copy and its timer.

*undo*
	Undoes the change being tried now, without waiting for the time.

	Both say *NET-179* when nothing is being tried.

*--json* and *--try* may come before the words or after them. *--json*
goes with *show* alone, and *--try* with *set*.

# OUTPUT

```
$ network show
net0  52:54:00:12:34:56  link up, 1000 Mb/s
  addresses  10.0.2.15/24 (dhcp), fe80::5054:ff:fe12:3456/64 (link-local)
  routers    10.0.2.2
  dns        10.0.2.3
  settings   no section, dhcp, metered auto
  lease      10.0.2.15, ends in 11:12:40
```

The addresses name where each came from: *dhcp* (DHCP, or DHCPv6),
*fixed* (an *address* of the card's section in *network*(5)), *router*
(made from what a router advertised), *link-local*, and *other* (given
with *ip address add*, or by the *ip=* flag of *dukovo-boot*(7)). An
IPv6 address that is still being tested is not shown, as it is not the
card's yet.

After *set*, it says what it wrote and how it is used:

```
$ elevate network set net0 fixed 10.0.2.15/24 router 10.0.2.2 dns 10.0.2.3 --try 120
network: net0: fixed 10.0.2.15/24, router 10.0.2.2, dns 10.0.2.3
/system/settings/network: trying the change for 120 s
network: Spark undoes it at 14:32:05 unless it is kept: `network keep`
$ elevate network keep
network: kept: the network settings stay as they are
```

# JSON

With *--json*, one line with an object of two fields, in this order:

*cards*
	An array with an object for each card.

*trial*
	*{"left_ms": N}*, the time in milliseconds before the *network*
	trial is undone unless it is kept (*spark*(8)), or *null* when the
	settings are not being tried, or Spark does not answer.

A card has these fields, in this order:

*name*
	The card's name.

*mac*
	Its MAC address, in lower case.

*up*
	Whether its link is up.

*speed_mbps*
	Its speed in megabits a second, or *null* while the link is down or
	the card does not say.

*addresses*
	What it has now, IPv4 first: objects of *address* (without its
	prefix), *prefix* (a number) and *from* (above).

*routers*
	Its default routers, IPv4 first.

*dns*
	The name servers the machine asks, as *netd* writes them into
	_/system/live/netd/resolv.conf_. *netd* merges the servers of every card
	into one list, so the list is the same on each card. _/etc/resolv.conf_
	leads to *resolved*(8)'s own file, which names only *resolved*.

*settings*
	What *network*(5) says of the card, an object (below).

*lease*
	*{"address": A, "ends_ms": N}* for the lease the card holds, with
	the time it ends in milliseconds since 1970, or *null* for a lease that
	lasts for ever. It is *null* when the card holds no lease: it does not
	ask DHCP, it has not got one, the lease file's address is not one the
	card has, or the person may not read the file. *netd*'s files of the
	leases it holds are readable by everyone.

The settings are an object of these fields, in this order:

*section*
	The name of the section that sets the card up, as the file has it: the
	card's name or its MAC address. *null* when no section names it, and the
	card takes the defaults.

*mode*
	*dhcp* when the card asks DHCP, *fixed* when it has addresses of its
	own and does not, and *none* when *netd* leaves it as it finds it
	(*dhcp = no* and nothing of its own).

*addresses*
	The card's own addresses, IPv4 first, each as _address_*/*_prefix_.

*router*
	The card's own router: the IPv4 one, else the IPv6 one.

*dns*
	The card's own name servers.

*metered*
	*auto*, *yes* or *no*.

*dhcp_server*
	Whether the card gives addresses by DHCP (*dhcp-server*).

*other*
	The names of the section's other keys, in the file's order, each once:
	*ipv6* or *dhcp-server-lease*, for those the settings have no field for.

# FILES

_/kernel/class/net_, _/processes/net/route_
	The cards and the routes (*radix-sysfs*(5), *radix-proc*(5)).

_/system/settings/network_
	The settings (*network*(5)), which *set* writes.

_/system/state/spark/trials/network_
	Spark's copy of the settings while a change is being tried, which
	only root reads.

_/system/state/netd_
	*netd*'s lease files, a file for each card, named by its MAC address.

_/system/live/netd/resolv.conf_
	The name servers.

# EXIT STATUS

0 when the cards were shown, even none, and when a change was made, is
being tried, was kept or was undone. 1 for a card the machine does not
have (*NET-181*, with its page in the support center), and when the
kernel's list of the cards cannot be read; 1 when *set* refuses a value
or the change (*NET-176*, *NET-178*), when *keep* or *undo* finds nothing
being tried (*NET-179*), when it does not run as root, and when *netd*
did not take a change that was tried, which Spark then undid; 1 for a
mistake in the command line, which also prints its usage.

# SEE ALSO

*ip*(8), *netd*(8), *network*(5), *netcheck*(8), *firewall*(8),
*spark*(8), *service*(8), *elevate*(8), *systools*(1)
