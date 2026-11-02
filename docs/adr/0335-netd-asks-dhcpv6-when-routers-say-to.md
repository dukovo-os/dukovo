<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0335. netd asks DHCPv6 for addresses and settings when routers say to

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's netd is a "DHCPv4/v6 client". A router advertisement's M
flag says addresses come from DHCPv6, and its O flag that other
settings do, the DNS servers and the domains to search; some networks
give DNS servers only that way, and managed ones give addresses only
that way. QEMU's user network sets neither flag, but answers
INFORMATION-REQUEST, without naming itself, and from port 8962: 547 with
its bytes swapped.

## Decision

- **A DHCPv6 client in netd** (`dhcp6.rs`, RFC 8415), a state machine
  with no I/O of its own, as the DHCPv4 one is (ADR 0191). With O it
  asks for the settings alone with INFORMATION-REQUEST, again when the
  server's refresh time ends, a day without one. With M it asks for an
  address too, in an IA_NA: SOLICIT, ADVERTISEs for a first timeout,
  REQUEST to the best, RENEW at T1, REBIND at T2, and the address given
  up when its valid lifetime ends or a server has no binding for it.
  Retransmissions are RFC 8415's.
- **The client is known by its DUID-LL**, from the card's MAC address,
  and its IA_NA by the MAC address's last four bytes, so that both are
  the same at every boot.
- **netd sends from a socket of its own on port 546**, bound to the
  card; its file asks to listen on 546 as on 68. It gives the card the
  address as ip(8) would, keeping the others, and puts the DNS servers
  and domains into resolv.conf after DHCP's.
- **What a server says is checked first**: an answer must match the
  transaction and name the client and a server, except that an answer
  to INFORMATION-REQUEST that names no server is taken, as QEMU's is,
  from whatever port it comes, as other clients take it; addresses must
  be ones a host may have, and domains only names.
- **network(5)'s `dhcpv6`** says when a card asks: `auto` (as its
  routers say), `settings`, `address` or `no`.

## Consequences

- A machine gets its DNS servers on IPv6-only networks that give them by
  DHCPv6, and its address on managed ones.
- The `network-pair` boot test checks the settings from QEMU's network
  through a bridge, with `dhcpv6 = settings`.
- Prefix delegation (IA_PD), for a machine that routes for others, is
  left for when Dukovo routes.
