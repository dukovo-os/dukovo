<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0347. Signing in over SSH with keys: sshd on russh, authd checks each signature

- Status: Accepted
- Date: 2026-11-03

## Context

Phase 13c. A computer with no screen, or one in another room, is used
from another over SSH; Linux's sshd is OpenSSH's, which is BSD-licensed
C with its own privilege separation, PAM hooks and a large
configuration. The roadmap chose russh (Apache-2.0), a Rust library of
SSH's protocol for clients and servers, and asks that sign-ins go
through authd(8), keys only unless the person turns passwords on, and
that sshd be off until someone turns it on. The clients are OpenSSH's,
in the image already (ADR 0289).

Two things must hold, as for smbd (ADR 0228) and nfsd (ADR 0343): a
session acts as the person who signed in and no more, which only
authd, holding the names capability, may start (ADR 0250); and sshd,
which parses what any machine on the network sends, never makes anyone
a person by its word alone.

## Decision

- **sshd(8)** (userspace/programs/sshd), in every image, is a Spark
  service, root's, holding port 22, authd's socket and
  `/system/state/sshd`. It is `enabled = false`: port 22 is closed
  until `service sshd enable`, new in service(8), sets `enabled = true`
  in its file and starts it, as `systemctl enable --now` does;
  `service sshd disable` stops it and turns it off again. The same
  helper turns sharing on and off for share(1).
- **russh 0.65** speaks SSH 2 (RFC 4253, 4252, 4254), with ring's
  cryptography and nothing else (`default-features = false, features
  = ["ring"]`): no aws-lc-rs, no RSA crate, no zlib. Each crate it
  brings is under a license deny.toml allows. Key exchange is mlkem768x25519-sha256 or
  curve25519-sha256; the finite-field groups are left out, as they
  cost the server more than the client. Ciphers are
  chacha20-poly1305, AES-GCM and AES-CTR with SHA-2 MACs, russh's
  defaults.
- **A process for each connection.** The listener accepts and starts
  `sshd --connection` with the connection as its standard input, as
  Linux's sshd forks one, so that a crash or a bug in one
  connection's parsing touches no other. The connection
  says on a pipe once someone signed in; until then it counts against
  OpenSSH's own limits: 30 connections signing in, 10 from one
  address, 100 open, two minutes and six keys to sign in.
- **authd checks the signature.** russh verifies the client's
  signature, and sshd asks authd, with a new `ssh-key` request, whether
  the key may sign the person in before the client signs. But to start
  a session, the new `ssh-session` request carries what the client
  signed (RFC 4252 section 7: the session identifier, then the
  request) and the signature, and authd checks them itself: the
  signature against the key the request names (Ed25519, ECDSA P-256 or
  P-384, with ring), that the request names the account, and that the
  key may sign the person in. A patch to russh (ports/rust-crates)
  hands the handler those bytes beside the key it verified, which
  russh keeps to itself otherwise.
- **The key may sign the person in** when theirs is a person's account,
  not disabled, and `~/.ssh/authorized_keys` names the key, as
  OpenSSH's does with StrictModes: authd opens each folder from the
  home down and the file without following links, and passes over a
  file or folder that is not the person's or root's, or that its group
  or others may write; it reads 1 MiB at most. Lines with options
  (`from=`, `command=` and the rest) are passed over and logged: none
  is honoured yet, and taking such a line as a plain key would grant
  more than its owner wrote.
- **Sessions as at the console.** authd starts the session as it starts
  one at a console (ADR 0250): the person's root, view and groups,
  through `login --session`, on the terminal sshd made when the client
  asked for one, or on three pipes. A command the client asks for runs
  as `login --session --command`, which runs the person's shell with
  `-c`. The session gets SSH_CLIENT, SSH_CONNECTION, SSH_TTY on a
  terminal, TERM, and the client's LANG, LANGUAGE and LC_* settings,
  as Debian's sshd passes them; no other variable. A session on a
  terminal is recorded for last(1), from the client's address, and
  shows the last sign-in and the notices as at the console; every
  session is noted in the audit log with the key's fingerprint. authd
  runs 32 SSH sessions at most; a connection holds four.
- **The host key** is Ed25519, made from the kernel's random bytes the
  first time sshd starts, in OpenSSH's format in
  `/system/state/sshd/ssh_host_ed25519_key`, root's and of mode 0600,
  with its public half beside it; sshd logs its fingerprint, which a
  person compares with what their client shows the first time.
- **A client sends only what a session takes.** russh gives a client
  room to send again as each packet comes, so a client may send faster
  than a session reads, and sshd must then hold all of it or stop
  reading the connection, which stops the session's output too, and
  `ssh host cat < big` stops for good. The same patch lets a handler
  give room back itself, as OpenSSH does: sshd gives it as it writes
  to the session, two megabytes at most outstanding on a channel, and
  ends a connection whose client sends more than its window.
- **Refused:** passwords, keyboard-interactive and certificates; RSA,
  P-521 and security keys; SFTP and other subsystems; port forwarding,
  X11 and agent forwarding. The client hears that each is refused.
- **Tested** by host tests of authd's checks (signatures of both kinds,
  requests read strictly, authorized_keys lines and unsafe files) and
  of the requests, and by the `sshd` boot test: the guest turns sshd
  on, and the host's OpenSSH client signs in through a port QEMU
  forwards, runs commands with their output, error, input and exit
  status, and a shell on a terminal; root, another person and a key
  not listed are refused, and the guest's own client signs in too.

## Consequences

- `service sshd enable`, a key in `~/.ssh/authorized_keys`, and the
  computer is reachable with `ssh name@computer`.
- sshd, root's, parses the network's bytes, but cannot start a session
  without a signature authd checks against a key the person listed; a
  bug that skips russh's check does not sign anyone in. What it cannot
  stop: root, or a sshd taken over, could replay a signed request it
  saw, as authd does not see the connection; root needs no SSH to act
  as anyone.
- What russh holds of a connection's keys may be swapped out: Radix
  has no mlock(2) yet, so a patch to russh-cryptovec has it lock
  nothing there, as OpenSSH's sshd locks nothing; mlibc would log each
  call otherwise. The patch goes once Radix locks memory.
- Stopping sshd ends its connections and their sessions, as Spark ends
  what a service leaves behind; Linux's sshd leaves them running
  (KillMode=process). `service sshd restart` over SSH ends the
  connection it was typed in.
- **Not yet:** passwords, which a person turns on (13c, sshd_config(5));
  SFTP and scp; port forwarding gated by the person's network
  permission; authorized_keys options; RSA and security keys, which
  wait for a cryptography library.
