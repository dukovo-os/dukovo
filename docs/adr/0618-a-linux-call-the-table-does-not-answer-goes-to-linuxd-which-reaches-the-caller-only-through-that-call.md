<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0618. A Linux call the table does not answer goes to linuxd, which reaches the caller only through that call

- Status: Accepted
- Date: 2026-11-24

## Context

The kernel's Linux table (ADR 0610) answers the calls that static programs
make, and fails the rest with ENOSYS. A Linux program copied from a
distribution expects more: netlink sockets for `getifaddrs`, a `/proc` and
`/sys` in Linux's text, and the odd call that only some programs make. Much
of that is text and bookkeeping that has no place in the kernel, and a fault
in it should end one program's helper, not the machine. So a program in a
Linux view gets a helper, linuxd (ADR 0621), and the kernel needs a way to
hand it what the table does not answer.

linuxd must then read and write the caller's memory (a `sendmsg`'s buffers,
a `getsockopt`'s answer) and give it descriptors (a netlink socket). A
capability to the caller's address space would reach far more than one call
needs, for as long as linuxd held it.

## Decision

- **The endpoint.** A program may have, in slot `SLOT_LINUX` (57,
  `abi/startup.ridl`), an endpoint linuxd receives on; it stays through fork
  and exec. Without one, nothing changes: the table answers as before.
- **What goes.** A call the table does not know, a `socket` of a family the
  kernel does not have (instead of EAFNOSUPPORT), and the socket calls
  (`bind`, `connect`, `sendto`, `recvfrom`, `sendmsg`, `recvmsg`,
  `shutdown`, `getsockname`, `getpeername`, `setsockopt`, `getsockopt`) on a
  descriptor that is no socket of the kernel's (instead of ENOTSOCK). The
  kernel sends, as a call on the endpoint and without the grant right, a
  message labelled `FORWARDED` with the call's number in word 0 and its six
  arguments in words 1 to 6. The reply's word 0 is the answer, as Linux
  gives it (a value or a negative errno), put in rax as it is; a reply with
  no words is EIO. An endpoint whose receivers have all gone answers ENOSYS,
  as if there were none.
- **The call object.** The thread that received the call takes a capability
  to it with radix-process(2)'s `forwarded_call` (69, version 14), which
  looks at the call that thread holds the reply to: a message anyone else
  sent on the endpoint has no call, and the method fails with `wrong_kind`.
  The object is a new kind, *Linux call* (radix-linux-call(2), interface
  version 1), and its capability has the read, write and control rights,
  never copy or transfer, so linuxd cannot pass it on:
  - `info`: the caller's process, thread, user and group, and the call's
    number.
  - `read` and `write`: at most 64 KiB of the caller's memory, from and to a
    buffer of linuxd's; `read_string`, a NUL-ended string. The kernel walks
    the caller's page tables under its space's lock and copies through the
    direct map, making pages that are not there yet as a fault would; memory
    that is not mapped, or not readable (writable), is `bad_address`, and a
    write to a shared file mapping is refused, since its page would change
    behind the page cache.
  - `give`: copies a capability of linuxd's into the caller's lowest free
    descriptor from `lowest`, below its limit of open files, with
    close-on-exec if asked (`no_room` as EMFILE); `take`: copies the
    caller's descriptor's capability into a slot of linuxd's. Both need the
    copy and transfer rights on the capability they copy. linuxd runs as the
    person whose program it serves, so `take` adds nothing they could not do.
- **A call lasts as long as its thread waits.** The call ends when its
  answer arrives, or the thread stops waiting (it is stopped, or a signal
  takes the call out of the endpoint's queue before linuxd received it, by
  the native restart rules). From then on every method fails with `gone`,
  even while linuxd still holds the capability. A call linuxd has received
  runs to its answer, as an IPC call's reply and a FUSE request do (ADR
  0226); linuxd never waits inside a call, so it answers at once.

## Consequences

- linuxd serves netlink, and any call the table leaves out, without a line
  of it in the kernel, and reaches a process only while one of its calls is
  in hand, and only that process.
- A forwarded call costs an IPC round trip and a capability, about the cost
  of a FUSE request; the bench's `lx_forwarded_null` row measures it.
- A signal does not cut short a call linuxd has received, and a call that
  would wait on Linux (a blocking `recvmsg` on a netlink socket) answers
  EAGAIN or what it has; rows of docs/deferred.md.
- A thread has at most one forwarded call at a time, since a thread makes
  one call at a time.
