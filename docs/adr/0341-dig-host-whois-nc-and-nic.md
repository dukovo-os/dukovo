<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0341. dig, host, whois, nc and nic

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's 12e gives the administrator the network tools a Linux one
reaches for: dig and host, nc, whois and ethtool among them. Scripts and
habits read their output, so each must take Linux's options and print
Linux's words. On Linux, dig and host are BIND's (MPL-2.0, with its own
libraries), whois is Marco d'Itri's (GPL-2.0) or FreeBSD's, nc is
OpenBSD's netcat as Debian ships it, and ethtool (GPL-2.0) reads the
cards through the SIOCETHTOOL ioctl. The roadmap named hickory-dns for
dig and host. Dukovo already has its own DNS code: netproto's, which
resolved(8) asks the servers with (ADR 0193). Radix's drivers know each
card's model, rings and link (ADR 0197), but the kernel showed none of it
past its log.

## Decision

- **dig(1) and host(1) are systools', on netproto's DNS code**, not
  hickory's: the messages resolved(8) builds and reads are the ones dig
  shows, and a second DNS library would bring its own parser for the
  same bytes. netproto gains the names of types, classes, opcodes and
  rcodes and the presentation of each record's data (`present`): every
  type BIND's dig 9.18 shows by name in an answer, from A to CAA, with
  DNSSEC's records in BIND's forms and any other as RFC 3597's `\# len
  HEX`. dig asks over UDP with EDNS's OPT record, 1232 bytes, and the RD
  and AD flags, as BIND's 9.18 does; asks again over TCP for an answer
  cut short; and prints its sections, flags and statistics with BIND's
  words and tab stops. Its `+` options are BIND's that a single query
  needs; `+trace` and `+nssearch`, which walk the delegations, say they
  are not supported yet. host is the same code with BIND's host's
  sentences and its defaults: A, AAAA and MX for a name, PTR for an
  address. Both ask resolv.conf's servers, the local resolver on a
  machine that has no other, unless told one; their exit codes are
  BIND's (dig: 9 when no server answers, 10 for an internal failure).
- **whois(1) is systools'**, RFC 3912 over TCP port 43: it starts at
  IANA's server, whose `refer:` names the registry, and follows
  `whois:`, ARIN's `ReferralServer: whois://` and a thin registry's
  `Registrar WHOIS Server:` up to four servers, never one twice,
  printing each answer after a line naming its server, as FreeBSD's
  whois does; ARIN's server is asked `n + ADDRESS` for an address. `-h`
  asks one server and follows nothing. It keeps no list of registries of
  its own: IANA's answers stay right as registries move.
- **nc(1) is OpenBSD's netcat 1.238 with Debian's patches**
  (netcat-openbsd 1.238-1, BSD-3-Clause), Linux's nc on Debian and
  Ubuntu, with their options (`-N`, `-q`, `-b`, `-C`...). A patch of our
  own puts in what Debian takes from libbsd and mlibc lacks: strtonum,
  readpassphrase, arc4random_uniform over getrandom and b64_ntop, each
  as OpenBSD's.
- **Sockets take MSG_PEEK.** nc's UDP listener peeks at the first
  datagram to learn who sent it before it connects, and mlibc's Radix
  code refused the flag. radix-file(2)'s SocketFlags gain `peek`
  (bit 3), which `receive`, `receive_from`, `receive_message` and
  `receive_files` take. A datagram or ping socket leaves its datagram
  where it was; a TCP socket shows its bytes from both pieces of its
  receive buffer (smoltcp's TCP socket gains `peek_at` for the second)
  and opens no window; a Unix socket shows its bytes up to the next that
  files ride on, and those files come as copies, to each peek and to the
  receive that takes their byte, as on Linux. mlibc peeks once into the
  sum of recvmsg's buffers, and a peek with MSG_WAITALL waits for some
  bytes, not all.
- **getnameinfo keeps quiet and names scopes.** mlibc's printed "ipv6
  is not fully supported in this function" to standard error for every
  IPv6 address, which nc and ssh then said on each IPv6 connection, and
  it dropped a link-local address's scope. A patch removes the line and
  appends the scope as glibc does: `fe80::1%net0`, or the number with
  NI_NUMERICSCOPE, for an interface that is gone, or another scope.
- **nic(8) is systools', ethtool's output over the kernel's files**: a
  command named for what it shows, with ethtool's options (`-i`, `-g`,
  `-k`, `-S`, `-P`) and words, so that what reads ethtool's output reads
  nic's. `/kernel/class/net/NAME` gains Linux's `speed` and `duplex`,
  from each driver's link (radix-nic's `Nic::link`, a USB adapter's link
  page, a virtio card's status without a speed), and Radix's `card`: the
  driver, the model, the PCI address, the rings, the interrupt and the
  frames the driver dropped. The kernel uses no card's offloads, so `-k`
  shows each off and fixed, `lo`'s checksums on and fixed as on Linux;
  `-s` and `-K`, which change a card on Linux, say why they cannot.

## Consequences

- dig and host print what BIND's do for the questions a script asks,
  over the code the resolver already trusts; what BIND has beyond one
  query (+trace, zone transfers, TSIG, DNS over TLS from dig) waits.
- Every interface's speed, duplex and driver can be read without
  privilege, from the files any program may read, as on Linux.
- The network-tools boot test runs two machines on a cable, an e1000e
  and a virtio card: nic shows the e1000e's 1000 Mb/s full duplex and
  the virtio card's unknown speed; nc carries TCP and UDP both ways;
  dig and host read a test name server and ask again over TCP; host
  reaches it through resolved; and whois follows a test IANA's referrals
  through the names resolved finds.
- The roadmap's source column for dig and host changes from hickory-dns
  to netproto's own code.
- Programs that peek, as nc does before it connects a UDP socket, work
  as on Linux; the posix test checks MSG_PEEK on TCP, UDP, ping and Unix
  sockets. The ABI header's new flag rebuilds every port.
