<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0346. Packet captures, libpcap and tcpdump

- Status: Accepted
- Date: 2026-11-03

## Context

The plan's 12e lists tcpdump with the other administrator's tools.
tcpdump prints the packets an interface receives and sends, those a
filter such as `port 53` picks, and writes them to capture files that
Wireshark reads; it gets them through libpcap, which compiles the filter
to a classic BPF program and asks each system for packets its own way.
On Linux that is a packet socket (packet(7)), which only a program with
CAP_NET_RAW may open: it reads each frame of an interface, or of every
one, before the firewall sees it, with a `sockaddr_ll` saying which
interface it was and whether it came in or went out, and runs the
filter in the kernel. On the BSDs it is a BPF device, /dev/bpf, set to
an interface and given a filter with ioctls, whose reads give records of
a `bpf_hdr` and the frame.

Dukovo gives no program a raw socket (ADR 0345), and nothing in the
kernel let a program see the frames an interface carries.

## Decision

- **The kernel opens packet captures** (radix-network(2)'s
  `open_capture`, network interface version 7): files whose reads take
  the frames of the interface asked for, or with 0 of every interface,
  those added later among them. It needs the network's control right,
  which a service with admin.network and a command run with elevate(8)
  have, as Linux's packet sockets need CAP_NET_RAW: a capture reads
  everyone's traffic. A read takes as many whole records as fit in its
  buffer, each a CaptureHeader (the time in nanoseconds, the frame's
  length, the bytes kept, the interface's number, its link type and
  whether it went out), the bytes and zeros to a multiple of 8, as a BPF
  device's do, and cuts the first to the buffer if it alone does not
  fit. A capture keeps each frame's first `snap` bytes, up to libpcap's
  262,144, and keeps 64 KiB to 64 MiB of them for its reader, 2 MiB
  unless asked, libpcap's default on Linux, in pieces of 64 KiB, as
  Linux's packet rings are blocks; a frame that finds no room
  is lost and counted, as Linux and the BSDs count theirs. A file may
  be non-blocking, and a poll finds it readable while a frame waits.
  Sixteen captures may be open at once, between every program; the
  next fails with EBUSY, as opening /dev/bpf does on a BSD whose BPF
  devices are all taken. When the
  interface is taken away, a read finds what waits, then fails with
  ENXIO, and a poll finds the file in error.
- **Each interface's stack has a tap**, which sees each frame it
  receives before anything takes it, the firewall, bridges and VLANs
  among them, as Linux's packet sockets see frames before netfilter, and
  each frame it sends as it goes to the link. A card's frames are
  Ethernet's; a tunnel's are the IP packets alone (pcap's LINKTYPE_RAW);
  `lo`'s are seen once each, as they are received, behind an Ethernet
  header of zero addresses, as Linux's `lo` gives them. While `lo` is
  watched a TCP connection over it sends its bytes in packets, which
  the loopback link otherwise hands across in place, for them to be
  seen. An interface no capture looks at costs a load and a branch for
  each frame.
- **Classic BPF filters run in the kernel**, with a file control,
  CAPTURE_FILTER, that takes the program as libpcap's `struct bpf_insn`
  and Linux's `struct sock_filter` lay it out: up to 4,096 instructions,
  Linux's BPF_MAXINSNS. radix-net's `bpf` checks a program as Linux's
  `bpf_check_classic` and the BSDs' `bpf_validate` do (every
  instruction classic BPF's, every jump forward and inside it, the
  scratch memory's sixteen words, no division by a constant 0 or shift
  by 32, a return last) and refuses others with EINVAL, and runs it on
  each frame as the paper and both systems define it: a load past the
  frame's end, or a division by an X of 0, returns 0, and the frame is
  not kept; otherwise what it returns is how many of the frame's bytes
  are. Setting a filter throws away the frames
  waiting and counts again, as BIOCSETF does. CAPTURE_COUNTS gives the
  frames kept and those lost. There is no eBPF and no compiler of
  filters to machine code: a filter of tcpdump's takes tens of
  instructions for each frame.
- **libpcap 1.10.7 is ported with a capture module of ours**,
  `pcap-radix.c` (Apache-2.0 OR MIT), in the place of `pcap-linux.c`: it
  lists the interfaces from getifaddrs and if_nameindex with their
  state from /kernel/class/net, and "any"; opens the capture with the
  snap length and buffer asked for; turns the records into pcap's
  packet headers, by microseconds or nanoseconds; gives the kernel the
  filter for one interface and runs libpcap's own for the rest; takes
  `-Q`'s direction from the header; counts with CAPTURE_COUNTS; and
  stops at pcap_breakloop through a pipe. For "any" it cooks each frame
  as Linux's packet sockets do, into DLT_LINUX_SLL or DLT_LINUX_SLL2,
  with the interface's number, its ARPHRD type and whether the frame was
  the machine's, a broadcast, a multicast or another's, so that tcpdump
  prints each packet's interface and direction as on Linux. Two patches
  leave out pcap_lookupnet's ioctls, which the module answers from
  getifaddrs, and the timeradd macros mlibc has as functions. libpcap's
  remote capture, USB, Bluetooth, D-Bus, RDMA, netmap and DAG modules are
  not built.
- **tcpdump 4.99.6 is ported on it**, without libcrypto (so no `-E`
  decryption of ESP), libcap-ng or libsmi, and installed as
  /system/commands/tcpdump with its manual page and libpcap's
  pcap-filter(7), pcap-linktype(7), pcap-tstamp(7) and pcap-savefile(5).
  It is an ordinary command, not setuid: run with elevate, it captures.
- **Not yet**: a card is not put in promiscuous mode, as Dukovo's drivers
  cannot yet be asked to take every frame; libpcap says so, and
  tcpdump warns ("That device doesn't support promiscuous mode"), and
  captures the frames the driver lets in: the machine's own, broadcasts
  and its multicast groups'. A capture cannot send (pcap_inject fails),
  and there is no memory-mapped ring as Linux's TPACKET_V3: each read
  copies.
- **Their licenses are kept whole.** libpcap's and tcpdump's files carry
  many notices: the University of California's and Lawrence Berkeley
  Laboratory's BSD licenses, among them four-clause ones with the
  advertising clause (BSD-4-Clause, BSD-4-Clause-UC and the shortened
  "provided that" form, BSD-4-Clause-Shortened), and BSD-3-Clause,
  BSD-2-Clause, ISC and the 4.3BSD Reno and Tahoe notices; a few of
  libpcap's are BSD-3-Clause or GPL-2.0-only, of which Dukovo takes the
  BSD license. Each port's manifest names them by their SPDX
  identifiers, checked against the SPDX License List's templates, and
  gives a text of its own, LicenseRef-libpcap-notices and
  LicenseRef-tcpdump-notices, with every notice of every file the build
  compiles, as the file has it: `ports/libpcap/notices.py` writes it
  from the compiler's dependency files, and the build fails if it is not
  the one in image/licenses. Their advertising clauses ask that
  advertising materials that mention their features or use carry the
  University's acknowledgement; they bind nothing else, and every Linux
  distribution and BSD ships both. No GPL code is linked with them.

## Consequences

- tcpdump runs as on Linux for an administrator, with Linux's options,
  filters, lines and capture files, on cards, `lo`, tunnels (WireGuard's
  among them) and every interface at once. The network-capture boot
  test checks the kernel's captures, filters, cuts, counts and limits,
  and tcpdump on `lo`, `any`, a tunnel and a virtio card, into files and
  back, by direction, refused without the control right, and stopped by
  SIGINT.
- An interface no capture looks at pays a load and a branch for each
  frame: a UDP datagram there and back on loopback takes 17,835
  instructions rather than 17,794 (its budget is 19,000), and a TCP
  connection made, taken and closed 57,857 rather than 57,684
  (62,000).
- Advertising that names tcpdump's or libpcap's features must carry
  their acknowledgement, "This product includes software developed by
  the University of California, Lawrence Berkeley Laboratory and its
  contributors"; their notices are in the image's licenses.
- Promiscuous mode waits for the drivers to take a receive filter; so
  do capturing with a TPACKET-like ring and sending through a capture.
- libpcap's and tcpdump's security releases are taken as they come, as
  every port's are.
