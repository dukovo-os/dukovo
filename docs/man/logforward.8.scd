logforward(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

logforward - send the system's log to syslog servers

# SYNOPSIS

*logforward run*

*logforward status* [_name_]

*logforward test* [_name_]

# DESCRIPTION

logforward sends the system's log, as *logd*(8) keeps it, to syslog
servers (ADR 0482): each record as an RFC 5424 message, over TLS
(RFC 5425), TCP (RFC 6587) or UDP (RFC 5426), to each server that
_/system/settings/logforward.toml_ names (*logforward.toml*(5)). Spark
runs *logforward run* as the *logforward* service, after *resolved*(8)
(*spark*(8)). It sends nothing until a server is named.

A server that cannot be reached loses nothing. The log's day files,
which logd appends to, are the queue: logforward keeps a place in them
for each server, on the disk, and sends from there once the server is
back, across restarts and boots. It reads the files and never writes
them, and it is a service of its own so that logd, which holds the whole
log, stays off the network.

*netlog*(8) sends the kernel's records over UDP as they are written,
from the first seconds of the boot, for a machine whose screen shows
nothing or that hangs before its log is saved, and loses what it sends
while nothing listens. logforward is for the log of a running machine
that is kept: everything logd keeps, over a connection that is checked
and encrypted, and sent again to a server that missed it.

# COMMANDS

*logforward run*
	The service. It reads the settings, starts a thread for each
	destination, so that a server that is down holds up no other, and
	goes on until it is stopped. With no destination it says so in the
	log and waits for *service logforward reload*.

*logforward status* [_name_]
	Where each destination stands, or the destination _name_:

	```
	NAME        ADDRESS                STATE      SINCE                SENT  BEHIND
	siem        logs.example.com:6514  connected  2026-11-13 10:30:00  1204  2.0K
	backup-box  10.0.0.9:514           waiting    2026-11-13 09:00:05  0     0
	siem: TLS 1.3, certificate for logs.example.com, issued by Example CA
	backup-box: cannot connect to 10.0.0.9:514: Connection refused
	```

	*STATE* is *connected*, *connecting*, or *waiting* between a failed
	attempt and the next; *SINCE* is when it began, in UTC. *SENT*
	is how many messages have been sent since the service started, and
	*BEHIND* how much of the log lies after the destination's place in
	it. A line after the table gives each error, and what the TLS
	handshake found for each destination that is connected over TLS.
	It exits with 1 when no destination is called _name_.

*logforward test* [_name_]
	Connects to each destination, or _name_, afresh, sends one test
	record and ends the connection, and says what it found, or why it
	failed:

	```
	siem: sent a test record to logs.example.com:6514 over TLS 1.3 (certificate for logs.example.com, issued by Example CA)
	backup-box: sent a test record to 10.0.0.9:514 over UDP
	```

	Over TCP it waits a moment to see whether the server closes the
	connection; over UDP nothing says whether the datagram arrived.
	The test record is a message of the *user* facility and the
	*notice* level from the application *logforward*, which says it
	is a test record from this machine to the destination. It also
	repeats what is wrong in the settings. It reads the client key of
	a destination that has one, so only root runs it: *elevate
	logforward test* (*elevate*(8)). It exits with 1 if any
	destination failed.

# MESSAGES

A record becomes a message of RFC 5424's form:

```
<PRI>1 TIME HOST APP PROCID MSGID - MSG
```

*PRI* is the facility times 8 plus the severity. The facility is
*authpriv* (10) for a security event, whatever wrote it (*logd*(8)),
else *kern* (0) for the kernel's records, *daemon* (3) for a service's
lines and *user* (1) for a program's. The severity is the record's
level: 3 for *error*, 4 for *warning*, 6 for *info* and 7 for *debug*.
*TIME* is the record's time, as logd has it: RFC 3339, in UTC, to the
microsecond. *HOST* is the machine's name, and *APP* the name of the
service or program that wrote the record, or *kernel*, each with its
octets outside the printable ASCII range replaced by an underscore and
cut to what RFC 5424 allows, 255 and 48; a dash stands for a field with
nothing.
*PROCID* is the process that wrote a program's record, else a dash.
*MSGID* is *audit* for a security event, else a dash, and the structured
data is a dash: there is none.
*MSG* is the record's text.

Over TLS and TCP each message has its length in octets and a space in
front of it (octet counting, RFC 5425 section 4.3), and goes on a
connection that stays open; a message is cut at 8192 octets, at the end
of a character. Over UDP a message is a datagram, cut at 2048 octets.

A destination's settings choose the records it is sent by level, by
name and to security events alone (*logforward.toml*(5)). The position
of a destination moves past the records it is not sent as well.

# DELIVERY

Delivery is at least once. A server's kernel taking the bytes of a
message is not the server keeping it, and RFC 5425 has no
acknowledgment. So a record counts as delivered only once the connection
has stayed open for 10 seconds after it was sent. A connection that
fails before that is made again, and sending starts again from the last
record that counts as delivered: the server may see the last few seconds
twice, never a gap. A server that takes a connection and ends it within
10 seconds is sent the same records at each attempt. Over UDP, which
nothing acknowledges, a record counts as delivered once it is sent.

A destination's place in the log is a byte in each day file, after the
last whole line sent from it: logd appends a record to the file of the
record's own day, so a record stamped a moment before midnight, or
written after the clock was set back, reaches a file that an earlier
day began, and is sent all the same. The place is saved
in _/system/state/logforward_ at most once a second while it moves, by a
file written, synced and renamed over the old one, so that a power cut
leaves the old place or the new. A destination with no place yet,
because it is new, is sent the log from when its thread starts, and not
the log from before. The place of a destination that the settings stop
naming stays, so that naming it again goes on from there; removing its
file starts it from the log's end again.

A failed connection is tried again after 1 second, then 2, 4, and so on
up to 60 seconds, and starts again at 1 once a connection has lasted
10 seconds. Each failure is said once in the log, not at each attempt, and
a connection made says so. A day file that *logtrim*(8) removes while a
server is away is lost to it (*log*(5)); logforward says which, at the
*warning* level, when the file goes before all of it was sent. A line of
the log that is not a record is passed over, and counted in one line of
the log for each file.

Over TLS, which is TLS 1.2 or 1.3, the server's certificate must chain
to a certificate in the destination's *ca* file when it names one, and
to no other, or else to the root certificates the system trusts
(*dukovo-certificates*(7)); it must be valid, in name and dates, for
*server_name* or else for the host in *address*. A certificate in the
*ca* file that the server presents as its own is trusted as it is,
whoever signed it, so that copying a server's self-signed certificate
there pins it. No setting turns the check off. With *certificate* and
*key*, logforward shows that certificate to a server that asks for one,
for mutual TLS (RFC 5425 section 5.3). A connection that fails says
which step failed and why, in *status*, *test* and the log.

# SIGNALS

*SIGHUP*
	Read the settings again: *service logforward reload*
	(*service*(8)) sends it. The destinations' threads stop, each
	saves its place, and the ones the settings now name start.

*SIGTERM*
	Save each place that counts as delivered, close the connections
	and end, after 5 seconds at most.

# FILES

_/system/settings/logforward.toml_
	The destinations (*logforward.toml*(5)).

_/system/state/log_
	The log's day files, which logforward reads.

_/system/state/logforward/_name_.position_
	A destination's place in the log: a first line *logforward
	position 1*, then a line for each day file, its name and the byte
	after the last record from it that counts as delivered.

_/system/live/logforward/_name_.json_
	A destination's status, which *status* reads: JSON, written when
	the state changes and once a second after that, and removed when
	the destination stops being served. It is in the live folder, so
	a boot begins without it.

_/system/settings/ssl_
	Where the *ca*, *certificate* and *key* files of a destination are
	(*logforward.toml*(5)).

_/system/settings/spark/services/logforward.toml_
	Its service, which holds the network to send through, the log to
	read, the settings' certificates, and its two folders to write,
	beyond what every service holds (*service.toml*(5)).

# EXIT STATUS

*run* exits with 0 when it was stopped. *status* and *test* exit with 0,
or with 1 for a destination that does not exist, a test that failed, a
mistake in the command line, or when *test* is run by someone other than
root.

Not yet: RFC 3164's message format, for old receivers; RELP, which
acknowledges each message; structured data besides *MSGID*; a page in
the web console; and receiving logs from other machines.

# EXAMPLES

Send the log to a server that has a certificate from a company's
authority, on its own port, and to a machine on the local network that
takes only warnings, and check them:

```
# cat /system/settings/logforward.toml
[[destination]]
name = "siem"
address = "logs.example.com"
ca = "/system/settings/ssl/example-ca.pem"

[[destination]]
name = "backup-box"
address = "10.0.0.9"
transport = "udp"
level = "warning"

# service logforward reload
# elevate logforward test
# logforward status
```

On the receiving computer, rsyslog takes TLS on 6514 with its *imtcp*
module and the *gtls* driver, as the handbook's chapter on logs shows.

# SEE ALSO

*logforward.toml*(5), *logd*(8), *netlog*(8), *log*(1), *log*(5),
*logtrim*(8), *dukovo-certificates*(7), *dukovo-privacy*(7),
*service*(8), *spark*(8), *systools*(1)
