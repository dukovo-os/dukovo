ping(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

ping - ask a machine whether it answers, and how soon

# SYNOPSIS

*ping* [*-4* | *-6*] [*-c* _count_] [*-i* _interval_] [*-W* _timeout_] [*-w* _deadline_] [*-s* _size_] [*-t* _ttl_] [*-b*] [*-q*] [*-n*] _destination_

# DESCRIPTION

ping sends ICMP echo requests to _destination_, an address or a name,
and says of each reply how long it took to come, as iputils' ping does
on Linux, with its options and what it prints. Its requests go through
a ping socket, which any program may have (ADR 0336): ping needs no
privilege. A name is looked up as any program's is (*resolved*(8)), and
a name under _.local_, as *b.local*, through *discoverd*(8). An IPv6
address, or a name with only IPv6 ones, is asked over ICMPv6.

```
$ ping -c 2 b.local
PING b.local (192.168.50.2) 56(84) bytes of data.
64 bytes from b.local (192.168.50.2): icmp_seq=1 time=0.412 ms
64 bytes from b.local (192.168.50.2): icmp_seq=2 time=0.388 ms

--- b.local ping statistics ---
2 packets transmitted, 2 received, 0% packet loss, time 1001ms
rtt min/avg/max/mdev = 0.388/0.400/0.412/0.012 ms
```

When a router on the way says a request's time to live ran out, or
that _destination_ cannot be reached, ping says which router and what,
as iputils' does, from the ICMP errors the kernel gives its socket
(IP_RECVERR, ADR 0345):

```
$ ping -c 1 -t 1 10.77.9.9
PING 10.77.9.9 (10.77.9.9) 56(84) bytes of data.
From 10.77.1.1 icmp_seq=1 Time to live exceeded
```

An error counts as an answer for *-c*: ping stops once each request has
had a reply or an error, and with *-w* at the first error.

It sends a request each second until it is interrupted, by *Ctrl-C*,
or until _count_ have gone, and then prints how many were answered, and
the least, the average and the most time a reply took, and how far they
were from the average. A reply that came twice says *(DUP!)*.

A machine on the networks the machine is on is reached with the network
capability's local right, and one beyond them, through a router, with
its send right (ADR 0337): a program in a bundle pings the printer
beside it only with the network.local permission (*bundle*(1)).

# OPTIONS

*-4*
	Only IPv4: a name's IPv4 address.

*-6*
	Only IPv6: a name's IPv6 address.

*-c* _count_
	Stop after _count_ requests, once each has been answered or
	_timeout_ has passed since the last one.

*-i* _interval_
	The seconds between requests, as a fraction too, as *0.2*; 1 unless
	it is given, and 0.002 at least.

*-W* _timeout_
	How long to wait for the replies after the last request, in seconds;
	10 unless it is given.

*-w* _deadline_
	Stop after _deadline_ seconds, however many requests were answered.

*-s* _size_
	The bytes of data each request carries, 56 unless it is given, and
	65507 at most. With 8 or more, the first 8 carry the time it went,
	from which its reply's time is told.

*-t* _ttl_
	The time to live of the requests, or for IPv6 their hop limit, from
	1 to 255.

*-b*
	Allow _destination_ to be a broadcast address.

*-q*
	Print only the first line and the statistics.

*-n*
	Changes nothing: ping gives addresses as numbers.

Not yet: *-f*, *-l*, *-p*, *-R*, *-r*, *-I* and the other options that
a ping socket does not allow or that iputils keeps for its
administrator, and the TTL of each reply, which the kernel does not give
(ADR 0336).

# EXIT STATUS

0 when a reply came, or with both *-w* and *-c*, _count_ replies; 1 when
none came, or fewer than _count_ with *-w*; 2 for an option or an
argument it does not take, a name that cannot be looked up, or any other
failure.

# SEE ALSO

*discover*(1), *discoverd*(8), *ip*(8), *mtr*(8), *resolved*(8), *ss*(8),
*systools*(1), *traceroute*(8)
