<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0334. The firewall denies what other machines start, unless a service listens there or a rule allows it

- Status: Accepted
- Date: 2026-11-02

## Context

The roadmap's 12c asks for "a small ordered packet-filter ruleset that
denies inbound connections by default (enabling sshd opens only port
22), managed with a firewall command that lists, adds and removes rules
and logs dropped packets to logd, documented in firewall(8)", and adds:
"a program a user runs stays reachable from this machine, and when it
listens beyond it, firewall status lists it and one command allows it
from other machines, remembered per program". It is done when inbound is
blocked and sshd's port 22 opens only when sshd is on.

A firewall that needs a rule per service drifts from what runs: a
service enabled and its port forgotten, or a port left open after the
service went. Spark already knows which ports each service may listen
on (`network.listen`, ADR 0184).

## Decision

- **The filter is in the kernel, in each interface's hooks** (radix-net
  `filter`): it decides only about what other machines start, a TCP
  connection (SYN without ACK), a UDP datagram that answers nothing
  this machine sent, and a ping; the segments of connections and
  answers pass, and `lo` has no hooks. What is not let in is dropped
  without an answer, counted, and logged outside the stack's lock, at
  most five a second and then how many more.
- **Rules first, in order**: allow or deny, TCP, UDP, ping or any, a
  port range, a network it comes from, an interface, and a program. The
  first that matches decides; 128 at most.
- **Without a rule, a ping is answered, and a connection or datagram
  reaches only a socket bound through a network capability with the
  new `inbound` right** for its port. Spark gives that right with the
  ports a service's file asks to listen on, so a service's ports are
  open exactly while it runs, and sshd's file will ask for port 22
  alone. The sessions of people who sign in get every right but
  inbound, and bundles never get it: what a person runs listens for this
  machine alone.
- **A rule names a program by the file it runs from**: the kernel keeps,
  for each process, the mount and file numbers of what it last ran
  with `exec` (the script for a script), and each socket records its
  owner at bind. `firewall status` lists every listener, its program and
  process and who reaches it, with the one command that allows it,
  `firewall allow program PATH`, kept in the rules file.
- **The firewall capability** (kind 23, radix-firewall(2), read and
  control) holds the rules. The kernel gives it to Spark; Spark gives a
  copy to a service whose file asks for `admin.firewall`: the
  `firewall` service, which loads `/system/settings/firewall` at boot,
  and authd, which hands it to an administrator's command in elevate(8).
- **firewall(8)** is in systools: `status`, `list`, `allow`, `deny`,
  `allow process N`, `remove N` and `load`.

## Consequences

- Enabling a service opens its ports and stopping it closes them, with
  no rule to keep in step; a fresh machine answers nothing it did not
  start.
- A person's development server is reachable from their own machine at
  once, and from others after one command that says what it allows.
- The `network-pair` boot test probes it from a second machine.
- Rules cannot yet match outbound traffic or rate-limit; both wait for
  a need.
