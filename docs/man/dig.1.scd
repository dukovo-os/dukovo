dig(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dig - ask a name server about a name, and show its whole answer

# SYNOPSIS

*dig* [*@*_server_] [*-p* _port_] [*-4* | *-6*] [*-t* _type_] [*-c* _class_] [*-x* _address_] [*-q* _name_] [_name_] [_type_] [_class_] [*+*_option_...]

# DESCRIPTION

dig sends one question to a name server, over UDP unless it is told
otherwise, and prints the answer as BIND's dig 9.18 does: its header,
its flags, its EDNS record, and its question, answer, authority and
additional sections, each record a line with its name, time to live,
class, type and data, then how long the answer took and from whom. It
asks the servers of _/etc/resolv.conf_ unless _@server_ names
one, which is the local resolver, *resolved*(8), on a machine that has
no other. dig is Dukovo's own, on the same DNS code as *resolved*(8), and
takes BIND's options and prints what BIND's prints, so that what reads
dig's output on Linux reads it here (ADR 0341).

```
$ dig @192.0.2.53 example.com MX +noall +answer
example.com.		3600	IN	MX	10 mail.example.com.
```

A _type_ or _class_ after a name is that question's; several names are
several questions, each asked in turn. With no name dig asks for the
root's name servers. An answer cut short over UDP is asked again over
TCP. The data of a type dig does not know is shown as RFC 3597 gives it,
*\\# 4 C0000201*.

# OPTIONS

*@*_server_
	The server to ask: an address, or a name looked up first.

*-p* _port_
	Its port, 53 unless it is given.

*-4*, *-6*
	Only the server's IPv4, or IPv6, address.

*-t* _type_
	What to ask for: *A* unless it is given, or *AAAA*, *MX*, *TXT*, *NS*,
	*SOA*, *SRV*, *CAA*, *ANY*, any other type's name, or *TYPE*_n_.

*-c* _class_
	The class: *IN* unless it is given, or *CH* or *HS*.

*-x* _address_
	An IPv4 or IPv6 address, whose name is asked for backwards, under
	_in-addr.arpa_ or _ip6.arpa_.

*-q* _name_
	The name, even one that looks like a type or an option.

*+*[*no*]*tcp*, *+*[*no*]*vc*
	Ask over TCP, not UDP.

*+*[*no*]*short*
	Print only each answer's data.

*+*[*no*]*recurse*
	Ask the server to find the answer itself (the RD flag); on unless it
	is turned off.

*+*[*no*]*adflag*, *+*[*no*]*cdflag*
	Set the AD flag, which asks whether the answer was checked, on unless
	it is turned off, or the CD flag, which asks the server not to check
	its signatures.

*+*[*no*]*dnssec*
	Ask for the answer's signatures (EDNS's DO flag).

*+*[*no*]*edns*, *+bufsize=*_n_
	Send EDNS's OPT record, offering _n_ bytes over UDP, 1232 unless it
	is given; on unless it is turned off.

*+time=*_seconds_, *+tries=*_n_, *+retry=*_n_
	How long to wait for each answer, 5 seconds unless it is given, and
	how many times to ask, 3 unless it is given.

*+*[*no*]*cmd*, *+*[*no*]*comments*, *+*[*no*]*question*, *+*[*no*]*answer*, *+*[*no*]*authority*, *+*[*no*]*additional*, *+*[*no*]*stats*, *+*[*no*]*all*
	Print, or not, the first line, the comments, each section, the
	statistics, or all of them.

*+*[*no*]*ttlid*, *+*[*no*]*class*, *+*[*no*]*identify*
	Print, or not, each record's time to live and class, and with
	*+short* the server each answer came from.

*-h*
	Print what dig takes.

Not yet: *+trace* and *+nssearch*, which follow the delegations from the
root themselves, zone transfers, TSIG keys, and DNS over TLS or HTTPS.
*+multiline* and *+search* are taken and change nothing.

# EXIT STATUS

0 when an answer came, whatever it says, NXDOMAIN too; 1 for an option or
an argument it does not take; 9 when no server answered; 10 for a name
that cannot be asked or a server that cannot be found.

# SEE ALSO

*host*(1), *resolvectl*(1), *resolved*(5), *resolved*(8), *whois*(1),
*systools*(1)
