<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0349. Port forwarding over SSH: a forwarder that runs as the person makes every connection

- Status: Accepted
- Date: 2026-11-03

## Context

Phase 13c, after ADRs 0347 and 0348. The roadmap asks for port
forwarding, "gated by capabilities". OpenSSH's sshd forwards both ways
unless AllowTcpForwarding says otherwise: for `ssh -L`, `-D` and `-W` a
client opens `direct-tcpip` channels, and its sshd connects where each
names; for `ssh -R` it sends `tcpip-forward`, its sshd listens, and
each connection taken opens a `forwarded-tcpip` channel back (RFC 4254
section 7). On Linux the connecting and listening are done by sshd's
child that runs as the person, with whatever the network lets anyone.

Dukovo's sshd holds port 22 and nothing more of the network (ADR 0347),
and a person's network is a capability their session gets from authd
(ADR 0257): outbound connections, and listening on ports 1024 to 65535,
for this machine alone until the firewall lets others in (ADR 0334).
Sockets cannot be passed between processes on Radix, so no process with
the person's network can make a connection and hand it to sshd.

## Decision

- **A forwarder per connection, run as the person.** The first time a
  client forwards anything, sshd asks authd for an SSH session of the
  new kind `forward`, with the same proof as any session's, which authd
  checks again. authd starts `sshd --forward` as the person, with the
  network a session gets and nothing else of one's: no shell, whose
  startup could write into its frames, and no sign-in record. It holds
  three pipes from sshd, and ends when sshd closes them, as the
  connection ends, and every connection and port with it.
- **Frames between them** (`accounts::forwarding`, host-tested): a kind,
  a stream's or listen's number, a length, then at most 32 KiB. sshd
  asks for a connection to a host and port, or for a port to listen on;
  the forwarder answers that it connected, listens, or failed, and that
  a port took a connection. Then both pass a stream's bytes, its end one
  way (as `shutdown`), and its close. sshd numbers what it asks for with
  even numbers, the forwarder what it takes with odd ones.
- **Each stream goes only as fast as its reader.** The forwarder sends
  sshd no more on a stream than sshd gave it room for: 256 KiB as it
  opens, and as much again as sshd hands to the client, whose channel
  window it waits for, a stream at a time. sshd lets the client send no
  more than the forwarder has written to the connection, by the window
  it gives back. So a stalled connection holds up its own channel and
  nothing else, and neither process keeps more than a window per
  stream.
- **The forwarder's word is checked as a client's is.** It runs as the
  person, so sshd ends the connection's forwarding, and closes its
  channels, when it sends more than it had room for, answers what was
  not asked, takes a connection with a number of sshd's, or sends what
  does not read.
- **sshd_config(5) takes AllowTcpForwarding** (`yes` or `all`, `local`,
  `remote`, `no`; `yes` by default, as OpenSSH's) **and GatewayPorts**
  (`no`, `yes`, `clientspecified`; `no` by default: a port listens on
  127.0.0.1 and ::1 alone, as OpenSSH decides it). A port below 1024 is
  refused, as OpenSSH's sshd refuses it to anyone but root, and as the
  person's network would. Unix sockets (`streamlocal`) are refused.
- A connection forwards 128 connections at once, and 16 ports; names
  are looked up in the forwarder, as the person's programs look them
  up.

## Consequences

- `ssh -L`, `-D`, `-W` and `-R` work as against Linux, and what they
  connect to and listen on is the person's: the firewall and anything
  that later limits a person's network apply to forwarding as to their
  programs. A port forwarded on every address reaches other machines
  once an administrator lets it, with `firewall allow program
  /system/commands/sshd`.
- Turning forwarding off keeps nothing from someone who can run their
  own programs here, as OpenSSH's manual says; it is for administrators
  who want it anyway.
- Each forwarding connection costs a process and a round of frames;
  bytes are copied once more than in OpenSSH's sshd, through a pipe.
- **Not yet:** PermitOpen and PermitListen, X11 and agent forwarding,
  Unix sockets, and listen addresses given by name.
