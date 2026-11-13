<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0482. The log goes to syslog servers from logforward, with the log's own files as its queue

- Status: Accepted
- Date: 2026-11-13

## Context

A server that a company runs keeps its logs on another machine: a
syslog server such as rsyslog or syslog-ng, or a collector that speaks
syslog, which an audit asks for and a compromised machine cannot reach
to rewrite. What that needs is a standard message (RFC 5424), a standard
transport that is checked and encrypted (RFC 5425, TLS on port 6514,
with TCP on 514 by RFC 6587 and UDP on 514 by RFC 5426 for servers that
take nothing else), and a log that is not lost while the server is
down or the network is.

netlog(8) (ADR 0196) is not that. It reads the kernel's records from
`/devices/kmsg` and sends each as one UDP datagram of plain text, in
the format netconsole has, to one target: for the boot's first seconds
and a machine that hangs. It has no TLS, no authentication and no
queue: a record sent while the target is away, or whose datagram is
dropped, is gone.

logd(8) already keeps the whole log, ordered, on the disk: a file of
JSON lines for each UTC day, appended to and never changed, and trimmed
from its oldest end by whole days (ADR 0210). That is a queue that
survives restarts and boots, and that is bounded by the same settings
that bound the log (log(5)).

## Decision

- **A service of its own, `logforward`, not a thread of logd.** logd
  never needs the network, and keeping it off the network keeps the one
  process that holds the whole log out of reach of what a server sends
  back, which a TLS stack must parse. logforward reads the day files,
  which it may read and cannot write, and so cannot change what logd
  kept. It is a command of systools (ADR 0096) that Spark starts after
  resolved (order 25), holding the network to send through, the log and
  the certificates to read, and two folders to write: its places in the
  log and its status.
- **Each record is an RFC 5424 message**, `<PRI>1 TIME HOST APP PROCID
  MSGID - MSG`: the facility is authpriv for a security event, whoever
  wrote it, else the kernel's, daemon for a service's lines and user
  for a program's; the severity is the record's level; the time is the
  record's; the application is the service's or program's name, cleaned
  to the octets RFC 5424 allows and cut to 48; the process is a
  program's; the message id is `audit` for a security event. Over TLS
  and TCP a message has its length in front (octet counting, RFC 5425
  section 4.3), and is cut at 8192 octets, rsyslog's default maximum;
  over UDP it is a datagram, cut at 2048 (RFC 5426).
- **The log's files are the queue, with a place for each destination**
  in `/system/state/logforward/NAME.position`: for each day file, the
  byte after the last whole line sent from it. One place per file, not
  one for the log, because logd appends a record to the file of the
  record's own day: a record stamped a moment before midnight that
  arrives after it, or one written after the clock was set back (a
  clock read from hardware kept in local time, corrected by the time
  server), reaches a file older than the newest. A single place would
  pass it over, or, with a clock that was ahead, wait in a file named
  for a later day while today's grew unsent. Each batch reads what each
  day file gained, oldest first. The place is written to a new file,
  synced and renamed over the old, and the folder synced, so that a
  power cut leaves the old place or the new. A new destination starts
  at the end of each day file and saves that place at once, so it is
  sent the log from then on, not the log's whole past. A line that logd
  has not finished is left until it is whole, or until its file has
  stayed unchanged a minute, which only a logd that ended in the middle
  of the line leaves. If logtrim removes a day file before all of it
  was sent, logforward says so at the warning level.
- **Delivery is at least once.** A server's kernel taking the bytes is
  not the server keeping them, and RFC 5425 has no acknowledgment. So a
  place counts as delivered only once the connection has stayed open 10
  seconds after the bytes were flushed (`SETTLE`); after a failure,
  sending starts again from the last place that counts as delivered, so
  that the server may see the last few seconds twice and never a gap.
  Over UDP, which nothing acknowledges, a place counts as delivered once
  it is sent. Places are saved at most once a second, and a save that
  fails is said once and tried again.
- **The TLS check cannot be turned off.** The server's certificate must
  chain to the certificates of the destination's `ca` file when it
  names one, and to no others, or else to the roots the system trusts,
  and must be valid for `server_name`, or else for the
  host of the address. A certificate in the `ca` file that the server
  presents as its own is trusted as it is, so that a server's
  self-signed certificate is pinned by copying it there; its name and
  dates are still checked. With `certificate` and `key`, logforward
  shows a client certificate to a server that asks for one (RFC 5425
  section 5.3). TLS is 1.2 or 1.3, on rustls with ring, as the system's
  other clients.
- **One thread for each destination**, so that a server that is down or
  slow holds up no other, and a failed connection is tried again after
  1, 2, 4 seconds and so on to 60, starting over once a connection has
  lasted 10 seconds. A failure is said once in the log until it
  changes, not at each attempt. Each destination reads up to 256 lines
  or 64 KiB at a time, and looks at the log again a second after it
  finds nothing new.
- **A destination chooses what it is sent** by level (`error`,
  `warning`, `info` or `debug`), by the names of services and programs,
  and to security events alone. A record that a destination is not sent
  still moves its place, so that the place does not stay behind a log
  that has nothing for it.
- **The settings are TOML**, one `[[destination]]` for each server, in
  `/system/settings/logforward.toml` (logforward.toml(5)), read when
  the service starts and at SIGHUP (`service logforward reload`). A
  destination with a mistake is left out, with a line in the log that
  names the file, the destination and the setting, and the others go
  on; with no file, or no destination, the service says so and waits.
  The `ca`, `certificate` and `key` files must be in
  `/system/settings/ssl`, where the system keeps its certificates and
  keys, so that a settings file cannot have logforward read another
  file as one, and only with TLS.
- **`logforward status` and `logforward test`.** Each thread writes
  `/system/live/logforward/NAME.json` (its state, since when, the last
  error, how many messages it has sent, how many bytes of the log lie
  after its place, and what the TLS handshake found), and `status`
  shows them as a table. `test` connects afresh to each destination,
  sends one test record, closes the connection and says what it found
  or why it failed. It needs root, as it reads the client key.
- **netlog stays as it is**, and is documented as what it is for: the
  boot's first seconds and a machine that hangs.

## Consequences

- A server that is away for a day, or until the next boot, gets all the
  log it missed when it is back, as long as the log's retention keeps
  those days; what logd trims before that is lost to that server, and
  the log says so.
- A destination's records reach its server in order, about a second
  after logd writes them.
- A server that ends each connection within 10 seconds of the
  messages it took is sent the same records at each attempt, as no
  connection lasts long enough for them to count as delivered. Such a
  server needs a longer time before it ends an idle connection, or UDP.
- A UDP destination loses what is sent while its server is down, as UDP
  does: no queue makes up for a datagram nobody took, and the place
  moves on at once.
- The place of a destination that the settings no longer name stays on
  the disk, so naming it again goes on from there, which can send a
  great deal at once; removing its file starts it from the log's end.
- Sending the log is sending what is in it: user names, addresses and
  what programs write. logforward sends nothing until a server is named,
  and dukovo-privacy(7) and the handbook's privacy chapter list it.
- The `log-forward` boot test runs two machines on a cable: one sends
  its log to TLS, TCP and UDP servers on the other, is refused by a TLS
  server whose certificate it does not trust, has records it logged
  while a server was away arrive in order when it is back, and is
  restarted without sending anything again that counted as delivered.
  The lab's `services/log-forward` test runs rsyslog with TLS on the
  lab PC, as an administrator's server would be, and checks that a line
  logged on the machine arrives and that a wrong `server_name` is
  refused.
- Not done, each in `docs/deferred.md`: RFC 3164's format, for old
  receivers; RELP, which acknowledges each message and so could end the
  10-second wait; structured data beyond the message id; a page in the
  web console; and receiving logs from other machines.

## Alternatives

- **A thread in logd.** It would share the log's files and positions
  without reading them again, but put a TLS client and everything a
  server sends in the process that holds the whole log, and a stuck
  connection among its threads. A service that only reads the files
  costs a second look at the files.
- **A disk queue of its own, as rsyslog can have.** A second copy of the
  log in files of its own, which must be sized, trimmed and kept across
  restarts. logd's day files are already such a queue, written once,
  trimmed by the settings the administrator already knows, and a place
  in them is a file name and a number.
- **RELP.** Each message is acknowledged by the receiver, so a place
  could count as delivered at once and nothing would be seen twice. It is
  a protocol of rsyslog and a few others, not a standard, which a SIEM
  or a managed collector often does not speak; TLS syslog is spoken
  nearly everywhere. A later destination type can add it.
- **Sending from the kernel's ring, as netlog does.** It loses what the
  ring dropped and what was sent while a server was away, which is the
  case this decision is for.
