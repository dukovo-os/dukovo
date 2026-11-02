<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: In every image
| [mdns-sd](https://github.com/keepsimple1/mdns-sd) 0.21.7 by Han Xu, with [if-addrs](https://github.com/messense/if-addrs) 0.15.0 by MaidSafe and Messense Lv, patched for Radix, [socket-pktinfo](https://github.com/pixsper/socket-pktinfo) 0.4.1 by Pixsper, [flume](https://github.com/zesterer/flume) 0.12.0 by Joshua Barretto, [spin](https://github.com/mvdnes/spin-rs) 0.9.9 by Mathijs van de Nes, John Ericson and Joshua Barretto, [lock_api](https://github.com/Amanieu/parking_lot) 0.4.14 by Amanieu d'Antras and [fastrand](https://github.com/smol-rs/fastrand) 2.5.0 by Stjepan Glavina, from crates.io | discoverd(8)'s multicast DNS and DNS-SD, which answer for the machine's `NAME.local` and find what the local networks offer ([ADR 0338](docs/adr/0338-discoverd-finds-what-the-local-networks-have-with-mdns-and-ws-discovery.md)) | Apache-2.0 OR MIT (mdns-sd, flume, lock_api, fastrand); MIT OR BSD-3-Clause (if-addrs); MIT (socket-pktinfo, spin) |
