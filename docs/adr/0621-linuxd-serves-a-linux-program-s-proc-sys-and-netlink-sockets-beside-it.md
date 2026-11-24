<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0621. linuxd serves a Linux program's /proc, /sys and netlink sockets, beside it

- Status: Accepted
- Date: 2026-11-24

## Context

A Linux program reads `/proc` and `/sys` in Linux's text: glibc's
`get_nprocs` reads `/sys/devices/system/cpu/online`, Go's runtime reads
`/proc/self/maps` and the cgroup files, Node and Python read
`/proc/cpuinfo` and `/proc/meminfo`, and `getifaddrs`, Go's
`net.Interfaces` and Node's `os.networkInterfaces` ask a netlink route
socket for the interfaces and their addresses. Dukovo's kernel keeps the
same facts in its own words, under `/processes` and `/kernel`
(radix-proc(5), radix-sysfs(5)). ADR 0618 gives the kernel a way to hand a
program's other calls to a program; this record is that program.

## Decision

- **One linuxd per started program.** `linux run` (ADR 0622) starts a
  linuxd beside each Linux program it starts, as the person, so that a fault
  in it reaches one program, not every Linux program of theirs, and holds
  the endpoint and the FUSE connection, so that it starts linuxd again
  (`linuxd serve --again`) if it ends, without the program noticing. Spark
  starts services and has no sessions, so a linuxd per session would have
  needed a supervisor of its own anyway.
- **Calls.** Three worker threads receive on the endpoint (ADR 0618), each
  taking the call's capability, answering and replying in one system call.
  A handler never waits: a call that would wait on Linux answers as a
  non-blocking one does. It answers `socket(AF_NETLINK, ...)`, the socket
  calls on its netlink sockets, and `personality` (the Linux persona, and
  no other: setarch(8) says it could not); any other call is ENOSYS, as
  before.
- **The filesystem.** linuxd serves, over FUSE (fuser, as Dukovo patches
  it, with a session that can start as already initialized for a restart),
  a root holding `proc`, `sys` and `.netlink`, which `serve_fuse` (ADR
  0619) gives the launcher to attach.
  - `/proc`: `self` and `thread-self` (links to the asking process, from the
    request), and for each process `/processes` lists: `cmdline`, `comm`,
    `environ` (empty: the kernel keeps no environment), `status`, `stat`,
    `statm`, `maps` (from the kernel's `/processes/<n>/maps`, in Linux's
    format), `mounts` and `mountinfo` (the sandbox's, as the launcher
    wrote them), `cgroup`, `limits`, `oom_score_adj`, `exe`, `cwd`, `root`,
    `fd/` and `task/<n>/`; and `cpuinfo`, `meminfo`, `stat`, `loadavg`,
    `uptime`, `version`, `filesystems`, `swaps`, `net/`, and the `sys/`
    values programs read (`osrelease`, `pid_max`, `somaxconn`,
    `ip_local_port_range` and the rest). Each text is Linux's proc(5)
    format, made from the kernel's files when the file is opened, and again
    at each read from its start.
  - `/sys`: the CPUs (`online`, `possible`, `present`, the topology), the
    network interfaces (`class/net/<if>/`), a read-only cgroup v2 root
    (`cpu memory pids`, no limits), and transparent huge pages' setting.
  - Node numbers are a function of the path, so a restarted linuxd answers
    the numbers the kernel already holds. What is per process is never
    cached by the kernel (timeouts of 0); the rest for a second.
  - Every file is read as the person: what the kernel will not show them
    under `/processes`, such as another person's `maps`, linuxd cannot show
    either, and its file answers that error.
  - The kernel answers a program's own `exe`, `cwd`, `root` and `fd/<n>`
    itself (ADR 0620); linuxd's are links to paths, for other processes,
    from the `program` and `directory` lines of `/processes/<n>/status` and
    from `/processes/<n>/files`.
- **Netlink.** A netlink socket is a file of the filesystem,
  `.netlink/<id>`, which linuxd opens and gives the program (ADR 0618's
  `give`); the kernel forwards the socket calls on it, which are not its
  sockets. It speaks NETLINK_ROUTE to the kernel alone: RTM_GETLINK,
  RTM_GETADDR and RTM_GETROUTE dumps, and one interface by number or name,
  made from `/kernel/class/net` and `/processes/net`; a request that would
  change the network is EPERM (EOPNOTSUPP for root, as Linux checks the
  right first), any other NLMSG_ERROR with EOPNOTSUPP. A dump is cut into
  datagrams of Linux's size (3,776 bytes, more for a larger buffer), since
  glibc and Go read with 4 KiB buffers and refuse a cut datagram. A receive
  with nothing queued is EAGAIN, as nothing can arrive later. The request
  parser is fuzzed (`linuxd-netlink`).

## Consequences

- A program copied from a Linux distribution finds what it reads at start
  in Linux's words; the boot test `linux-view` and the lab test check it on
  Dukovo, the host tests each text against Linux's format.
- No netlink event is ever sent, so a program that waits for a change of
  the network (NetworkManager's clients, `ip monitor`) waits for ever;
  `/proc/sys` cannot be written; the cgroup files show no limits; rows of
  docs/deferred.md.
- linuxd costs a process and a few threads per running Linux program.
