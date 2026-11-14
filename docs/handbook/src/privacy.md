<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Privacy and security reporting

This chapter is Dukovo's privacy promise, which the installer will show
too, and how to report a security problem. The reference is
dukovo-privacy(7).

## The promise

- **No telemetry.** Dukovo collects nothing about how you use it. No
  part of it sends usage figures, crashes, identifiers or anything else
  about you or your machine to us or to anyone else on its own. A crash
  is reported only when you choose to, read the whole report and send it
  ("Reporting a problem").
- **Nothing leaves the machine unless you start it,** but for what a
  network needs to work and the update checks, each listed below with
  how to turn it off. A program you run, a page you open or a file you
  share sends what you ask it to; Dukovo itself sends nothing on its
  own.
- **No account and no ads.** Dukovo needs no online account to install
  or use, and shows no ads.

Every service that sends something on its own is listed here, with
what it sends, where and how to turn it off, before it ships. If you
find Dukovo sending anything that is not listed, it is a bug: report it
as below.

## What stays on the machine

| What | Where | Kept |
| --- | --- | --- |
| The system's log and its audit log | `/system/state/log`, kept by logd | 90 days, and a year for the audit log, in at most 4 GiB or a tenth of their file system ("Logs, monitoring and troubleshooting") |
| The crash log | Reserved memory, shown at the next boot and kept in the log | Until the log trims it |
| Crash dumps | Reserved memory until the next boot, then `/system/state/crash`, readable by root alone | The ten newest, in at most 64 MiB ("Debugging the kernel") |
| Crash records of programs: the registers and up to 64 KiB of the stack of a program that a signal ended | The kernel's memory, read from `/devices/crashes` by reportd, which only root can do; never written to a disk | Until reportd reads them, and the 8 newest at most; `crashrecords=off` on the kernel's command line makes none |
| Crashes: what crashed, when, as which user, and the functions on the stack, with a signature of them; no registers, stack bytes or environment | `/system/state/reports/crashes`, kept by reportd, in files that root alone reads | At most 100, in 8 MiB, each for 30 days or until `report forget` |
| Reports: the number the server gave, a key that asks about it, the title and the time of each sent, and the text of each that waits for the network | `/system/state/reports`, kept by reportd | A report that waits is tried for 30 days and then dropped; `report delete` forgets a sent one |
| Notifications | notifyd's memory only | The last 64, until notifyd stops |
| The machine's history: processors, memory, disks, networks, temperatures and services | `/system/state/metrics`, kept by metricsd | A reading a minute for two weeks, and each hour's average, least and greatest for 13 months ("Metrics and early warnings") |
| Alerts | `/system/state/alerts`, kept by alertd, in files that root alone reads | The active alerts, the newest 1000 to 2000 changes, and for 24 hours at most the messages not yet delivered ("Alerts: email, ntfy and webhooks") |

A crash goes nowhere by itself. Dukovo offers to report it once, at the
next sign-in, and sends nothing until you have read the whole report and
sent it ("Reporting a problem"). A kernel dump goes into a report only as
its panic report, which is what the screen showed; to add more, copy it
into the report yourself, as "Debugging the kernel" says.

## What the machine sends on its own

Only what a network needs to work, each listed here with how to turn it
off; anything else sent on its own is a bug:

| Service | What it sends, and where | How to turn it off |
| --- | --- | --- |
| netd | On each network card, a DHCP request for an address, router and DNS servers, which carries the card's hardware (MAC) address and no host name, and a renewal at half the lease's time | Give the card an address of its own, or `dhcp = no`, in network(5) |
| resolved | Only the names programs look up, to the DNS servers DHCP gave or that you name, over DNS over TLS when they take it; `localhost` and `.local` names never leave the machine | It sends nothing on its own |
| timed | A request for the time to `time.cloudflare.com`, `nts.netnod.se` or `ptbtime1.ptb.de`, over Network Time Security, every 1 to 17 minutes, and to `pool.ntp.org` over plain NTP only while none of them answers; the requests carry random numbers and the server's own cookies, nothing about the machine | Other servers, or none, in timed(5); `enabled = false` in its service file |
| netlog | The log, to a computer you name, only when you name one to debug a machine | It sends nothing by default |
| logforward | The log, to the syslog servers you name in logforward.toml(5), over TLS, TCP or UDP, from when you name each, and again to a server that missed some | It sends nothing until you name a server; take the destination out, or `enabled = false` in its service file (logforward(8)) |
| metricsd | Nothing on its own. A Prometheus server that metrics.toml(5) names gets the machine's figures when it asks | It listens nowhere by default; take `listen` out of metrics.toml(5), or `enabled = false` in its service file (metricsd(8)) |
| alertd | The title and detail of each alert, which can name a disk's model and serial number, a mount point or a service, with the machine's name and the time, to the email account and the webhook addresses you name in alerts.toml(5), over TLS only | It sends nothing until you name a channel; take the channel out of the file and run `service alertd reload`, or `enabled = false` in its service file (alertd(8)) |
| discoverd | The machine's name and addresses, by multicast DNS on the local network only, so that `ssh name.local` works, and WS-Discovery probes that find Windows machines, printers and scanners there; nothing leaves the local network | `enabled = false` in its service file (discoverd(8)) |
| updated | Ten minutes after each start, then once a day, a read of the update channel at `updates.dukovo.org`, or the server you name, for a newer system and firmware set, and of the app catalogs; nothing about the machine | `check = "off"` in updates.toml(5), or `enabled = false` in its service file (updated(8)) |
| bundled | For bundles installed from a catalog, a daily read of that catalog for updates and known flaws, at `updates.dukovo.org` for Dukovo's own | Nothing is read while no catalog bundle is installed; updates by themselves stay off unless you turn them on with `bundle update --automatic on` (bundle(1)) |

## What you send when you report a problem

A problem report is sent only by you: `report` shows every byte of it
first, and sends exactly that text and nothing else. It goes to
`reports.dukovo.org`, or the server report.toml(5) names, over TLS. It
holds your answers to three questions, facts about the machine (the
Dukovo version and channel, the maker and model, the processor, the
amount of memory, the vendor and device IDs of the PCI and USB devices
with no serial number, and the language), and what the report is about: for
a crash, the program, the signal and the functions on its stack; for a
service, its last 200 lines of log, which only an administrator may add
and which you can leave out. Before you see it, the text is cleaned: user
names, home folders, the host name, IP and MAC addresses, email
addresses, Wi-Fi network names, serial numbers and the machine's ID are
cut out and marked, in your own words too.

No registers, no bytes of the stack, no environment, no files, no account
and no cookie go with it. The server is given a hash of a key made for the
report, so that only you can ask where it stands and have it deleted. A
report for the security team is encrypted to its keys before it leaves
the machine. `report --save` writes the text to a file or hands it to a
phone, and sends nothing to a server. The reference is dukovo-privacy(7).

## Reporting a security problem

Report a vulnerability privately, never in a public issue: open the
repository's Security tab on GitHub and choose **Report a
vulnerability**, as `SECURITY.md` in the source tree says. The
maintainers acknowledge a report within 3 working days, agree on a
disclosure date with you within 14, and release a fix and an advisory
within 90 days of the report.
