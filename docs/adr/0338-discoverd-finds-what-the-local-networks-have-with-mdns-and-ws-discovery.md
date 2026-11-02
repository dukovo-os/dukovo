<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0338. discoverd finds what the local networks have, with mDNS and WS-Discovery

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's 12c.3 asks for mDNS and DNS-SD, so that `hostname.local`
works between machines, for `discover`, which lists what DNS-SD and
WS-Discovery find, and for the network.local permission that lets a
bundle see it. Linux has Avahi, a daemon that answers for the machine's
name and offers its services, and that programs ask over D-Bus; Windows
answers WS-Discovery's probes, and so do many printers and scanners.
Until now the machine's name was `dukovo` on every machine, fixed in the
C library.

## Decision

- **The machine's name is the first line of /system/settings/hostname**,
  as Linux's /etc/hostname, 64 bytes at most; `dukovo` without one.
  gethostname and uname read it; sethostname, which hostname(1) calls,
  writes it, as an administrator may (EPERM for anyone else, as on
  Linux); hostname(5) says so.
- **discoverd(8)**, a service of systools, on the mdns-sd crate (Apache-2.0
  OR MIT), built without its async channels: it answers for `NAME.local`,
  NAME the machine's name as a DNS label, with the addresses of the
  interface each question came by, and offers `NAME._workstation._tcp`,
  as Avahi does, so that the network's browsers list the machine. Its
  service file asks for the network and port 5353, which other
  machines' questions come to.
- **Its socket, /system/live/discoverd/socket**, takes one request a
  connection, a line: `resolve NAME` gives NAME's addresses, IPv4 first,
  this machine's own from its interfaces, as its own questions do not
  come back to it; `browse SECONDS [TYPE...]` gives, for that long, the
  services of each type found, or of every type DNS-SD's
  `_services._dns-sd._udp` says the networks have, and the devices that
  answer WS-Discovery's Probe, which discoverd sends to 239.255.255.250
  port 3702 and whose ProbeMatches it reads by their RelatesTo. A link-
  local IPv6 address is left out of what `resolve` gives, as a DNS
  answer cannot say its interface.
- **resolved(8) asks discoverd for `.local` names** (RFC 6762's), and
  answers A and AAAA questions with what it says, kept two minutes; any
  program's lookup finds them, as nss-mdns gives glibc's. Without
  discoverd, or with no answer in 1.5 seconds, the name is not found.
- **discover(1)** lists what `browse` finds, as a table, or with `-r` as
  discoverd's lines; **ping(8)** is systools' own, with iputils' options
  and output, through a ping socket (ADR 0336).
- **A bundle sees discoverd's folder only with network.local** (ADR
  0337), which bundle(1) puts in its view; the folder is the service's,
  which Spark makes, and its socket anyone may connect to.

## Consequences

- Two Dukovo machines find each other by name, and Linux's, macOS's and
  Windows's machines find a Dukovo one; the network-discovery boot test
  checks both directions between two machines on a cable, by IPv4 and
  IPv6, and what discover lists, and the bundle-run test that a bundle
  sees discoverd only with network.local.
- A name is announced once discoverd starts: `service discoverd restart`
  after hostname(1) gives the new one.
- Conflicts between two machines of one name are not resolved yet, as
  RFC 6762's probing would rename one; nor does discoverd offer services
  other than the workstation, or answer WS-Discovery itself.
