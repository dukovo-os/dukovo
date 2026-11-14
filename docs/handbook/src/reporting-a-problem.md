<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Reporting a problem

This chapter describes how to tell the Dukovo project that something went
wrong: a program that crashed, a service that stopped, hardware that does
not work, an update that failed. Dukovo keeps a record of each crash on
the machine, offers it to you once, and sends nothing until you have read
the whole report and chosen to send it. The full references are
report(1), reportd(8), report.toml(5) and radix-crashes(4); what a report
holds is in "Privacy and security reporting".

## When something crashes

A program that is ended by a signal, such as a segmentation fault, a
service that panics, and the kernel's own panic each leave a record that
the reportd service keeps. Nothing is sent. The next time
someone signs in, the sign-in shows a line for the crash, once:

```
the service web stopped at 2026-11-13 10:02: SIGSEGV (address not mapped) in
ngx_http_core_run_phases. `report crash` reports it, `report forget` puts it away.
```

(on one line). The same crash, with the same functions on its stack, is
offered once, however often it happens: the next one adds to the count
and nothing to the sign-in. The log has a warning for each crash.

Each user sees the crashes of the programs that ran as them. An
administrator sees all of them, and is the one who may put a service's log
in a report.

```
$ report list
```

shows the crashes that are kept, the reports that wait for the network
and the reports that were sent.

## Reporting a crash

```
$ report crash
```

reports the newest crash that was not reported or put away; `report crash
ID` reports the one whose ID `report list` shows. `report` with no
words asks what the report is about instead: a command or an app, a
service, the hardware, installing or updating, or something else.

It asks three questions, and each answer ends with an empty line:

- *What happened?*
- *What did you expect?*
- *How can it be made to happen again?* Press Enter to skip it.

The crash tells the project where the program stopped, and the answers
tell it what you were doing, which only you know. Then `report` shows the
whole report, exactly as it would send it, in the pager (press `q` to
leave it), and asks what to do with it. Nothing goes anywhere before you
choose:

| Key | What it does |
| --- | --- |
| `S` | Sends the report |
| `L` | Leaves out the log, when it has one, and shows the report again |
| `E` | Opens it in the editor; the changed text is cleaned again and shown again |
| `F` | Saves it to a file |
| `P` | Offers it once to a phone, as a download |
| `C` | Cancels, and sends nothing |

The report is plain text, so that you can read all of it. This is the start
of one:

```
Dukovo problem report
Version: 0.16.0 (stable, server)
About: a crash of nginx 1.29.0 (the service web)
Hardware: Dell Inc. OptiPlex 7040, Intel Core i7-6700, 16 GiB
Devices: 8086:15b8, 8086:a102, ...
Language: en_US.UTF-8

== What happened
...
== Crash
nginx (the service web) stopped: SIGSEGV (address not mapped) at 0x0
Version: nginx 1.29.0
Thread: nginx
Signature: 3fa2c4d19e0b7a61
 0 ngx_http_core_run_phases+0x4c (/system/commands/nginx +0x5a2c1c, build 7be951e4...)
```

## What the report holds

The report holds your answers, a few facts about the machine, and what the
report is about: for a crash, the program and its version, the signal and
the functions on its stack; for a service, its last 200 lines of log, which
`L` leaves out. It holds no registers, no part of the memory the program
used, and none of its files or environment.

Before you see the text, it is cleaned, line by line, your own words too.
Each piece that is cut out is replaced by a mark that the preview shows:

| What is cut out | The mark |
| --- | --- |
| The names of the accounts | `[user]` |
| The paths of their home folders | `/users/[user]` |
| The host name, and the names in the hosts file | `[host]` |
| IPv4 addresses, and IPv6 addresses (not the loopback address) | `[IPv4]`, `[IPv6]` |
| MAC addresses | `[MAC]` |
| Email addresses | `[email]` |
| The names of the Wi-Fi networks the machine knows | `[Wi-Fi]` |
| The serial numbers the machine has | `[serial]` |
| The machine's ID | `[machine]` |

The cleaning finds what the machine knows about itself. It cannot know what
is private to you in what you typed, so read the preview, and use `E` to
take out what should not go. What is sent is the text you last saw, byte
for byte.

## Saving a report, or sending it from a phone

A machine with no network, or one that you do not want to send from, can
still make the report:

```
$ report crash --save report.txt
```

writes the text to a file that only you can read, which you can attach to
an email or a post on the project's issue tracker from another computer. A
stick's folder works as any path.

```
$ report crash --save
```

with no file offers the text to a phone on the same network, once. It
prints an address, a QR code of it, and the certificate's fingerprint, and
waits for one download for 10 minutes. The firewall keeps what a program
you run listens on for this machine alone, so the phone needs a rule that
allows the port first. `--port` names the port, so that an administrator
can allow it before the download starts, and take the rule away after:

```
$ elevate firewall allow tcp 8443
$ report crash --save --port 8443
$ elevate firewall list
$ elevate firewall remove 1
```

`firewall list` numbers the rules; remove the one for the port. Then:

1. Scan the code with the phone's camera, or type the address.
2. The phone warns that it does not know the certificate: the machine made
   it for this download. Compare the fingerprint the phone shows with the
   one on the screen, and go on if they are the same.
3. The phone gets the text, and the machine stops listening. A second
   request for the address, and any other address, is refused.

The address holds a random token, so that no one else on the network
can fetch the report.

## When the network is away

A report that cannot be sent, because the machine is offline or the
server is down, waits in a queue on the machine, and `report list` says
so. reportd tries again after 1, 5, 15 and 60 minutes, and then every hour,
for 30 days. `report retry` tries at once. After 30 days the report is
dropped, and the log says so.

## Where a report stands

```
$ report status
```

asks the server about each report you sent, and says whether it was
received, confirmed, is a duplicate of another report, was fixed in a
version, or will not be fixed, and why. `report status 12` asks about one.
Only you can ask: the server tells a report's status only to the key that
reportd made with it.

```
$ report delete 12
```

asks the server to delete a report you sent, and forgets it on the machine.

## A report for the security team

```
$ report crash --security
```

encrypts the report to the keys of the security team before anything is
queued, so that only they can read it. Use it for a flaw that could be
used against other people's machines, and not for an ordinary crash. The
preview shows the plain text, which only you and the team see. Until
`security-recipients` in report.toml(5) names a file of the team's keys,
`report --security` says so and tells you where to write instead:
security@dukovo.org. "Privacy and security reporting" says how to report a
vulnerability.

## After a failed start

When an update fails its health check and Dukovo falls back, the sign-in
shows the failure notice. It offers keys:

```
Press R to report this failure, Q for its support page as a QR code, or Enter to sign in.
```

`Q` draws the QR code of the notice's support page, with its address under
it, for a phone to read; any key goes back. `R` asks for the name and
password of an administrator, since the failed start's log is an
administrator's, and shows the report of the failed start: the notice, the
changes between the starts, and the warnings and errors of the start that
failed. It has the same keys as any report, and `C` cancels. `report
failure` makes the same report from a shell.

## Putting a crash away

```
$ report forget ID
```

puts a crash away: it is not offered again, and `report crash` skips it.
A crash is also deleted by itself after 30 days.

## Turning it off

reportd keeps the records of crashes and sends nothing on its own, so there
is little to turn off. To keep no record of the programs that a signal
ends, put `crashrecords=off` on the kernel's command line (dukovo-boot(7)).
`enabled = false` in reportd's service file stops the service:

```
$ elevate service edit reportd
```

## Sending reports to another server

By default, reports go to the project's server. A company that wants its
machines' reports for itself, or a person who tests a build against a
staging server, names the server in `/system/settings/report.toml`
(report.toml(5)):

```
server = "https://reports.example.org"
ca = ["/system/settings/ssl/reports-ca.pem"]
```

`ca` lists the files of the authorities that signed the server's
certificate, when the system does not already trust them. `service reportd
restart` makes reportd read the file again.
