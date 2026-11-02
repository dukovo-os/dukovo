<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0341. Sharing folders over NFS 4.2: nfsd, served as each person, to the networks a share names

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 13f.4. Linux and the other Unix systems share folders with each
other over NFS, and mount them with `mount -t nfs4`; ADR 0228 shared
Dukovo's folders with Windows PCs over SMB, and this shares them with
those machines in their own protocol. Linux's server, knfsd, is in its
kernel, which Radix keeps protocols out of; NFS-Ganesha, the server in
user space, is LGPL-3.0 C with its own RPC library and far more than a
home needs (pNFS, clustering, many back ends). unfs3 speaks only NFS 3,
whose locks need two more protocols.

NFS 4.1 (RFC 8881) and 4.2 (RFC 7862) are one protocol on one port,
with sessions, which make a request sent twice safe, and locks and
opens in the protocol itself. Every client that matters speaks 4.1 or
4.2: Linux's since 2.6.32, the BSDs', macOS's. Its usual security,
AUTH_SYS, is a user and group number the client states and the server
believes; Kerberos (RPCSEC_GSS) proves them, and comes after (13f.4c).

As for smbd, two things must hold: each call does what that person may
do and no more, as at their own shell, which a server running as root
and checking modes itself would get wrong somewhere; and root on a
client is not root here.

## Decision

- **nfsd(8)** (userspace/programs/nfsd), in every image, is a server of
  NFS 4.1 and 4.2 over TCP, port 2049, written from RFC 8881, RFC 7862,
  RFC 5531 (RPC) and RFC 4506 (XDR), with no dependency but libc and
  the tree's own manifest and accounts crates. NFS 4.0, 3 and 2 are
  refused (PROG_MISMATCH, or MINOR_VERS_MISMATCH for 4.0).
- **To the networks a share names.** A share of shares.toml(5) is
  served over NFS only with `nfs = ["192.168.1.0/24", ...]`, an address
  and prefix or one address, at most 64; `share add --nfs NETWORK`
  writes it. A connection from an address that no share names is
  closed; the root of the server lists, and LOOKUP finds, only the
  shares the client's address may reach, by name in any case.
- **On only when asked.** nfsd.toml is `enabled = false`: port 2049 is
  closed until `share nfs on` sets `enabled = true` and starts it, and
  `share nfs off` stops it and turns it off again. It is apart from
  `share on`: SMB and NFS are turned on each by itself.
- **Two halves.** `nfsd`, the *listener*, is a Spark service of its own,
  root's, holding port 2049 and authd's socket and nothing of the tree
  beyond what every service reads. It answers RPC and COMPOUNDs, and
  keeps the clients' state: their records, sessions and slots, with a
  replay cache, opens and their share reservations, POSIX byte-range
  locks, and stateids. It touches no file: for each lookup, read,
  write, create, rename or remove it asks a *worker*, over two pipes,
  with a small request and answer of its own (ipc.rs). Not a socket
  pair: Radix sends no socket with a socket's bytes (ADR 0091).
- **Each call as the person.** authd starts `nfsd --worker` at the
  listener's `nfs-worker` request, with the worker's ends of the pipes
  and the log, *as the person the call's number is*, with their groups,
  if it is a person's account and not disabled; any other number, a
  service's included, as no one (65534). The listener serves root (0)
  and AUTH_NONE as no one before anything else, as knfsd's
  `root_squash` does. The person's groups are Dukovo's, not the ones
  the call names. One worker for each person, at most 64, the one
  called longest ago ended past that; the kernel decides what each
  call may do.
- **A view of the shares alone.** A worker's root is not the system's:
  authd builds a view holding each share's folder at `/.nfs-` and the
  share's key (FNV-1a of its name in lower case), opened as root, so a
  share is reached as knfsd reaches an export, whatever its ancestors'
  modes, and nothing else is reached at all: no `..` and no symbolic
  link leads out of the shares, nor from one share into another, which
  may be read-only or kept from the client's network. A change to
  shares.toml is read within a second, and the workers are started
  again for the shares as they are.
- **Handles that last.** A filehandle is the share's key, the file's
  device and inode number: it lasts through renames and nfsd's
  restarts (FH4_PERSISTENT). The listener remembers each file's folder
  and name where it saw it, up to 2^18 of them; one it has not seen is
  looked for in its share, as the person, once in five minutes at most
  for a handle not found, so that a made-up handle costs one search. A
  file removed is NFS4ERR_STALE.
- **Limits**, against a network that is not friendly: 64 connections,
  512 client records, 128 sessions, eight of them a client's, 64 slots
  a session, answers of at most 8 KiB kept for replay (a larger one
  asked to be kept is refused with REP_TOO_BIG_TO_CACHE), 128
  operations in a COMPOUND, a record of at most a megabyte and 64 KiB,
  and a connection idle 15 minutes closed. Leases are 90 seconds; a
  client silent for twice that loses its state.
- **Not done, and said so.** No delegations and no backchannel (the
  session is made without one, and nfsd never needs to call back); no
  pNFS; no grace period: nfsd keeps its state in memory, and after a
  restart a reclaim is refused with NO_GRACE and the client opens and
  locks afresh. Owners and groups are numbers, as Linux sends them with
  AUTH_SYS; a name is BADOWNER. No NFS access lists; the change
  attribute is the ctime; COPY, CLONE, ALLOCATE, READ_PLUS and the
  extended attributes are NOTSUPP, which clients fall back from. The
  listener answers one COMPOUND at a time.
- **Tested** by nfsd's unit tests on the host, COMPOUNDs against a
  worker in the same process: sessions and replays, files made, written,
  read and closed, folders, renames, locks between owners, read-only
  shares and other networks refused, handles through a restart, root
  and calls without credentials served as no one, and the bounded
  replay cache and sessions; and by the `nfsd` boot test, where the
  host's own kernel mounts the guest's shares through a port QEMU
  forwards, lists the root, and as alice's number reads, writes,
  renames, links, locks and copies a large file it reads back after
  mounting again, while root is refused a write and the read-only
  share refuses one. During the work, Linux 6.8's client in a QEMU
  guest did the same and more against the listener on the host.

## Consequences

- A Linux machine mounts `dukovo:/Music`, and its people's files stay
  theirs, with their modes, when their numbers are the same on both.
- AUTH_SYS believes the client: root on a machine of a network a share
  names may say any person's number and act as them in the shares
  (not as root, nor as a service). share(1), shares.toml(5) and
  nfsd(8) say so, and a share names no network until asked;
  Kerberos (13f.4c) is the cure.
- A bug in a worker acts only as the person, in the view of the shares;
  the listener, root's, parses RPC and XDR in bounded records but opens
  no file.
- **Not yet:** Dukovo as an NFS client (13f.4b); Kerberos (13f.4c);
  delegations, a grace period and state kept through restarts; COPY and
  CLONE on the server; and COMPOUNDs answered in parallel, which the
  benchmarks will ask for.
