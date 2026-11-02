<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0340. wgd runs WireGuard on boringtun

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's 12d asks for WireGuard, with wg's commands and wg-quick's
files, working with Linux's. Tunnels are kernel links a program carries
(ADR 0339). The implementations a program can carry them with are
wireguard-go (MIT, Go, which Dukovo does not build), boringtun
(Cloudflare's, BSD-3-Clause, Rust, run by Cloudflare's WARP clients)
and wireguard-rs (unmaintained). Each speaks WireGuard's cross-platform
text protocol (wireguard.com/xplatform), which wireguard-tools' wg uses
on Linux for any interface not in the kernel. Linux keeps wg-quick's
files in /etc/wireguard; Dukovo's settings in /system/settings are read
by every service, and these files hold private keys.

## Decision

- **wgd(8), a service of systools, on boringtun 0.7.1** with its default
  features off: its noise protocol, sessions, timers and cookies alone,
  without its device, threads or tun code. One thread carries every
  interface: it polls their tunnels' files, their UDP sockets and wg's
  connections, runs each peer's timers every 250 ms and lets the
  handshakes' rate limiter count afresh every second, as boringtun's
  device does. Each interface has one UDP socket, [::]:port, which takes
  IPv4 too (ADR 0332).
- **Cryptokey routing as WireGuard's paper gives it**: a packet from the
  tunnel goes to the peer whose AllowedIPs hold its destination, the
  longest prefix first, and nowhere if none does or that peer has no
  endpoint; a packet a peer sent reaches the tunnel only if the same
  lookup of its source gives that peer. A prefix given to a second peer
  moves to it, as on Linux. A peer's endpoint becomes where its last
  packet that opened came from, so that a roaming peer is followed.
- **The files are wg-quick's: /system/state/wireguard/NAME.conf** for
  interface NAME (1 to 15 of wg-quick's characters), which only the
  system reads; wgd makes the folder 0700 and warns of a file others
  may read. `[Interface]` takes PrivateKey, ListenPort, FwMark, Address
  (which wgd gives the interface) and MTU (always 1420; another is
  warned of); DNS, Table and the Pre/Post scripts are warned of and
  passed over. `[Peer]` takes PublicKey, PresharedKey, AllowedIPs,
  Endpoint (a name is looked up when the file is read) and
  PersistentKeepalive. A mistake names its line, and leaves the
  interface as it was, so that a slip in a file read again does not
  take a working tunnel down.
- **wgd brings up an interface for each file** at its start, making the
  tunnel or taking the one of that name its last run left, and on
  SIGHUP (`service wgd reload`) reads them again: a new file's comes up,
  a removed one's goes, and the rest take their files as `wg syncconf`
  would give them, keeping the sessions of peers whose keys,
  preshared key and keepalive did not change. SIGTERM takes the
  interfaces away. Its service is on by default and holds no port
  without files; its file asks for port 51820, the network, the control
  right and its two folders.
- **wg(8) is wireguard-tools' wg** over /system/live/wireguard/NAME.sock
  (0600, the system's): `show` and its fields and `dump`, `showconf`,
  `set`, `setconf`, `addconf`, `syncconf`, `genkey` (clamped), `genpsk`
  and `pubkey`, with wg's words, sorting and colours. What it changes
  lasts until wgd reads the file again, as on Linux until wg-quick's
  `save`. FwMark is kept and shown, but marks nothing yet.

## Consequences

- Dukovo machines reach each other and Linux's WireGuard through
  tunnels; the network-wireguard boot test runs two machines on a cable,
  each with wgd, and checks TCP and ping by IPv4 and IPv6 inside the
  tunnel, that the cable carries none of it in the clear, that a peer's
  packets from an address its AllowedIPs leave out are refused, and
  that a reload keeps the session.
- Full tunnels (AllowedIPs = 0.0.0.0/0 with the default route through
  the interface) wait for the routing table, as ADR 0339 says; DNS= waits
  for resolved(8) to take per-interface servers. wg-quick itself is not
  there: a file and `service wgd reload` do its work.
- boringtun's crates.io package carries no license file, so ob keeps its
  repository's LICENSE.md and puts it with the image's notices.
- WireGuard's cryptography runs in a program, at a cost of a copy each
  way through the tunnel's queues; the network bench's tunnel rows, when
  they come, measure it against Linux's in-kernel WireGuard.
