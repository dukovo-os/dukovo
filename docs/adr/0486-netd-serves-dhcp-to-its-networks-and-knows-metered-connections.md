<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0486. netd serves DHCP to its networks and knows metered connections

- Status: Accepted
- Date: 2026-11-13

## Context

A small office's or a lab's server is often its network's DHCP server
too. That is the job of ISC dhcpd (now ended), Kea or dnsmasq on Linux,
and of the "DHCP server" checkbox in Webmin, Proxmox's SDN and TrueNAS's
apps. Without one, a network whose router gives no addresses, or a
private network behind the server's second card, needs a second
program and its own settings, in a file format of its own.

netd already holds each card's settings (network(5)) and runs the DHCP
client with smoltcp's DHCP messages (ADR 0191). The server is the other
half of the same protocol, from the same settings file.

A server on a phone's hotspot, a 4G router or a satellite link pays for
what it downloads. updated (ADR 0502) would download a release there as
anywhere else. Android's hotspot says it is metered in its DHCP answer
(vendor option 43, `ANDROID_METERED`), which NetworkManager reads.
docs/deferred.md kept both the server (ADR 0194) and metered
connections (ADR 0502) open for this phase.

## Decision

- **A card with an address of its own gives its network addresses by
  DHCP when network(5) gives it a range** (`dhcp-server =
  192.168.10.100-192.168.10.199`).
  - The range must be inside the card's network, at most 65,536
    addresses, and must not hold the network's own address, its
    broadcast address or the card's.
  - `reserve = MAC ADDRESS [NAME]` gives a machine its address always,
    and nobody else, even outside the range.
  - The lease lasts 12 hours unless `dhcp-server-lease` says otherwise.
  - The router, DNS servers, domain and time server are the card's own
    unless said:
    - the router is the section's `router`;
    - the DNS servers are those resolv.conf names that are not
      loopback;
    - the time server is the card's address while timed serves time
      (ADR 0487).
  - A mistake turns the server off for that card and says why.
- **It answers as RFC 2131 has a server answer**
  (`systools/src/dhcp_server.rs`, which has no I/O of its own and is
  tested with time of its own):
  - **DISCOVER:** the client's reservation, else the address it had
    last (by client identifier, else MAC address), else the one it asks
    for if free, else the lowest free address. An address whose lease
    ended goes to another client only when no other is free. An offer
    is held for a minute.
  - **REQUEST:** acknowledged for an offer it made and an address the
    client holds. Refused for another's address or another network.
    Unanswered for an INIT-REBOOT client it knows nothing of, which
    another server may know.
  - **DECLINE** leaves the address alone for 10 minutes. **RELEASE**
    ends the lease, and the address stays the client's first choice.
    **INFORM** gets the options without a lease.
  - A NAK is always broadcast. Relayed requests are ignored, and the
    log says so once.
  - Every field of a packet is read through bounds checks. Anything not
    a well-formed request from an Ethernet client is ignored, never
    answered.
- **netd runs it** on a socket of its own on port 67, bound to the card,
  in a thread of its own.
  - Each lease is written to `/system/state/netd/dhcp-server-CARD`
    (synced, then renamed) before the client is told it has it.
  - Leases survive a restart, and a reload of the settings keeps those
    that still fit.
  - The log has a line for each new lease, decline and release, and
    when the pool is full.
  - `netd leases` lists the leases and the reservations, with the time
    left.
- **Metered connections.**
  - A card's `metered = yes | no | auto`. `auto`, the default, believes
    an `ANDROID_METERED` in the card's own DHCP lease.
  - netd writes `/system/live/netd/metered`. Its first line is `yes` or
    `no` for the card with the default route, then a line per card. It
    logs a change of the first line.
  - updated, with updates.toml's `metered = "wait"` (the default), still
    reads the update server's small signed files on a metered
    connection, and says what is ready. It downloads and stages nothing,
    for the system or for apps, until the connection is not metered or
    an administrator runs `update apply`.
  - `dhcp-server-metered = yes` tells the network's own clients that it
    is metered, for a server whose uplink is.

## Consequences

- One section of network(5) makes a Dukovo server the DHCP server of
  its network, with its time server and DNS given out too.
- A machine on a hotspot does not spend its data on a release.
- Not yet, each a row of docs/deferred.md:
  - a DHCPv6 server and router advertisements: IPv6 clients keep
    SLAAC from the network's router;
  - DHCP relays (`giaddr`);
  - the ICMP probe before an offer, as DECLINE covers a conflict (RFC
    2131 §3.1.5);
  - noticing another DHCP server on the network.
- Tests:
  - **Units:**
    - every message and state of the server against RFC 2131's rules;
    - reservations, a full pool, ended leases reused only after the free
      ones, and declines;
    - the lease file;
    - packets that are not requests, and no packet making it panic;
    - each setting and mistake;
    - option 43 read by the client, and updated's choice for each
      setting.
  - **The `network-serve` boot test**, two machines on a cable:
    - b gets its reservation with the options set, renews, and releases;
    - a second client gets the pool's lowest address;
    - b sees the network metered;
    - b's timed sets its clock from a over NTS.
  - **The lab's `network/dhcp-server`:** dhcpcd in a namespace on the
    lab PC.

## Alternatives considered

- **Porting dnsmasq or Kea.** Either is a second program with its own
  settings and leases file, beside netd's. dnsmasq's DNS half would
  overlap resolved. The server needed here is a few hundred lines on the
  messages netd already builds.
- **An ICMP echo before each offer**, as ISC dhcpd did. It delays each
  offer by a second and misses firewalled machines. Clients check with
  ARP and DECLINE, which the server honours.
- **Deciding metered by the card's kind** (Wi-Fi, USB modem). A Wi-Fi
  network may be a home's or a hotspot's. The hotspot's own word, and
  the owner's setting, are what NetworkManager and Android use.
