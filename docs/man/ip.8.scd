ip(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

ip - show and set the network interfaces, their addresses and routes

# SYNOPSIS

*ip* [_options_] *link* [*show* [[*dev*] _name_]]

*ip* [_options_] *link set* [*dev*] _name_ *up*

*ip link add* _name_ *type bridge*

*ip link add link* _parent_ *name* _name_ *type vlan id* _number_

*ip link set* [*dev*] _name_ {*master* _bridge_ | *nomaster*}

*ip link del* [*dev*] _name_

*ip* [_options_] *address* [*show* [[*dev*] _name_]]

*ip address* {*add* | *del*} _address_[/_prefix_] *dev* _name_

*ip address flush dev* _name_

*ip route* [*show* [*dev* _name_]]

*ip route* {*add* | *replace*} *default via* _address_ [*dev* _name_]

*ip route del default* [*via* _address_] [*dev* _name_]

*ip -6* {*address* | *route*} ...

# DESCRIPTION

ip shows the network interfaces, their IPv4 and IPv6 addresses and their
routes, and sets them, with the words and output of iproute2's ip on
Linux for what Dukovo has (ADRs 0190, 0332 and 0333). An object or a command may be cut to its
first letters, as iproute2 takes them: *ip a* is *ip address show*, and
*ip r* is *ip route show*. With no command, an object is shown.

The interfaces are *lo*, numbered 1, and the network cards, *net0*,
*net1*... from 2, in the order the kernel found them, then the bridges
and VLANs made with *link add*. ip reads them from
_/kernel/class/net_ (*radix-sysfs*(5)) and the routes from
_/processes/net/route_ (*radix-proc*(5)), which anyone may.

Each card has up to four addresses, the first the one it sends from, and
at most one router, its default route; the route to each address's
network comes with the address. ip changes them through the network
capability (*radix-network*(2)): it reads what the interface has, changes
that and gives it the whole back. Only a program given *admin.network*
by its service (*spark*(8)) may; root's shells at the console are
(*consoled*(8)).

# OBJECTS AND COMMANDS

*link* [*show* [[*dev*] _name_]]
	Each interface, or the one named: its number, name, flags, MTU and
	state, then its MAC address and the broadcast one. The flags are
	Linux's: *LOOPBACK* for *lo*, else *BROADCAST*; *UP*, as every
	interface always is; and *LOWER\_UP* while its link is, else
	*NO-CARRIER*. The state is *UP*, *DOWN* for a card the kernel
	stopped after an error, or *UNKNOWN* for *lo*.

```
2: net0: <BROADCAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DEFAULT group default
    link/ether 52:54:00:12:34:56 brd ff:ff:ff:ff:ff:ff
```

*link set* [*dev*] _name_ *up*
	Does nothing: the interfaces are always up. *down* fails, as taking
	one down is not there yet.

*link add* _name_ *type bridge*
	Makes the bridge _name_, which joins its ports into one network: a
	frame a port receives goes on to the port its destination was last
	heard on, or to every other, and to the bridge's own addresses. It
	takes its first port's MAC address. netd(8) sets it up as a card.

*link add link* _parent_ *name* _name_ *type vlan id* _number_
	Makes the 802.1Q VLAN _name_, number 1 to 4094, on _parent_, a card
	or a bridge: what it sends goes out of _parent_ tagged, and what
	_parent_ receives tagged for it comes to it. *ip link* shows it as
	_name_*@*_parent_.

*link set* [*dev*] _name_ *master* _bridge_
	Makes _name_ a port of _bridge_; it has no addresses of its own
	while it is one, as on Linux, and *ip link* shows *master* _bridge_
	on it. *nomaster* takes it out again. *ip link show master* _bridge_
	shows the bridge's ports.

*link del* [*dev*] _name_
	Takes the bridge or VLAN _name_ away; a bridge's ports become cards
	of their own again. A card cannot be taken away.

*address* [*show* [[*dev*] _name_]]
	As *link*, with each IPv4 address, its network's broadcast address
	and its scope: *host* on *lo*, else *global*; an address in the
	network of one before it is a *secondary* one. _lo_ shows
	127.0.0.1/8 and ::1/128 only: the cards' addresses it answers are
	theirs. Then each IPv6 address, from the kernel's _ipv6_ file of the
	interface: its scope (*link*, *site* or *global*) and iproute2's
	flags, *temporary* for a temporary address (RFC 8981), *dynamic*
	for one made from a router's advert, *stable-privacy* for one made
	with the machine's secret (RFC 7217), *tentative* while it is
	tested, *deprecated* once it is no longer preferred; and the
	lifetimes left (ADR 0331).

```
    inet 10.0.2.15/24 brd 10.0.2.255 scope global net0
       valid_lft forever preferred_lft forever
    inet6 fec0::5d1c:a2f0:3b44:91e7/64 scope site temporary dynamic
       valid_lft 86393sec preferred_lft 14393sec
```

*address add* _address_[/_prefix_] *dev* _name_
	Gives the card one more address, after those it has; without a
	prefix, a /32, or a /128 for IPv6. An IPv6 address is tested before
	it is used, and shows *tentative* until it is. *local* may come before the address, and *brd +* or
	the network's own broadcast address after it, as scripts write
	them; another broadcast address is refused, as the kernel takes the
	network's.

*address del* _address_[/_prefix_] *dev* _name_
	Takes the address from the card; without a prefix, whatever its
	prefix. The router goes with the last address whose network
	reaches it, as Linux's routes through it do.

*address flush dev* _name_
	Takes all the card's addresses, and its router.

*route* [*show* [*dev* _name_]]
	The default routes, then each address's network, as Linux's main
	table lists them; _lo_'s, in Linux's local table, are not shown.
	With *-6*, the IPv6 networks and routers: those routers advertise
	and those given.

```
default via 10.0.2.2 dev net0
10.0.2.0/24 dev net0 proto kernel scope link src 10.0.2.15
```

*route add default via* _address_ [*dev* _name_]
	Gives the card whose network reaches _address_, or the one named,
	_address_ as its router. *0.0.0.0/0* is *default*. Other routes are
	not set this way: each network's comes with its address.

*route replace default via* _address_ [*dev* _name_]
	As *add*, in place of the router it has.

*route del default* [*via* _address_] [*dev* _name_]
	Takes the card's router, if it is _address_ when one is given.

# OPTIONS

*-br*, *-brief*
	A line for each interface: its name, state and MAC address and
	flags for *link*, or its addresses for *address*.

*-s*, *-stats*, *-statistics*
	With *link*, what each interface received and sent: bytes,
	packets, errors and drops.

*-c*, *-color*[*=always*|*auto*|*never*]
	Interface names, states, addresses and MAC addresses in colour,
	always (the default) or with *auto* only on a terminal and not
	with *NO_COLOR* set. *IP_COLORS* changes the colours, as
	_name_*=*_SGR_ pairs joined by *:*, with the names *name*, *up*,
	*down*, *address* and *mac*; *nukesh*(1) sets it from
	*colors.toml*(5), and its alias gives *-color=auto*.

*-4*, *-6*
	Only IPv4, or only IPv6: the addresses of that version, and its
	routes.

*-d*, *-details*
	With *link*, what each interface is: *bridge*, or *vlan protocol
	802.1Q id* _number_.

*-h*, *-help*
	The commands, briefly.

Not yet: the other objects (*neigh*, *rule*, *netns*...), *-j*,
routes to networks of their own, *ip route get*, *ip link set* _mtu_ or
_down_, and links of other types (*bond*, *veth*, *wireguard*...).

# EXIT STATUS

0 when it showed or changed what it was asked; 1 for a mistake in the
command line, or an interface that is not there; 2 when the change was
refused, after *RTNETLINK answers:* and why, as iproute2 says it: *File
exists* for an address or router the card has, *Cannot assign requested
address* for an address it does not, *Network is unreachable* for a
router no card's network reaches, and *Operation not permitted* without
*admin.network*.

# EXAMPLES

```
# ip address add 192.168.1.20/24 dev net0
# ip route replace default via 192.168.1.1
# ip -br address
lo               UNKNOWN        127.0.0.1/8 ::1/128
net0             UP             192.168.1.20/24 fe80::4c1e:9a2:d07:113b/64
# ip link add br0 type bridge
# ip link set net0 master br0
# ip link add link br0 name br0.7 type vlan id 7
```

# SEE ALSO

*ss*(8), *colors.toml*(5), *radix-network*(2), *radix-proc*(5),
*radix-sysfs*(5), *spark*(8), *systools*(1)
