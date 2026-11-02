discoverd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

discoverd - the machine's name on the local networks, and what they offer

# SYNOPSIS

*discoverd*

# DESCRIPTION

discoverd speaks multicast DNS (RFC 6762) and DNS-Based Service
Discovery (RFC 6763) on each network card (ADR 0338), as Avahi does on
Linux. Spark runs it as the *discoverd* service, after *netd*(8) and
*resolved*(8) (*spark*(8)).

It answers for _NAME.local_, _NAME_ the machine's name (*hostname*(5))
as a DNS label: its first part, in lower case, with a hyphen for each
character other than a letter or a digit, *dukovo* if nothing is left.
A question that comes by a card is answered with that card's addresses;
it speaks on every card but *lo*.
It also offers _NAME.\_workstation.\_tcp_, on port 9, as Avahi's
*publish-workstation* does, so that the network browsers of other
machines list this one.

It asks for _.local_ names for *resolved*(8), which any program's
*getaddrinfo*(3) asks, so that *ping b.local* works, and for the
services the other machines offer, for *discover*(1). It also sends
WS-Discovery's Probe to 239.255.255.250, port 3702, which Windows
machines, printers and scanners answer, and reads their ProbeMatches.

The name is read when discoverd starts: after *hostname*(1) names the
machine anew, *service discoverd restart* announces the new one.

## Its socket

Programs ask discoverd through _/system/live/discoverd/socket_, which
anyone on the machine may connect to; a bundle sees discoverd's folder
only with the network.local permission (*bundle*(1), ADR 0337). Each
connection takes one request, a line, and discoverd closes it once it
has answered:

*resolve* _name_
	A line for each address of _name_, a name under *.local*, IPv4
	first, once a machine answers, or none if none does within five
	seconds. This machine's own name is answered with the
	addresses of its cards. A link-local IPv6 address is left out, as
	a DNS answer cannot say its card.

*browse* [_seconds_ [_type_...]]
	For that long, three seconds unless it is given and thirty at
	most, a line for each service
	found of each _type_, as *\_ipp.\_tcp*, or without one, of each type
	the networks' machines say they offer
	(_\_services.\_dns-sd.\_udp.local_): *service*, then its _name_,
	_type_, _host_, _port_, _addresses_ and _txt_, by tabs. Then a line
	for each machine that answered WS-Discovery's Probe: *device*, then
	its _endpoint_, _types_, _from_ and _urls_, by tabs.

	A service's _type_ is without *.local*, and its _addresses_ are by
	commas, IPv4 first, without link-local IPv6 ones; _txt_ is its TXT record, as _key_*=*_value_
	by semicolons. A device's _endpoint_ is the URN it keeps, its
	_types_ are as it gives them (*wsdp:Device pub:Computer*), _from_
	is the address its answer came from, and _urls_ are where it
	describes itself, by spaces.

Thirty-two connections are answered at once; past that, a connection is
closed without an answer.

Its service holds port 5353, which other machines' questions come to,
and its own folder to write, beyond what every service holds
(*service.toml*(5)).

# SIGNALS

*SIGTERM*
	End.

# FILES

_/system/settings/hostname_
	The machine's name (*hostname*(5)).

_/system/live/discoverd/socket_
	Where programs ask it.

_/system/settings/spark/services/discoverd.toml_
	Its service (*service.toml*(5)).

# EXIT STATUS

0 when it was ended; 1 for an argument, as it takes none, or when its
multicast sockets or its socket cannot be made.

Not yet: RFC 6762's probing, which would rename one of two machines of
one name; services offered beyond the workstation; answering
WS-Discovery's Probe; and LLMNR.

# SEE ALSO

*discover*(1), *ping*(8), *hostname*(5), *resolved*(8), *netd*(8),
*bundle*(1), *service*(8), *spark*(8), *systools*(1)
