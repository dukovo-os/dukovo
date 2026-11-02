ss(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

ss - list the TCP and UDP sockets

# SYNOPSIS

*ss* [*-t*] [*-u*] [*-4*] [*-6*] [*-l* | *-a*] [*-n*] [*-H*]

# DESCRIPTION

ss lists the TCP and UDP sockets, from _/processes/net/tcp_ and
_/processes/net/udp_, and _tcp6_ and _udp6_ for IPv6 ones
(*radix-proc*(5), ADR 0332), with the columns of iproute2's ss on Linux
(ADR 0190): each socket's state, its queues, and its two ends. A socket
listening on all of an interface's addresses shows *0.0.0.0*, or *[::]*
for IPv6, whose addresses are in brackets, and an end not connected,
*\**.

By default ss lists the connections: the TCP sockets neither listening
nor ending in *TIME-WAIT*, nor half open in *SYN-RECV*, and the UDP
sockets that are connected. With both TCP and UDP, the first column
names each socket's protocol.

```
$ ss -tl
State  Recv-Q Send-Q Local Address:Port Peer Address:Port
LISTEN 0      0            0.0.0.0:22        0.0.0.0:*
LISTEN 0      0               [::]:8080         [::]:*
```

Recv-Q and Send-Q are the bytes received and not yet read, and those
written and not yet acknowledged.

# OPTIONS

*-t*, *--tcp*
	The TCP sockets.

*-u*, *--udp*
	The UDP sockets. Without *-t* or *-u*, both.

*-l*, *--listening*
	The TCP sockets that listen, and the UDP sockets that are bound
	and not connected (*UNCONN*).

*-a*, *--all*
	Every socket, listening, connected or ending.

*-H*, *--no-header*
	No header line.

*-4*, *--ipv4*
	The IPv4 sockets.

*-6*, *--ipv6*
	The IPv6 sockets. Without *-4* or *-6*, both.

*-n*, *--numeric*
	Changes nothing: ss gives addresses and ports as numbers.

Not yet: *-p*, the process that holds each socket, which the kernel's
lists do not name; *-x*, the Unix sockets; *-s*; *-e*; *-i*; and the
filters after the options.

# EXIT STATUS

0 when the sockets were listed, even none; 1 when the kernel's lists
cannot be read, or for an option it does not know.

# SEE ALSO

*ip*(8), *radix-proc*(5), *systools*(1)
