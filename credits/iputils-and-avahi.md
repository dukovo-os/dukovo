<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [iputils](https://github.com/iputils/iputils): ping(8)'s options and
  what it prints, and Linux's ping sockets, which let anyone ping
  ([ADR 0336](docs/adr/0336-multicast-groups-packet-info-and-ping-sockets-on-smoltcp.md)).
- [Avahi](https://avahi.org/): one daemon on the machine that answers for
  its `.local` name and offers its `_workstation._tcp` service, which other
  programs ask instead of speaking multicast DNS themselves
  ([ADR 0338](docs/adr/0338-discoverd-finds-what-the-local-networks-have-with-mdns-and-ws-discovery.md)).
