<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0355. Go's net package lists interfaces from radix-sysfs, and a destination of all zeros is this machine

- Status: Accepted
- Date: 2026-11-21

## Context

Go's network poller runs on the event queue (ADR 0353), so Go's `net`
package works on Radix: its sockets are non-blocking, and its deadlines
hold. The test binaries of `net`, `net/http` and `crypto/tls`, run on
Radix, then failed in four places.

- **Interfaces.** `net.Interfaces` and `net.InterfaceAddrs` listed
  nothing (ADR 0350). Go reads them from netlink on Linux and from a
  routing socket on the BSDs; Radix has neither. It lists its
  interfaces in radix-sysfs, a directory each in `/kernel/class/net`,
  with their addresses in its own `ipv4` and `ipv6` files, which the C
  library's `getifaddrs` reads.
- **A destination of all zeros.** Go dials `":80"`, and a listener's
  `"[::]:80"`, as a connection to 0.0.0.0 or `::`, as the documentation
  of `net.Dial` says: the host empty or all zeros is the local system.
  Linux and the BSDs connect such a socket, and send such a datagram, to
  the machine itself; Linux takes the address the socket is bound to,
  else 127.0.0.1 or `::1`. Radix refused both with `EINVAL`.
- **A connected datagram socket's name.** After `connect`, Linux names a
  UDP socket bound to any address by the address it sends to its peer
  from. Radix named it 0.0.0.0, so a reply sent to that name failed.
- **The tests' own files.** `net`'s `sendfile` tests read Go's shared
  test data, `src/testdata`, which the test disk did not have, and its
  `writev` test expects a call count from each system that has `writev`,
  by name.

## Decision

- **Go's `net` reads Radix's interfaces from radix-sysfs.** A file of the
  Go port's patches, `net/interface_radix.go`, lists the directories of
  `/kernel/class/net` and reads each one's `ifindex`, `mtu`, `address`
  (no hardware address for all zeros, as `lo` has), `flags` (Linux's
  `IFF_` bits, in hexadecimal) and `carrier`, which says whether it is
  running; and its addresses with their prefixes from `ipv4`, and the
  address lines of `ipv6`. It reads them as `getifaddrs` does, with no
  new call into the C library. An interface's multicast addresses are
  not listed: radix-sysfs does not show the groups joined.
- **The kernel takes a destination of all zeros as this machine.** A
  network socket that connects or sends to 0.0.0.0 or `::`, or the
  IPv4-mapped all zeros, goes to the address it is bound to, else to
  127.0.0.1 or `::1` by the address's version, as on Linux
  (`State::destination`). The right it needs is the same: `net::reach`
  already counts all zeros as this machine's own (ADR 0337).
- **A connected datagram socket is named by its source address.** A UDP
  or ping socket bound to any address and connected gives, as its name,
  the address the routes send to its peer from, found when it is asked;
  where it is bound and what it receives do not change.
- **The `go-runtime` boot test runs `net`, `net/http` and `crypto/tls`**,
  25 packages, with Go's shared test data on its disk. Go's own tests
  name Radix among the systems that have `writev`, that have no
  `SOCK_SEQPACKET` sockets (`unixpacket`), and whose interfaces list no
  multicast addresses. The five tests `userspace/go/skips.txt` left out
  until Go had networking run again.

## Consequences

- Go programs list interfaces and their addresses, find an interface by
  name or index, and join IPv4 and IPv6 multicast groups on one.
- Programs in any language that dial a listener at its all-zeros
  address reach it, as on Linux.
- `getsockname` of a connected UDP socket bound to any address takes
  the routes' look and one stack's lock.
- `net.Interface.MulticastAddrs` returns nothing on Radix
  (`docs/deferred.md`).

## Alternatives considered

- **`getifaddrs` from Go.** Go on Radix calls the C library for system
  calls (ADR 0350), and could call `getifaddrs` too. It gives names,
  flags and addresses but no index or MTU, which take more calls, and it
  reads the same files.
- **All zeros as loopback in Go alone.** A branch in Go's `net` would
  leave every other program that connects to all zeros failing, where
  Linux and the BSDs take it; the kernel is where they agree.
- **Fixing the source address at `connect`.** Linux also narrows such a
  socket to receive at that address alone. Radix's datagram socket is a
  part on each interface; narrowing it would move parts on `connect`,
  for no program found that needs it.
