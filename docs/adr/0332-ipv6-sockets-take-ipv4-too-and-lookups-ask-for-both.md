<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0332. IPv6 sockets take IPv4 too, and lookups ask for both

- Status: Accepted
- Date: 2026-11-02

## Context

With IPv6 addresses on the cards (ADR 0331), programs need AF_INET6
sockets: curl, OpenSSH, git and the programs on Rust's standard library
open them whenever a name has an AAAA record. On Linux an AF_INET6
socket bound to `::` takes IPv4 connections too, showing their
addresses IPv4-mapped (`::ffff:10.0.2.2`), unless IPV6_V6ONLY is set;
servers rely on it to listen once for both. A link-local address needs
the interface it is on (`sin6_scope_id`). glibc's `getaddrinfo` asks
for A and AAAA records at once for AF_UNSPEC and orders the answers as
RFC 6724 says, and AI_ADDRCONFIG leaves out a family the machine has
no address of.

Radix's ports were one table for IPv4 (ADR 0183), and mlibc asked only
IPv4 nameservers for A records.

## Decision

- **AF_INET6 sockets in the kernel, dual-stack by default**: one bound
  to `::` takes IPv4 too, its peers shown IPv4-mapped, unless
  IPV6_V6ONLY. Each version of IP has ports of its own, so a socket on
  `0.0.0.0:80` and one on `[::]:80` with IPV6_V6ONLY do not clash, as
  on Linux; a dual-stack socket holds the port in both. A socket on any
  address is kept to its version by a smoltcp patch.
- **radix-network(2)'s Address gains a scope**, the interface of a
  link-local address, which the C library fills from `sin6_scope_id`.
- **The C library (mlibc's Radix sysdeps, patch 0053)** makes AF_INET6
  sockets with sockaddr_in6, SO_DOMAIN, IPV6_V6ONLY and
  IPV6_UNICAST_HOPS; asks IPv6 nameservers; asks AF_UNSPEC lookups for
  A and AAAA records and orders them by RFC 6724; and answers
  AI_ADDRCONFIG from the interfaces' files.
- **`/processes/net` gains Linux's `if_inet6`, `ipv6_route`, `tcp6` and
  `udp6`**, which ss(8), ip(8) and ported tools read.

## Consequences

- Servers listen once, on `::`, for both versions; programs reach
  names that have only AAAA records.
- The `posix` boot test's inet part checks a connection over `::1` and
  an IPv4 client of a socket on `::`; `sockets` checks Rust's std; and
  `network-pair` checks curl between two machines by IPv6.
