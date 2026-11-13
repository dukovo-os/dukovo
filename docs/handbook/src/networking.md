<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Networking

This chapter describes networking in Dukovo: sockets for programs, TCP
that matches Linux, `netd` with DHCP, DNS and `curl`, the time service,
the Realtek, Intel and USB Ethernet cards, IPv6, bridges, VLANs, the
firewall and WireGuard.

## What works

- **virtio network cards**, the ones QEMU and clouds on KVM give a
  virtual machine. The PCI scan finds each card and gives it a kernel
  thread, `net0`, `net1` and on, up to eight cards in all.
- **Intel's, VMware's, Amazon's and Google's cards**: Intel's e1000 and
  e1000e, the cards QEMU, VirtualBox and VMware give a machine by
  default, its igb, igc at 2.5 Gb/s and ixgbe at 10 Gb/s; VMware's
  VMXNET3; Amazon's ENA, on EC2; and Google's gVNIC, on Compute Engine
  (e1000(4), igb(4), igc(4), ixgbe(4), vmxnet3(4), ena(4), gve(4)).
  Every card is driven behind the IOMMU, through the same interface,
  and set up as a virtio card is
  ([ADR 0197](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0197-network-cards-share-one-driver-interface-and-copy-frames-through-slots.md)).
  QEMU emulates the e1000, e1000e, igb and VMXNET3, which the
  `network-cards` boot test runs; the others are checked against
  models of the cards, and the first boot on each kind of machine is
  their real test.
- **Realtek's chips**: the RTL8169, RTL8168 and RTL8111 at 1 Gb/s and
  the RTL810x at 100 Mb/s, the onboard Ethernet of most boards that is
  not Intel's (re(4)). No emulator has them: a model of the chips tests
  the driver, and the Dell Inspiron 15 5570 the real thing.
- **Realtek's 2.5 and 5 gigabit chips**: the RTL8125A, RTL8125B and
  RTL8125D at 2.5 Gb/s and the RTL8126A at 5 Gb/s, among them Killer's
  E3000 and E5000 and the Framework Desktop's port (rge(4)). The driver
  sets them up as OpenBSD's does, translated from its code, without the
  firmware Realtek ships for the chips, which is closed: they run on
  the code in their own ROMs. Its tests check its register reads and
  writes on each chip against OpenBSD's own code, run on a model of the
  chips; the first boot on a machine with one is its real test.
- **Atheros' chips**: the AR8131, AR8151, AR8161 and AR8171 at 1 Gb/s,
  the AR8132, AR8152, AR8162 and AR8172 at 100 Mb/s, and the Killer
  E2200, E2400 and E2500 built on them, on many laptops from 2009 to
  2018 (alc(4)). They too are tested against a model of the chips.
- **Broadcom's NetXtreme chips**: the BCM5717, BCM5718, BCM5719 and
  BCM5720 of many servers, the BCM5761 and BCM5762 of desktop boards,
  and the BCM57765 family and BCM57780 of laptops and Macs, copper ports
  only (bge(4)). On the chips with Broadcom's APE, which can share the
  port with a server's management controller, the driver takes the
  APE's locks and tells its firmware when it starts and stops. A model
  of the chips tests them too.
- **A card's driver can run as a program**: with `e1000=program`, which
  the images `ob image` builds have, the kernel leaves the e1000 and
  e1000e cards an IOMMU confines to e1000d(8), which drives them with
  the same driver in a process of its own; when it ends, the kernel
  stops the card and Spark starts it again
  ([ADR 0198](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0198-a-card-s-driver-can-run-as-a-program-on-a-device-capability.md)).
  The `userspace-e1000` boot test kills it halfway through 2 MiB of
  traffic, which comes back whole.
- **USB Ethernet adapters**: CDC-ECM and CDC-NCM on an xHCI
  controller, the protocols of most USB-C docks and of Realtek's RTL8153
  in its class configuration (cdc_ether(4)). An adapter gets its
  interface when it is plugged in, after the PCI cards', and the same
  one back when it is plugged in again
  ([ADR 0199](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0199-usb-ethernet-adapters-share-the-driver-program-link.md)).
  The `usb-ethernet` boot test runs QEMU's `usb-net` through DHCP, 2 MiB
  of traffic, and an unplug.
- **The TCP/IP stack runs in the kernel**, built on
  [smoltcp](https://github.com/smoltcp-rs/smoltcp) in safe Rust
  ([ADR 0180](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0180-the-tcp-ip-stack-runs-in-the-kernel-on-smoltcp.md)).
  It speaks Ethernet, ARP, IPv4, ICMP, TCP and UDP: it answers pings,
  the kernel's self-test pings with it, and programs connect, listen
  and send datagrams through it.
- **The loopback interface, `lo`**, at 127.0.0.1 and every card's own
  address: what a program sends to the machine itself goes through it,
  without touching a card. As on Linux, the program that sends a packet
  over it delivers it too, before its call returns, with no switch to
  another thread, and a program may connect to itself as often as it
  likes
  ([ADR 0185](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0185-loopback-runs-in-the-senders-thread-and-the-network-joins-the-gate.md)).
  A TCP connection over `lo` passes its bytes straight from the sender
  into the receiver's buffer, in no packet, as Solaris's TCP fusion
  does, so a byte is copied twice, as on Linux
  ([ADR 0186](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0186-a-loopback-connection-passes-its-bytes-straight-across.md)).
  `ob bench` holds TCP and UDP over `lo` to Linux's speed.
- **TCP behaves as Linux's** where a program or the other end can tell
  ([ADR 0187](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0187-tcp-keeps-quiet-connections-and-controls-congestion-as-linux-does.md)):
  CUBIC congestion control from a window of ten segments, a 200 ms
  floor on the retransmission timeout, timestamps from a random start
  for each connection, and keep-alive probes after two hours of quiet,
  nine of them 75 seconds apart, which TCP_KEEPIDLE, TCP_KEEPINTVL and
  TCP_KEEPCNT change. A connection with nothing to send stays open
  however long it is quiet; one whose data goes unacknowledged for 15
  minutes is reset.
  A window that loses several segments resends each of them in about
  one round trip, from the SACK blocks the other end sends, and old
  duplicates are dropped by their timestamps (PAWS)
  ([ADR 0188](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0188-tcp-recovers-from-losses-with-sack-and-an-interface-holds-thousands-of-sockets.md)).
- **Thousands of sockets on each interface**: up to 16,384, in slots the
  interface adds as they fill. A connection in TIME-WAIT that its
  program closed keeps none of its buffers, and a client may connect
  again from the same port at once, as on Linux (ADR 0188).
- **Sockets for programs, through network capabilities**
  ([ADR 0183](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0183-programs-reach-the-network-through-capabilities-of-its-ports.md)).
  A program uses the network only if it was given a network
  capability, a range of ports with two rights: *send*, to connect out
  and send datagrams anywhere, and *receive*, to bind to and listen on
  the ports in its range. The kernel checks the capability on every
  bind, connect and send, so no library can skip it. The kernel gives
  Spark one for all ports; Spark hands out narrower ones.
- **Each card reaches only its own memory** behind the IOMMU: the
  driver copies frames through slots of DMA memory that belong to the
  card
  ([ADR 0181](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0181-virtio-net-copies-frames-through-slots-of-its-own.md)).
- **smoltcp is patched** so that a socket's memory goes back to the
  kernel when the socket closes, and so that a full socket table or a
  stale socket never panics the kernel
  ([ADR 0182](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0182-the-kernel-patches-smoltcp-so-socket-memory-comes-back.md)).
  `./ob crates` fetches it from crates.io and applies the patch, and
  every build does that first.
- **Tested against broken and hostile networks**: `ob test` gives two
  stacks 100,000 frames each, changed at random, alongside their own
  traffic; neither may panic or send a malformed frame, and a new
  connection between them must work afterwards. Kani proves that the
  stack reads any Ethernet, ARP, IPv4, ICMP and UDP header, and any TCP
  option, without a panic and writes it back the same
  ([ADR 0189](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0189-frames-changed-at-random-break-neither-stack-and-kani-proves-the-headers.md)).

- **The C library's BSD sockets**: `socket`, `bind`, `listen`,
  `connect`, `accept`, `send`, `recv`, `sendto`, `recvfrom`, `sendmsg`,
  `recvmsg`, `getsockname`, `getpeername`, `getsockopt`, `setsockopt`
  and `shutdown` work on `AF_INET` TCP and UDP sockets, as radix-posix(7)
  describes. A call the program's capabilities do not allow fails with
  `EACCES`
  ([ADR 0184](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0184-services-and-the-c-library-use-the-network-capabilities-they-were-given.md)).
- **Services get the network their files ask for**: under
  `[permissions]`, a service file's `network.outbound = true` lets it
  connect out, and `network.listen` names the ports it may listen on
  (service.toml(5)). A service that asks for
  neither cannot use the network.

- **`ip` and `ss`**, with iproute2's words and output: `ip address`,
  `ip link` and `ip route` show the interfaces, their addresses and
  routes, `ip address add` and `ip route add default via` set a card
  up, and `ss` lists the TCP and UDP sockets
  ([ADR 0190](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0190-interfaces-are-shown-as-linux-shows-them-and-set-up-through-the-network-capability.md)).
  The interfaces are in `/kernel/class/net` and the counts, routes and
  sockets in `/processes/net`, in Linux's formats.
- **`netd` sets every card up by DHCP** at boot, or as
  `/system/settings/network` says, keeps each lease across restarts and
  reboots, and writes the DNS servers where `/etc/resolv.conf` leads
  ([ADR 0191](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0191-netd-sets-each-card-up-by-dhcp-or-as-its-settings-say.md)).
  What a DHCP server says is checked before it reaches the routes or
  the resolver.
- **TLS for programs in Rust**, rustls on ring, and the root
  certificates Mozilla trusts in
  `/system/resources/certificates/ca-certificates.crt`, which
  `/etc/ssl/certs/ca-certificates.crt` and `/etc/ssl/cert.pem` lead to
  ([ADR 0192](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0192-tls-is-rustls-on-ring-and-mozillas-roots-are-a-file-of-the-systems.md)).
  dukovo-certificates(7) says how to trust an authority of your own.
- **`resolved` looks every name up**, at 127.0.0.53, which
  `/etc/resolv.conf` names: it keeps answers as long as they say, asks
  DHCP's servers or your own in turn, over DNS over TLS when they
  answer it, and over DNS over HTTPS when you name an `https://` URL
  ([ADR 0193](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0193-resolved-caches-every-programs-lookups-and-asks-over-tls-when-it-can.md)).
- **`timed` keeps the clock right** over Network Time Security, which
  authenticates every answer, and writes the real-time clock; the IANA
  time zone database and English's locales come with it
  ([ADR 0194](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0194-the-clock-is-a-capability-and-timed-sets-it-over-nts.md)).
- **curl and libcurl**, 8.23, on rustls, with zlib and zstd
  ([ADR 0195](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0195-curl-and-libcurl-use-rustls-through-rustls-ffi.md)).
- **`netlog` sends the log to another computer** over UDP as it is
  written, to debug a machine whose screen shows nothing
  ([ADR 0196](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0196-netlog-sends-the-kernel-log-over-udp.md)).
- **`logforward` sends the log to syslog servers** as RFC 5424 messages
  over TLS, TCP or UDP, from the log's own files, keeping its place in
  them for each server so that a server that was away is sent what it
  missed
  ([ADR 0482](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0482-the-log-goes-to-syslog-servers-from-logforward-with-the-log-s-own-files-as-its-queue.md)).
- **IPv6, with nothing to set**: each card makes its link-local address
  and, from what its routers advertise, a stable address that is the
  same on the same network at every boot but tells no network its MAC
  address (RFC 7217), and temporary ones it sends from (RFC 8981), each
  tested before it is used; it takes the routers, DNS servers and MTU
  the adverts carry
  ([ADR 0331](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0331-the-kernel-makes-each-cards-ipv6-addresses-by-slaac.md)).
  netd asks DHCPv6 for addresses or settings when the routers say to
  ([ADR 0335](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0335-netd-asks-dhcpv6-when-routers-say-to.md)).
- **IPv6 sockets** that take IPv4 too unless `IPV6_V6ONLY`, as on
  Linux, and lookups that ask for A and AAAA records and order them as
  RFC 6724 does, so curl, ssh and git reach names with only AAAA records
  ([ADR 0332](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0332-ipv6-sockets-take-ipv4-too-and-lookups-ask-for-both.md)).
- **Bridges and VLANs**, made with iproute2's commands: `ip link add br0
  type bridge`, `ip link set net0 master br0`, `ip link add link net0
  name net0.7 type vlan id 7`; netd sets them up as cards
  ([ADR 0333](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0333-bridges-and-vlans-are-kernel-links-joined-through-each-stacks-hooks.md)).
- **A firewall that keeps what is open in step with what runs**: other
  machines reach what a service listens on while it runs, and what the
  rules allow; nothing else they start gets in, and what is dropped is
  logged. What you run yourself listens for your machine alone until
  `firewall allow program PATH` lets others reach it
  ([ADR 0334](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0334-the-firewall-denies-inbound-unless-a-service-listens-or-a-rule-allows.md)).
- **Finding the machines beside it**: discoverd(8) answers for the
  machine's `NAME.local` by multicast DNS, so `ping studio.local` works
  from the machines that ask it, as macOS, Windows, Linux with Avahi
  and other Dukovo machines do, and finds their
  names, the services they offer by DNS-SD and the devices that answer
  WS-Discovery, which discover(1) lists
  ([ADR 0338](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0338-discoverd-finds-what-the-local-networks-have-with-mdns-and-ws-discovery.md)).
  ping(8) is anyone's, through a ping socket, and multicast works as on
  Linux
  ([ADR 0336](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0336-multicast-groups-packet-info-and-ping-sockets-on-smoltcp.md)).
- **WireGuard**, with wg-quick's files and wireguard-tools' `wg`:
  wgd(8) brings up an interface for each file in
  `/system/state/wireguard` and carries it to its peers, Linux's
  WireGuard among them, on Cloudflare's boringtun
  ([ADR 0340](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0340-wgd-runs-wireguard-on-boringtun.md)),
  through the kernel's tunnels
  ([ADR 0339](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0339-tunnels-are-ip-links-whose-packets-a-program-carries.md)).
- **The administrator's tools**: dig and host, whois, OpenBSD's nc,
  nic for what each card says of its link, as ethtool, openssl and
  iperf3, traceroute and mtr, which run without privilege as on
  Linux, from the ICMP errors the kernel gives sockets, and tcpdump, on
  the kernel's packet captures and their filters
  ([ADR 0341](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0341-dig-host-whois-nc-and-nic.md),
  [ADR 0342](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0342-openssl-and-iperf3.md),
  [ADR 0345](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0345-traceroute-mtr-and-icmp-errors-for-sockets.md),
  [ADR 0346](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0346-packet-capture-libpcap-and-tcpdump.md)).
- **An app reaches the local network only if you let it**: a bundle's
  network.local permission, asked for when it first runs, is a right of
  its own, apart from reaching the internet
  ([ADR 0337](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0337-the-local-right-reaches-the-networks-the-machine-is-on.md)).

Native programs use the sockets through radix-network(2) and
radix-file(2).

## Giving a service the network

A service file asks for what it needs, and gets nothing else:

```toml
service_version = 1
description = "A web server"
program = "/system/commands/httpd"

[permissions]
network.outbound = false
network.listen = [80, 443]
```

Spark derives one capability for each range from its own and puts them
in the service's network slots. The service can bind to and listen on
ports 80 and 443, and nothing more: binding another port, connecting
out or sending a datagram anywhere fails with `EACCES`. Its
capabilities for 80 and 443 have the inbound right too, so other
machines reach it there through the firewall while it runs, and not
once it stops.

## The firewall

With no rules, a fresh machine answers pings and lets other machines
reach only what its services listen on. `firewall status` lists every
socket that listens and who reaches it:

```
$ firewall status
Firewall: 0 rules; 3 packets dropped since the machine started
Other machines reach what services listen on, and what the rules allow; pings are answered.

Listening      Process  Program                          Reached from
tcp 443             41  /system/commands/httpd           other machines (its service)
tcp 8000           212  /users/alice/bin/devserver       this machine only
                        To allow it from other machines: firewall allow program /users/alice/bin/devserver
```

`firewall allow program PATH`, `firewall allow tcp 8080 from
192.168.1.0/24` and `firewall deny udp 5353 on net1` add rules,
`firewall list` numbers them and `firewall remove N` takes one away;
they are kept in `/system/settings/firewall`, and the first that
matches decides. Changing them needs an administrator, through
`elevate`. The kernel logs what it drops, five a second at most:

```
Firewall: dropped a TCP connection from 192.168.1.7:51514 to port 8000 on net0: process 212 listens for this machine alone
```

## Bridges and VLANs

A bridge joins cards into one network, as a switch does, for virtual
machines or a second machine on another port; a VLAN reaches a tagged
network on a switch's trunk port:

```
# ip link add br0 type bridge
# ip link set net0 master br0
# ip link set net1 master br0
# ip link add link br0 name br0.7 type vlan id 7
# cat >> /system/settings/network
[br0.7]
dhcp = no
address = 192.168.7.1/24
address = fd07::1/64
# service netd reload
```

netd asks DHCP for `br0`'s address as it would for a card's, and the
ports have none of their own while they are ports. `ip -d link` shows
what each interface is.

## WireGuard

A file for each interface, in wg-quick's format (wireguard.conf(5)),
and wgd brings it up. Make the keys first, kept from others, and give
the peer the public one:

```
# umask 077
# wg genkey > /system/state/wireguard/wg0.key
# wg pubkey < /system/state/wireguard/wg0.key
9wjCu3Ap0zMQ2K+E+hjKuatuKFDsu29PFtTDfAyPzmQ=
# cat > /system/state/wireguard/wg0.conf
[Interface]
PrivateKey = <what wg0.key holds>
Address = 10.9.0.2/24, fd09::2/64

[Peer]
PublicKey = <the peer's public key>
AllowedIPs = 10.9.0.0/24, fd09::/64
Endpoint = vpn.example.org:51820
PersistentKeepalive = 25
# service wgd reload
# wg show wg0
```

A file from a Linux machine, a router or a VPN provider works as it is;
what wg-quick alone does (DNS, Table and the Up and Down scripts) is
warned of and passed over. `wg set`, `wg syncconf` and the rest change
an interface until wgd reads its file again, and `service wgd reload`
does that without ending the sessions of the peers that did not
change. The interface listens on the file's ListenPort; a port other
than 51820 needs a line in wgd's service too
(`service edit wgd`). Until the routing table comes, AllowedIPs of
`0.0.0.0/0`, which would send everything through the tunnel, do not:
the kernel routes by each interface's subnet.

## The administrator's tools

The tools take Linux's options and print Linux's words, so what you
type and what your scripts read stay the same:

```
$ nic net0
Settings for net0:
	Speed: 1000Mb/s
	Duplex: Full
	Link detected: yes
$ dig example.org MX +short
$ host -t TXT example.org
$ whois example.org
$ nc -zv 192.168.1.10 22
$ iperf3 -c 192.168.1.10
$ openssl s_client -connect example.org:443 < /dev/null
$ traceroute example.org
$ mtr --report example.org
$ elevate tcpdump -i net0 -n port 53
$ elevate tcpdump -i any -w capture.pcap
```

`nic` alone lists each interface's link, speed and driver, `nic -i`
shows a card's driver, model and PCI address, `-g` its rings, `-k` its
offloads and `-S` its counts (nic(8)). dig and host ask the servers in
`/etc/resolv.conf`, the local resolver unless you name one
with `@server` (dig(1), host(1)); whois starts at IANA's server and
follows the registries' referrals (whois(1)). nc is OpenBSD's netcat as
Debian ships it, iperf3 ESnet's, and openssl OpenSSL's command, whose
settings are in `/system/settings/ssl` with the root certificates.
traceroute is Dmitry Butskoy's as Linux distributions have it, by UDP or
with `-I` by ICMP, and `traceroute6` or `-6` traces over IPv6
(traceroute(8)); mtr asks each router on the way again and again and
shows how many answers come back and how soon, on the terminal or with
`--report` once, `-z` with each network's AS number (mtr(8)). Neither
needs privilege: the routers' "time exceeded" errors come back to the
sockets their probes went from.

tcpdump is the Tcpdump Group's, on their libpcap (tcpdump(1)). It prints
what an interface receives and sends, or with `-i any` what every
interface does, each packet's interface and direction before it as on
Linux, picked by a filter such as `port 53` or `icmp6`
(pcap-filter(7)), which the kernel runs itself for one interface; `-w`
writes what it captures to a file that Wireshark and tcpdump `-r` read,
and `-Q in` or `-Q out` keeps one direction. Capturing needs the
network's control right, so run it with elevate(8), as Linux asks for
root. A card is not yet put in promiscuous mode, so tcpdump sees what
its driver lets in, the machine's own frames, broadcasts and the
multicast groups it joined, and says so; `-p` asks for no more.

## Setting the cards up

netd(8) runs on every machine and asks DHCP for each card's address,
router and DNS servers, with nothing to configure. The system's log
says what it got:

```
netd: net0: asking for an address by DHCP
netd: net0 has 10.0.2.15/24 from 10.0.2.2 for 1 day, router 10.0.2.2, DNS 10.0.2.3
```

To give a card an address of its own, name it in
`/system/settings/network` (network(5)), by its name or its MAC
address, and have netd read the file again:

```
# cat >> /system/settings/network
[net0]
address = 192.168.1.10/24
router = 192.168.1.1
dns = 192.168.1.1
# service netd reload
```

`dhcp = no` in a card's section, with no address, has netd leave it
alone. netd changes only the addresses it gave, so one added with ip(8)
stays. netd writes the DNS servers it was given to
`/system/live/netd/resolv.conf`, which resolved asks (see [DNS](#dns)
below); to use DNS servers of your own, give a card `dns =` settings, or
name them in `/system/settings/resolved`.

The `dhcp` boot test has netd set a card up by QEMU's DHCP server,
restarts netd, and changes the settings to an address of their own and
back:

```
./ob test --case dhcp
```

## Giving a card an address at boot

The first card can also take its address from two kernel flags
(dukovo-boot(7)), and netd then leaves it as it is:

```
ip=10.0.2.15/24 gateway=10.0.2.2
```

Add them to `boot.cmdline` in `dukovo.toml`, or pass them with
`./ob run --cmdline`. QEMU's user network, which `ob run` does not add
by itself, hands out 10.0.2.15 and routes through 10.0.2.2. To try it:

```
./ob run --cmdline "selftest=net" -- -netdev user,id=n0 \
    -device virtio-net-pci,netdev=n0,disable-legacy=on
```

The log shows the card, its address and four pings to the router:

```
Net: net0 is a virtio network card at 0000:00:03.0, MAC 52:54:00:12:34:56, 256 receive and 256 transmit slots, MSI-X vector 0x52
Net: net0 has the address 10.0.2.15/24, through the router 10.0.2.2
Net: 10.0.2.2 answered ping 1 in 7.792 ms
selftest net: passed; the router answered 4 pings, the fastest in 1.390 ms
```

QEMU's user network answers pings to 10.0.2.2 by itself, but it sends
a ping to any other machine through the host's unprivileged ping
sockets. Hosts that turn those off, such as Linux Mint, whose
`/proc/sys/net/ipv4/ping_group_range` reads `1 0`, drop every such
ping: `ping 10.0.2.2` answers and `ping dukovo.org` does not, though
DNS, TCP and UDP work. Allow the host's users to ping, then start QEMU
again:

```
sudo sysctl -w net.ipv4.ping_group_range="0 2147483647"
echo 'net.ipv4.ping_group_range = 0 2147483647' | sudo tee /etc/sysctl.d/60-ping.conf
```

`ob test --case net` runs the same test behind QEMU's IOMMU, as CI
does. QEMU's `e1000`, `e1000e`, `igb` and `vmxnet3` cards work the same
way in place of `virtio-net-pci`, and `ob test --case network-cards`
runs them all at once, with the e1000's 82545EM, each on a network of
its own.

## Setting a card up by hand

At the console, root's shell may set a card up with ip(8), as on
Linux; a service may when its file asks for `admin.network = yes`
(spark(8)). With QEMU's user network:

```
# ip address add 10.0.2.15/24 dev net0
# ip route add default via 10.0.2.2
# ip -br address
lo               UNKNOWN        127.0.0.1/8
net0             UP             10.0.2.15/24
# ip route
default via 10.0.2.2 dev net0
10.0.2.0/24 dev net0 proto kernel scope link src 10.0.2.15
```

Each card takes up to four addresses, the first the one it sends from,
and a router. `ip address del`, `ip address flush` and `ip route del
default` take them away again; `ss -tl` lists what listens. The
`interfaces` boot test does the same with the address QEMU's DHCP
server offers, which a socket bound to the bare card asks for:

```
./ob test --case interfaces
```

## DNS

Every program looks names up through resolved(8): the C library's
`getaddrinfo` finds a name in `/etc/hosts` first, which names localhost,
and otherwise reads `/etc/resolv.conf`, which names 127.0.0.53, where
resolved answers. resolved asks the servers DHCP gave unless
`/system/settings/resolved` (resolved(5)) names others, and keeps each
answer as long as it says. `resolvectl` shows what it asks and how, and
looks names up through it:

```
$ resolvectl query dukovo.org
dukovo.org.	300	IN	A	192.0.2.10
-- 1 record in 31 ms
$ resolvectl
Listening on: 127.0.0.53:53 (UDP and TCP)
DNS over TLS: opportunistic
Cache: 112 of 4096 answers
Queries: 431, 302 from the cache, 6 answered here, 0 failed
Server: 10.0.2.3 (from DHCP): plain DNS over UDP; 123 answered, 0 failed
```

A server DHCP gave is asked over DNS over TLS first, on port 853, and
over plain DNS if it does not answer there. To have every name go
encrypted to servers you trust, name them with the name their
certificate carries, or by an `https://` URL for DNS over HTTPS, and
read the file again:

```
# cat >> /system/settings/resolved
servers = 9.9.9.9#dns.quad9.net https://cloudflare-dns.com/dns-query#1.1.1.1
# service resolved reload
```

`localhost` is answered on the machine, and `.local` names never go
to a server: resolved asks discoverd for them (below).
`resolvectl flush-caches` empties the cache.

## The machine's name and the local network

The machine's name is the first line of `/system/settings/hostname`
(hostname(5)), `dukovo` until you give it one. discoverd(8) answers for
it on each network as `NAME.local`, and finds the other machines'
`.local` names for every program's lookups:

```
# hostname studio
# service discoverd restart
$ ping -c 1 printer.local
PING printer.local (192.168.1.7) 56(84) bytes of data.
64 bytes from printer.local (192.168.1.7): icmp_seq=1 time=0.912 ms
```

`discover` lists what the networks offer, by DNS-SD, as printers,
shared folders and other machines do, and what answers WS-Discovery,
as Windows machines do:

```
$ discover
SERVICE  TYPE               AT                 ADDRESSES
laptop   _workstation._tcp  laptop.local:9     192.168.1.12
printer  _ipp._tcp          printer.local:631  192.168.1.7

DEVICE        TYPES                     ENDPOINT               DESCRIBED AT
192.168.1.20  wsdp:Device pub:Computer  urn:uuid:5a1e33c0-...  http://192.168.1.20:5357/5a1e33c0-...
```

An app sees discoverd, and reaches the machines on your networks, only
with network.local, which it asks you for when it first runs
(bundle(1)).

## The time and the time zone

timed(8) sets the clock from the network at boot and keeps it right,
asking Cloudflare's, Netnod's and PTB's servers over Network Time
Security, so that nobody on the way can change the time; it writes the
real-time clock too. `timedatectl` shows the time, the zone and what
timed did, and sets the zone:

```
$ timedatectl
               Local time: Sun 2026-11-01 12:00:00 UTC
           Universal time: Sun 2026-11-01 12:00:00 UTC
                 RTC time: Sun 2026-11-01 12:00:00
                Time zone: UTC (UTC, +0000)
System clock synchronized: yes
              NTP service: active
          RTC in local TZ: no
# timedatectl set-timezone Europe/Berlin
$ date
Sun Nov  1 13:00:00 CET 2026
```

`timedatectl list-timezones` lists the zones, and the `TZ` variable
sets one for a single program, as `TZ=America/New_York date`. Other time
servers, or none, go in `/system/settings/timed` (timed(5)). The
locales are English's in each of its countries, `en_US.UTF-8`,
`en_GB.UTF-8`, `en_IN.UTF-8` and the others, with their dates,
numbers and sorting order; `LANG=en_GB.UTF-8` picks one.

## Fetching with curl

curl(1) and libcurl fetch over HTTP and HTTPS with the system's root
certificates, as on Linux:

```
$ curl -sI https://dukovo.org
HTTP/1.1 200 OK
```

A port links libcurl, a static library, from the sysroot, where
`libcurl.pc` gives pkg-config(1) its flags. HTTP/2, HTTP/3 and
international domain names are to come.

## Sending the log to another computer

A machine whose screen shows nothing can still send its log. On
another computer on the network, listen:

```
nc -klu 6666
```

and boot the machine with `netlog=192.168.1.20` on its kernel command
line (dukovo-boot(7)), or name the target in `/system/settings/netlog`
(netlog(5)) to keep it. netlog(8) sends every record from the start of
boot, each as one line with its priority and time, as a syslog server
takes them. The log goes unencrypted, so send it only across a network
you trust with it. For the log of a machine that runs, to a syslog
server that keeps it, over TLS, and sent again after the server was
away, use logforward(8) (see
[Logs, monitoring and troubleshooting](logs.md)).

## Trying the network services

The `network-services` boot test runs, on `lo`, a DNS server, a DNS
over TLS and an HTTPS server, a Network Time Security server and a
log sink, and checks resolved, resolvectl, curl, timed, timedatectl,
netlog, the time zones and the locales against them:

```
./ob test --case network-services
```

## Trying TLS

The `tls` boot test runs `tls-test`, which checks ring's hashes and
ciphers against answers OpenSSL gave, OpenSSL's signatures, rustls's
handshakes over `lo` with each version, cipher suite, key exchange
group and kind of key, the certificates a client must refuse, and the
system's root certificates:

```
./ob test --case tls
```

## Trying two machines

The `network-pair` boot test boots two machines on a cable: the first
bridges its network and the cable, with VLAN 7 on the bridge, and the
second gets its addresses by DHCP, SLAAC and DHCPv6 through the bridge,
fetches the first's page with curl by IPv6 and IPv4, and probes its
firewall:

```
./ob test --case network-pair
```

The `network-discovery` boot test boots two machines on a cable, names
them `a` and `b`, and checks that each pings the other's `.local` name,
by IPv4 and IPv6, that `discover` lists b's workstation and b as a
WS-Discovery device, and that a ping reaches b with the local right
and not the send right alone:

```
./ob test --case network-discovery
```

The `network-wireguard` boot test boots two machines on a cable, each
with wgd, and checks that b fetches a's page through their tunnel by
IPv4 and IPv6, that they ping each other through it, that the cable
carries only WireGuard's sealed datagrams, that a reload of wgd keeps
the session, and that a refuses b's packets from an address b's
AllowedIPs leave out:

```
./ob test --case network-wireguard
```

The `network-tools` boot test boots two machines on a cable, the first
with an e1000e and the second with a virtio card, and checks nic on
both, nc both ways over TCP and UDP, iperf3 against the second's
server, dig and host against its name server and through resolved,
whois through its referrals, and openssl's keys and certificates, with
the second's s_client reaching the first's s_server over TLS 1.3:

```
./ob test --case network-tools
```

## Trying the sockets

The `sockets` boot test starts a test program with Spark's network
capability (`--network`). The program derives one capability that may
only connect out and one that may only listen on ports 8000 to 8009,
then runs TCP and UDP over `lo` and TCP through a virtio card to an
echo server QEMU runs, and checks that each call refuses what its
capability does not allow:

```
./ob test --case sockets
```

Its log reads:

```
sockets: the network capability covers 65536 ports from 0
sockets: derived one capability to connect out and one to listen on ports 8000-8009
sockets: tcp: connected from port 52311 to port 7000 over lo, and the connection taken
sockets: tcp: 1048576 bytes went one way and came in order
sockets: udp: datagrams went each way over lo, kept their bounds, and said where they came from
sockets: rights: each method refused what its capability does not allow (process 12)
sockets: virtio: a connection to 10.0.2.100 port 7 echoed what went through the card
sockets: every check passed
```
