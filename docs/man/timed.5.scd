timed(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

timed - which time servers timed asks, and the time it serves

# SYNOPSIS

_/system/settings/timed_

# DESCRIPTION

*timed*(8) sets the clock by the time servers this file names (ADR
0194). The default file says nothing but comments, so timed asks
Cloudflare's, Netnod's and PTB's servers over Network Time Security,
and _pool.ntp.org_ over plain NTP only while none of them answers,
and serves no time to others. *service timed reload* has timed read
the file again.

Each line is _setting_ *=* _value_. A line or the end of one that starts
with *#* is a comment.

# SETTINGS

*servers =* _host_[*:*_port_]...
	The Network Time Security servers to ask, in turn, by the name of
	their key establishment server, whose certificate must carry it,
	and its port, 4460 unless it is given; an IPv6 address goes in
	brackets when it has a port. *servers =* with nothing after it asks
	none. The default: *time.cloudflare.com nts.netnod.se
	ptbtime1.ptb.de*.

*fallback =* _host_[*:*_port_]...
	The plain NTP servers to ask only while no NTS server answers, on
	port 123 unless it is given. Their time is not authenticated: a
	machine on the way could change it. *fallback =* with nothing after
	it asks none, so that the clock is only set by authenticated time.
	The default: *pool.ntp.org*.

*certificates =* _file_...
	More authorities' certificates to trust, PEM files, beside the
	machine's (*dukovo-certificates*(7)): for an NTS server of a
	network's own, whose certificate a private authority signed.

# SERVING TIME

With *serve*, timed serves the time it keeps to the machine's networks
(ADR 0487): over NTS on TCP port 4460 and UDP port 123, to chrony,
NTPsec and other Dukovo machines, and over plain NTP too if asked, to
printers, cameras and switches that cannot do NTS. It serves the time
as one stratum further from the reference clock than the server it last
set the clock by. Three hours after its last sync, or before its first,
it says it is not synchronized, and clients do not take its time.
*netd*(8)'s DHCP server gives clients this machine as their time server
while it serves them plain NTP (*network*(5)), which they ask in.

*serve =* *off* | *nts* | *nts+ntp*
	What it serves: nothing, the default; NTS alone; or NTS and plain
	NTP. Plain NTP is not authenticated, so a machine on the way could
	change the time it carries; give it only to devices that cannot do
	NTS.

*serve-certificate =* _file_, *serve-key =* _file_
	For *nts* and *nts+ntp*, which need both: the PEM files of the
	certificate chain NTS key establishment is served with and of its
	private key, files in _/system/settings/ssl_. Clients check the
	certificate as they would a web server's, so it must carry the
	name they ask for. *service timed reload* serves a renewed one.

*serve-allow =* *any* | _network_...
	The clients answered, as networks such as *192.168.1.0/24* or
	*fd00::/8*, or one address alone, with commas or spaces between
	them; or *any*. The default: the networks of the machine's own
	cards.

*serve-unsynced =* *yes* | *no*
	For a network with no other time: *yes* serves the machine's own
	clock at stratum 10 while it has no server to sync from, as chrony's
	*local stratum 10* does. The default is *no*.

Each client address may have eight answers at once, and one more every
two seconds; past that, NTS requests are not answered, and plain ones
are told to slow down (the kiss code *RATE*). A cookie is good for two
days at least, until timed restarts: a client then runs key
establishment again, as NTS has it do. *timedatectl*(1) shows how many
answers it gave.

# MISTAKES

timed says what is wrong in the file in the system's log, with the
line's number, and leaves out that line: a setting it does not know,
a line with no *=*, a value a setting does not take, or a certificate
or key outside _/system/settings/ssl_. *serve* without both
*serve-certificate* and *serve-key* serves nothing, and says so.

# EXAMPLES

Netnod's servers only, and no plain NTP:

```
servers = nts.netnod.se sth1.nts.netnod.se
fallback =
```

A network's own NTS server, whose certificate its own authority signed:

```
servers = time.example.lan:4460
certificates = /system/settings/example-ca.pem
fallback =
```

The machine as its network's time server, over NTS and plain NTP, with
a certificate for *time.example.lan*:

```
serve = nts+ntp
serve-certificate = /system/settings/ssl/time.example.lan.pem
serve-key = /system/settings/ssl/time.example.lan.key
```

# FILES

_/system/settings/timed_
	This file; Spark copies the default one there when there is none
	(*spark*(8)).

# SEE ALSO

*timed*(8), *timedatectl*(1), *network*(5), *dukovo-certificates*(7),
*dukovo-privacy*(7)
