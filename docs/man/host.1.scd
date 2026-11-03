host(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

host - look a name or an address up in the DNS, in a line each

# SYNOPSIS

*host* [*-4* | *-6*] [*-a*] [*-v*] [*-T*] [*-r*] [*-t* _type_] [*-c* _class_] [*-W* _seconds_] [*-w*] [*-R* _tries_] [*-p* _port_] _name_ [_server_]

# DESCRIPTION

host asks a name server about _name_ and says what it holds in a
sentence each, as BIND's host does: for a name, its IPv4 and IPv6
addresses and where its mail goes, and for an address, its name. It
asks the servers of _/etc/resolv.conf_ unless _server_ names
one. host is dig's other face (*dig*(1), ADR 0341).

```
$ host example.com
example.com has address 192.0.2.1
example.com has IPv6 address 2001:db8::1
example.com mail is handled by 10 mail.example.com.
$ host 192.0.2.1
1.2.0.192.in-addr.arpa domain name pointer example.com.
```

# OPTIONS

*-t* _type_
	Ask only for _type_: *A*, *AAAA*, *MX*, *TXT*, *NS*, *SOA*, *ANY* or any
	other type's name.

*-a*
	Ask for *ANY*, and show the answer as *dig*(1) does.

*-v*, *-d*
	Show each answer as *dig*(1) does.

*-c* _class_
	The class: *IN* unless it is given.

*-T*
	Ask over TCP, not UDP.

*-r*
	Do not ask the server to find the answer itself.

*-W* _seconds_
	How long to wait for each answer, 2 seconds unless it is given.

*-w*
	Wait for an answer for as long as it takes.

*-R* _tries_
	How many times to ask, 2 unless it is given.

*-p* _port_
	The server's port, 53 unless it is given.

*-4*, *-6*
	Only the server's IPv4, or IPv6, address.

# EXIT STATUS

0 when the name was found; 1 when it was not, when no server answered,
or for an option or an argument it does not take.

# SEE ALSO

*dig*(1), *resolvectl*(1), *resolved*(8), *systools*(1)
