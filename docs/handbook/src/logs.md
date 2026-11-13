<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Logs, monitoring and troubleshooting

This chapter describes the system's log and its notifications. The full
references are logd(8), log(1), journalctl(1), logger(1), logtrim(8),
log(5), dmesg(1), logforward(8), logforward.toml(5), netlog(8),
notifyd(8), notify-send(1) and notifications(1). What the machine keeps of
its own state, and its early warnings, are in
[Metrics and early warnings](metrics.md).

## What the log holds

Everything that says something on Dukovo says it to the kernel log:
the kernel itself, the services, whose output Spark copies there line
by line, and programs that write to `/devices/kmsg`. The kernel keeps
only its newest 512 records, so the logd service keeps every record for
good, as it comes, in `/system/state/log`: one file a day, each record a
line of JSON.

Each record says when it was made, in which boot, how urgent it is
(error, warning, info or debug), and who wrote it: the kernel, a service
by its name, or a program by its name, its process number and the user
it ran as. Who wrote a record is what the kernel says, not what the
record says, so a program cannot pass its lines off as a service's, or
as another user's.

A script writes a line with `logger`: `logger -t backup "All 3 disks are
copied."` and `log backup` shows it.

## Who reads what

The log is for administrators, as on Ubuntu, where only the `adm` group
reads the whole journal and `dmesg` needs root. Everyone reads the
records of their own programs:

| Who | `log` and `journalctl` | `log --audit`, `log --disk-usage`, `logtrim` | `dmesg` |
| --- | --- | --- | --- |
| Anyone | The records of their own programs | No | No |
| An administrator, with `elevate` | Every record | Yes | Yes |

So an administrator types `elevate log -p error` or `elevate dmesg`.
Without `elevate`, `log` adds a hint on its standard error that it shows
only your own programs' records; `-q` leaves it out. Anyone can still
write to the log.

## Reading it

`log` shows this boot's records, oldest first:

```
$ log
Oct 30 21:25:01 dukovo kernel: Radix 0.15.0 on 4 CPUs
Oct 30 21:25:02 dukovo spark: started volumed (/system/commands/volumed) as process 14
Oct 30 21:25:02 dukovo volumed: watching for volumes; 5 disks and partitions now
```

Narrow it down with options, which all must hold together:

| To see | Type |
| --- | --- |
| One service's or program's records | `log volumed`, or `log -s volumed -s consoled` |
| A service's records with Spark's lines about it: its starts, readiness, failures and stops | `log -s nginx`, or `service nginx logs` |
| Errors only, or warnings and errors | `log -p error`, `log -p warning` |
| The last few records | `log -n 20` |
| New records as they come | `log -f` (Ctrl+C stops) |
| A time range | `log --since "2026-10-30 08:00" --until -1h`, `log --since today` |
| Records whose text matches | `log -g "disk|volume"` |
| The kernel's records alone | `log -k` |
| The boot before this one | `log -b -1`; `log --list-boots` lists them |
| Every boot | `log --all` |
| The records as JSON, for scripts and `jq` | `log --json` |

`journalctl` takes systemd's options, so what you type on Linux works:
`journalctl -u volumed.service -b`, `journalctl -k`, `journalctl -f`,
`journalctl -p err --since today`. `elevate dmesg` still reads the
kernel's ring directly, and `dmesg -k` or `dmesg -u` shows only the
kernel's records or only the programs'.

## The audit log

Security events are kept twice: in the log, and in the audit log, which
keeps them for a year in `/system/state/log/audit`. Today they are
Spark's answers to requests that change something (a service started,
stopped or restarted, the machine powered off), each with the process
that asked; the power button; the kernel log cleared; records the
kernel's ring lost before logd read them; the log trimmed; accounts
made, passwords changed and wrong passwords; signing in and out; and
each command run with `elevate`, and each refusal. Permissions will be,
as they come.

```
$ elevate log --audit
Oct 30 21:40:12 dukovo spark: asked to restart volumed, by process 57
```

Only the kernel and Spark can put an event in the audit log. A service
writes one by starting a line of its output with `<86>`, the syslog
priority of a security event, and Spark passes it on as that service's;
a program that writes the same to the kernel log only adds a line to
the log. `journalctl _TRANSPORT=audit` reads the audit log too.

## How much space it takes

`log --disk-usage` says how much the log takes, and the audit log's
share. logd keeps 90 days of the log and a year of the audit log, in at
most 4 GiB or a tenth of their file system, whichever is less, and
removes the oldest days first, each removal a security event. Change
the limits in `/system/settings/log` and run `service logd reload`;
0 lifts a limit:

```
keep-days = 30
max-size = 1G
```

`logtrim` trims now (`logtrim -n` says what it would remove), and
`logtrim --size 500M` or `logtrim --days 30` once to a limit of your
own, as `journalctl --vacuum-size=500M` and `--vacuum-time=30d` do. The
audit log keeps to its own setting.

## Sending the log to a syslog server

Companies usually keep the logs of all their machines on one syslog
server, such as rsyslog, syslog-ng or a SIEM. The `logforward` service
sends the whole log to the servers you name, as it is written, as
RFC 5424 messages: over TLS, the standard way, on port 6514, or over
plain TCP or UDP on port 514. It sends nothing until you name a server.
It is for the log of a machine that runs and keeps it: everything logd
has, sent again to a server that missed it. `netlog` (netlog(8)) is for
the boot's first seconds and a machine that hangs: it sends the kernel's
records over UDP as they come, and keeps nothing.

Name each one in `/system/settings/logforward.toml`
(logforward.toml(5)), and put the certificate of the authority that
signed the server's in `/system/settings/ssl`, where logforward takes
certificates and keys from. Without `ca`, the server's certificate must
come from one of the authorities the system trusts
(dukovo-certificates(7)):

```
[[destination]]
name = "siem"
address = "logs.example.com"
ca = "/system/settings/ssl/example-ca.pem"
```

`service logforward reload` reads the file, and `logforward test` sends
a test record to each server and says what it found, or why it could
not. Only root runs it, so type `elevate logforward test`:

```
$ elevate service logforward reload
$ elevate logforward test
siem: sent a test record to logs.example.com:6514 over TLS 1.3 (certificate for logs.example.com, issued by Example CA)
$ logforward status
NAME  ADDRESS                STATE      SINCE                SENT  BEHIND
siem  logs.example.com:6514  connected  2026-11-13 10:30:00  1204  0
siem: TLS 1.3, certificate for logs.example.com, issued by Example CA
```

The server's certificate is always checked, for its authority, its name
and its dates, and no setting turns that off: a server that fails says
why, as `siem: TLS with logs.example.com:6514: its certificate is not
from an authority in /system/settings/ssl/example-ca.pem`. A server
whose certificate is self-signed is trusted by putting that certificate
in `ca`. `server_name` names what the certificate is valid for when
`address` is not that name, and `certificate` and `key` give a client
certificate, for a server that wants the machine to prove who it is.

A destination sends every record but debugging ones. `level` sets the
least urgent it sends (`error`, `warning`, `info` or `debug`), `names`
limits it to some services and programs, such as `["sshd", "kernel"]`,
and `audit = true` sends only the security events, so that one server
can have the audit log alone. `transport = "tcp"` or `"udp"` sends
without TLS, for a server on a network you trust with the log.

While a server is down, nothing is lost. The log's files are the queue:
logforward keeps its place in them for each server, on the disk, and
sends what the server missed once it is back, also after the machine
has restarted or booted. A server that is new is sent the log from when
you name it. Because nothing over TLS or TCP says that the server has
a message, a message counts as delivered only after the connection has
stayed open for 10 seconds, so after a failure the server may see the
last few seconds twice, never a gap. Days of the log that are trimmed
(How much space it takes) before the server comes back are lost to it,
and `log logforward` says which. `logforward status` shows each
server, its state, how many messages it has been sent and how far
behind the log it is, and `log logforward` shows what the service said
about it.

The receiving side takes RFC 5424 as it comes, with octet counting.
On Debian and Ubuntu, rsyslog needs the `rsyslog-gnutls` package for TLS,
and this in `/etc/rsyslog.d/dukovo.conf` takes it on 6514 with a
certificate and key of its own, which the authority above signed:

```
global(
  DefaultNetstreamDriver="gtls"
  DefaultNetstreamDriverCAFile="/etc/rsyslog.d/example-ca.pem"
  DefaultNetstreamDriverCertFile="/etc/rsyslog.d/logs.example.com.pem"
  DefaultNetstreamDriverKeyFile="/etc/rsyslog.d/logs.example.com.key"
)
module(load="imtcp" StreamDriver.Name="gtls" StreamDriver.Mode="1"
       StreamDriver.AuthMode="anon")
input(type="imtcp" port="6514")
```

For TCP or UDP, `module(load="imtcp")` with `input(type="imtcp"
port="514")`, or `module(load="imudp")` with `input(type="imudp"
port="514")`, takes them.

## Notifications

A program or a script can tell whoever is at the screen something with
`notify-send`, as on Linux; the console shows it on its bottom line for
8 seconds, after the name of the program that sent it:

```
$ notify-send -a backup 'Backup done' 'All 3 disks are copied.'
```

```
 notify-send (backup): Backup done - All 3 disks are copied.
```

`-t` sets the time in milliseconds, and `-u critical` keeps it, in
red, until you press a key. The name in front is the one the kernel
gives, so a program cannot pass a notification off as another's; a
name it gives itself with `-a` comes after it. `notifications` lists
the last 64, and `notifications -f` follows them. The notifyd service
keeps them in memory only, never on the disk or in the log, and
refuses more than 10 in 10 seconds from one program.

## When a service does not run

`service NAME status` says what Spark knows of a service: whether it
runs, and if not, why, in one line. The `Why` line names the service it
waits for or the failure that stopped it:

| `Why` says | What to do |
| --- | --- |
| it requires NAME, which is stopped | Start that one: `elevate service NAME start` starts both |
| it requires NAME, which has no service file | The service file names a service that is not installed, or is misspelt |
| it did not get ready in 90 s: nothing listened on port 8080 | The service started but never listened: read its log, `elevate log NAME` |
| it failed to start 5 times in a row (the last: ...) | Spark gave up on it; fix what the last failure names, then `elevate service NAME start` |
| it waits for itself, through a and b | Two services wait for each other: change one of their files |

`Health` shows its health check's last result, with the last line a
failed check wrote, and `Last` how its process last ended. Spark's own
lines about it are in the log:

```
$ elevate log | grep 'spark: web'
spark: web failed its health check (exited with code 0x7: connection refused)
spark: web failed its health check 3 times in a row; restarting it
```

service.toml(5) explains each of these, and spark(8) every line Spark
writes.

## When logd is not running

`log` and `journalctl` read through logd. If it has stopped, `service
logd status` says why, `service logd start` starts it again, and
`elevate dmesg` still shows the kernel's newest records. Started again, logd goes on
from the last record it kept, so nothing is kept twice.
