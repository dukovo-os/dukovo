<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0348. Passwords and SFTP over SSH: sshd_config, a ticket for each connection, OpenSSH's sftp-server

- Status: Accepted
- Date: 2026-11-03

## Context

Phase 13c, after ADR 0347, which signs people in over SSH with keys
alone. The roadmap asks for passwords too, but only once the person
turns them on, for SFTP and scp, and for sshd_config(5). Linux's sshd
reads `/etc/ssh/sshd_config`, has PasswordAuthentication on unless a
distribution turns it off, checks passwords through PAM in its
privileged monitor, and runs OpenSSH's sftp-server for the `sftp`
subsystem, which scp has used since OpenSSH 9.0.

What ADR 0347 holds must hold still: only authd(8) starts a session,
and it takes nothing on sshd's word alone. A key's signature is handed
to authd again for each session; a password cannot be, unless sshd kept
it for as long as the connection lasts, in memory that Radix may swap
out (no mlock(2) yet), where a taken-over sshd could read it and use
it elsewhere, for elevate too.

## Decision

- **sshd_config(5)** is `/system/settings/ssh/sshd_config`, in
  OpenSSH's format, with the keywords PasswordAuthentication (default
  `no`), PubkeyAuthentication (default `yes`), and AllowUsers,
  DenyUsers, AllowGroups and DenyGroups with `*` and `?` patterns,
  checked in OpenSSH's order before any key or password. The default
  file is all comments. The reader is in the accounts crate, so its
  unit tests run on the host.
- **Any other keyword is a mistake**, as is a value sshd does not take:
  sshd does not start, and a reload keeps the settings it had, with the
  line and why in the log. OpenSSH's sshd would carry such a line out,
  so passing it over could leave open what an administrator meant to
  close (an `AllowUsers alice@192.0.2.7`, a `Match` block).
  `sshd -t` checks the file and `sshd -T` shows what it makes, as
  OpenSSH's do.
- **Reload as OpenSSH's.** The listener reads the file as it starts and
  on SIGHUP, which `service sshd reload` sends, and hands each new
  connection the settings it holds then; open connections go on as
  they were. A restart would end every connection, the one the
  administrator typed it in among them.
- **authd checks the password and answers with a ticket.** The new
  `ssh-password` request carries the connection, the name and the
  password, from root alone. authd checks it as a console's, but counts
  wrong ones for the client's address, as it counts a Windows PC's
  (ADR 0228), so that tries over SSH neither slow down the console nor
  go faster from many connections of one address; a wrong one is
  recorded for lastb(1) on the terminal `ssh`, as Linux's sshd records
  only passwords. An empty password is refused, as OpenSSH's
  PermitEmptyPasswords no. A right one, of a person's account that is
  not disabled, is answered with 32 random bytes, a ticket that starts
  that person's sessions on that connection, asked by that process
  alone, until it sends `ssh-ended` as it ends. sshd wipes the password
  as soon as it is sent. authd keeps 128 tickets, the oldest going
  first, for connections whose end was never said.
- `ssh-session` names its proof: `key` with the signed request and its
  signature, as before, or `ticket`, which authd looks up and checks
  against the connection, the process and the name, and then the
  account as it is now.
- **SFTP is OpenSSH's sftp-server**, built in the OpenSSH port (ADR
  0289) into `/system/libraries/ssh/sftp-server`, where OpenSSH puts
  its libexec programs. The `sftp` subsystem asks authd for a session
  of the new kind `subsystem sftp`, which runs `login --session
  --command /system/libraries/ssh/sftp-server`, through the person's
  shell, as OpenSSH's sshd runs a subsystem. scp uses it; `scp -O` runs
  `scp -t` as a command, as before. No other subsystem is taken.

## Consequences

- An administrator turns passwords on with one line and a reload, and
  can keep SSH to some people or groups, as on Linux.
- A file written for OpenSSH's sshd with keywords Dukovo's does not
  carry out stops sshd until it is trimmed; the log and `sshd -t` say
  which line.
- authd now keeps state for SSH connections: the tickets, at most 128.
  A taken-over sshd can start sessions for the people who signed in on
  its own connections, as it could replay their signed requests with
  keys, and learns the passwords typed on them, as Linux's sshd's
  unprivileged child would; it cannot use a ticket from another
  process.
- Files copied with sftp and scp are the person's, written with their
  permissions and umask, as anything else in their session.
- **Not yet:** port forwarding, gated by the person's network
  permission (ADR 0257), which needs its own way to run connections as
  the person; keyboard-interactive; `Port`, `ListenAddress` and
  `Match`; `name@address` patterns.
