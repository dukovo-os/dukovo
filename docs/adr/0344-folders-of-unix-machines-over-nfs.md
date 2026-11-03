<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0344. Folders of Unix machines: nfs, an NFS 4.1 and 4.2 client served through FUSE

- Status: Accepted
- Date: 2026-11-03

## Context

Phase 13f.4b, the second NFS step: nfsd(8) shares Dukovo's folders (ADR
0343), and this mounts the folders a Linux server, a NAS or another Unix
machine shares; Kerberos comes to both after the keyring (13h). Linux
mounts them with its kernel's client and `mount -t nfs`, whose options
fill people's guides and mount tables. Dukovo's kernel keeps protocols
out: a program serves the folder through `/devices/fuse` (ADR 0226), as
smb(1) serves a Windows share (ADR 0227).

NFS 4.1 (RFC 8881) and 4.2 (RFC 7862) are what servers speak by default
now: one TCP connection to port 2049, no mount protocol, port mapper or
lock manager beside it, and sessions whose reply cache makes a call sent
again safe. NFS 3 needs all three of those and is left out. No Rust
crate is a client of NFS 4.1 with sessions; libnfs is C, and pulls its
own RPC in. nfsd already reads and writes everything NFS 4 says: its
XDR, the protocol's numbers and RPC's records.

## Decision

- **nfs(1)** (userspace/programs/nfs) is the client, in every image:
  `nfs mount [-f] [-o options] server:/path dir` finds the folder and
  mounts it with `mount_fuse`, then serves it in a process of its own in
  the background, or in the foreground with `-f`, until it is unmounted;
  `nfs ls server:/path` lists a folder. A folder is named as mount.nfs
  names one: `server:/path`, `[v6]:/path`, or `nfs://server[:port]/path`.
- **nfsd is a library too.** Its `xdr`, `nfs` and `rpc` modules are the
  client's, so that one reading of the protocol serves both sides, and
  its feature `testing` runs the server in a test's process for the
  client's tests. Nothing else of nfsd is public.
- **The protocol:** NFS 4.2, or 4.1 when the server speaks no 4.2, over
  one TCP connection. EXCHANGE_ID and CREATE_SESSION make a client ID
  and a session of one slot, with no back channel, so the server has no
  way to call back and gives no delegations; opens say they want none,
  and one given anyway is returned at once. RECLAIM_COMPLETE follows,
  which opens need. Each COMPOUND starts with SEQUENCE; those that change
  files ask the server to keep their reply. A connection lost is made
  again and the COMPOUND sent again in the same slot with the same
  sequence number, which the server answers from its reply cache when it
  had carried it out. A lost session is made again, and a lost client
  ID too; opens of an earlier client ID are made again before their file
  is used (the client's generation counts client IDs). DELAY and GRACE
  are waited out, up to three minutes. A thread renews the lease while no
  call does. The mount sends EXCHANGE_ID's owner as its own: each mount
  is a client of its own.
- **Who calls:** AUTH_SYS, the caller's user and group as the kernel
  names them in each FUSE request, and the other groups the group file
  lists the user in, at most 16, as Linux's client sends them. Owners
  the server names by number, as Linux's server does for AUTH_SYS, are
  those numbers; `name@domain` is this machine's account of that name,
  or no one (65534). Owners and modes are the server's, which the kernel
  checks (ADR 0226) and the server checks again. `sec=krb5*` is refused
  until 13h; so is a server that asks for it.
- **The files:** a node stands for a filehandle, which renames leave
  alone, and its number is the server's file number, as Linux's client
  shows it, unless another node has it (another of the server's
  filesystems). Each file the kernel opens is opened on the server by an
  open-owner of its own (CLAIM_FH), so that each close closes one open
  alone; reads and writes go as whoever opened it. Files are made with
  OPEN (GUARDED for O_EXCL, else UNCHECKED), folders, links, FIFOs and
  sockets with CREATE; device nodes are refused, as a mount of a program
  opens none. Writes are UNSTABLE, as Linux's client sends them: each
  file keeps what it wrote until a COMMIT, on fsync, past 16 MiB and
  when the kernel releases the file (when its node goes: ADR 0226),
  finds the server's write verifier unchanged, and writes it again
  with FILE_SYNC if the verifier changed, when the server restarted
  before the data reached its disk. A folder is listed with READDIR when
  it is opened, in pages. What the kernel is told it keeps for a second,
  since other clients change the files too. READ and WRITE carry at most
  1 MiB, or `rsize`, `wsize`, the session's room and the server's
  `maxread` and `maxwrite`, whichever is least.
- **Options** are mount.nfs's: `vers=4.2` or `4.1` (`vers=4` asks for
  either; 2, 3 and 4.0 are refused), `port=`, `addr=`, `hard` (the
  default, as Linux's) or `soft`, `timeo=`, `retrans=`, `rsize=`,
  `wsize=`, `ro`, `rw`, `proto=tcp` and `sec=sys`; those mount.nfs takes
  that change nothing here, the kernel's caching flags among them, are
  taken, so that a line written for Linux mounts. The mount itself waits
  only `retrans` tries for the server, hard or not.
- **mount(8) and umount(8)** run `nfs mount` for `-t nfs`, `-t nfs4` or
  a source `server:/path`, and list such mounts under `nfs` and `nfs4`.
- **Tested** by unit tests on the host, against nfsd served in their
  process: files made, written, read back, split into small WRITEs,
  committed; folders, renames, links and symbolic links; attributes set
  through an open and without one; listings of several pages; errors;
  a read-only mount; and opens made again after the server lost the
  client. The `nfs` boot test mounts a folder the CI runner's own Linux
  server exports, which QEMU's network reaches at 10.0.2.2, and uses it
  with the utilities as a person's number and as root, whom the server
  squashes; the host checks what the guest wrote.

## Consequences

- A folder of a Linux server is a folder every program uses, and the
  lines Linux mounts one with mount it here.
- A fault in parsing what a server sends ends nfs and its mount, not the
  system (ADR 0226).
- **Security:** AUTH_SYS believes the client, as ADR 0343 says of nfsd:
  the server trusts that a call is from the user it names. nfs sends the
  number the kernel gives for the caller, so a person reaches what that
  number may on the server; root is the server's to squash.
- **Not yet:** Kerberos (13h); locks the server and other clients see
  (a lock is this machine's alone, as Linux's `local_lock=all`);
  delegations and a page cache, and more than one call at a time, so a
  large file moves slower than with Linux's client on a fast network;
  NFS 3 and 4.0; pNFS; access lists and named attributes; READ_PLUS,
  COPY and CLONE; RENAME_NOREPLACE; and mounts by people who are not
  administrators.
