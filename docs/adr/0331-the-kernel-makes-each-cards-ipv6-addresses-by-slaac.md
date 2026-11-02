<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0331. The kernel makes each card's IPv6 addresses by SLAAC, stable and temporary ones

- Status: Accepted
- Date: 2026-11-02

## Context

The roadmap's 12c asks for IPv6 with NDP and SLAAC, "using stable
private (RFC 7217) and temporary (RFC 8981) addresses by default", and
is done when curl reaches a machine over IPv6. Until now the kernel's
stack (smoltcp, ADR 0180) carried only IPv4.

smoltcp 0.14 parses IPv6 and answers neighbour solicitations and
echo requests, and keeps the neighbour cache, but it has no address
autoconfiguration: it keeps no router advertisement's prefixes or
RDNSS option, sends no router solicitation, tests no address before it
uses it (duplicate address detection, RFC 4862) and holds at most a
handful of addresses. Linux does SLAAC in the kernel, with
`addr_gen_mode` choosing stable-privacy addresses and `use_tempaddr`
the temporary ones; the address a card's MAC address gives (EUI-64)
follows a laptop from network to network and is what RFC 8064 says to
stop using.

## Decision

- **SLAAC is in the kernel, in radix-net's `ipv6` module**, beside
  smoltcp and around it: it reads every router advertisement a card
  receives, with all of its prefix options and its RDNSS option (RFC
  8106), sends three router solicitations four seconds apart when the
  link comes up (RFC 4861), and gives smoltcp the addresses, routes,
  DNS servers and MTU that follow. At most 3 prefixes, 2 routers, 3 DNS
  servers and 12 addresses a card; smoltcp is patched for 16 addresses
  and to answer duplicate address tests.
- **Stable addresses by RFC 7217**, the link-local one too (RFC 8064):
  the interface identifier is the first 64 bits of SHA-256 over the
  prefix, the card's MAC address, the duplicate address counter and a
  secret of the machine's, so an address is the same on the same
  network at every boot and differs from network to network. netd(8)
  makes the secret once, keeps it in `/system/state/netd/ipv6-secret`
  and gives it to the kernel with the card's settings.
- **Temporary addresses by RFC 8981**, on by default: a random
  identifier, preferred for a day less up to 0.4 of one at random and
  valid for two, the next made five seconds before the last is
  deprecated; a card sends from its temporary address when it has one.
- **Every address is tested first** (RFC 4862, 5.4): one neighbour
  solicitation from the unspecified address and a second's wait. A
  stable address that is taken is made again with the next counter,
  three times at most; a temporary one with a new identifier.
- **radix-network(2) version 3 adds `configure_ipv6`**, which netd
  calls through the control right: IPv6 on or off, temporary addresses
  on or off, the secret, and static addresses and a router.
  network(5) gains `ipv6`, `temporary-addresses` and IPv6 `address`,
  `router` and `dns` keys. `/kernel/class/net/NAME/ipv6` says what each
  card's IPv6 has, a line each, and the kernel logs each address and
  router as it comes and goes.
- **Left out: MLD.** The interface does not join its solicited-node
  groups by MLD, which only switches that snoop MLD need.

## Consequences

- A card on a network with a router has a global address within a few
  seconds of its link coming up, with nothing to set, and its
  addresses do not tell the networks it joins its MAC address or what
  it did yesterday.
- The `network-pair` boot test checks it through a bridge, against
  QEMU's user network's router.
- A switch that snoops MLD and drops what nobody joined could keep
  neighbour solicitations from a Dukovo machine; that is left for when
  one is seen.
