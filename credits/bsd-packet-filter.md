<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- The BSD Packet Filter, from Steven McCanne and Van Jacobson's paper
  [The BSD Packet Filter: A New Architecture for User-level Packet
  Capture](https://www.tcpdump.org/papers/bpf-usenix93.pdf) (USENIX Winter
  1993), with its instructions as Linux's `struct sock_filter` and
  libpcap's `struct bpf_insn` lay them out, and its checks of a program
  as the BSDs' `bpf_validate` makes them: the kernel's filters of packet
  captures, and how its captures keep frames for their readers and count
  those they lose ([ADR 0346](docs/adr/0346-packet-capture-libpcap-and-tcpdump.md)).
