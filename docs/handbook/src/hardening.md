<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Hardening a server

This chapter is for the person who runs a Dukovo computer that other
machines reach: a web server, a file server, a machine in a cloud. It
says what Dukovo already does, then the steps that remain, in the order
to take them. Each step says why, gives the command, and says how to
see that it worked. The full references are dukovo-check(8), firewall(8),
sshd_config(5), blocking.toml(5) and updates.toml(5).

## What Dukovo does by default

A new installation is meant to have nothing to warn about, so that a
warning is real when one comes:

- **The firewall lets nothing in that nothing asked for.** Other
  machines reach what a service listens on, while it runs, and nothing
  a person runs: a program you start listens for this machine alone
  until you allow it with `firewall allow program PATH`
  ([Networking](networking.md#the-firewall)).
- **SSH is off,** and port 22 with it, until you turn it on. When it is
  on, keys sign in and passwords do not, and root never signs in at
  all: an administrator signs in as themselves and uses `elevate`
  ([SSH and remote access](ssh.md)).
- **Passwords are slow to guess.** They are kept as Argon2id hashes in a
  file only root reads, a wrong password waits before it is answered,
  and an address that keeps guessing is blocked from the sign-in ports
  for a while ([SSH and remote access](ssh.md#blocking-addresses-that-guess-passwords)).
- **Services see only what their files name.** Each has its own view of
  the files and the network, so a flaw in one does not reach the rest
  ([Running services](services.md#what-a-service-can-reach)).
- **Updates are signed and have a way back.** The system keeps two
  slots, checks every file the update server sends against the root it
  trusts, and falls back to the slot that worked if an update does not
  prove itself ([Updates](updates.md)).
- **Snapshots are made each day** of `/users`, `/shared` and
  `/system/settings`, so a deleted or damaged file comes back.
- **Security events are kept for a year** in the audit log: each
  command run with `elevate`, wrong passwords, and each address blocked
  ([Logs](logs.md#the-audit-log)).

## 1. Run dukovo-check

`dukovo-check` looks at what the rest of this chapter is about, and
says for each thing whether it passed, gave a warning, or could not be
looked at. It changes nothing. Run it now, and again after each step:

```
elevate dukovo-check
```

A warning has an ID, such as SEC-110, with a page in the support center
that says what the line means and the steps to take, and one sentence
under the line says the first of them. The service `dukovo-check` runs
it each morning and keeps the result, so the web console and
`dukovo-check --last` have one when nobody ran it. Its exit status is 0
when every check passed, and 2 when none failed and one warned, so a
script can read it.

## 2. Let SSH take keys only, from the people who need it

If you turned passwords on for SSH, turn them off once your key is in
`~/.ssh/authorized_keys`; a key cannot be guessed. Name who may sign in
at all, so that no account nobody uses is open to a guesser:

```
$ elevate nano /system/settings/ssh/sshd_config
PasswordAuthentication no
AllowGroups admins
$ sshd -t && elevate service sshd reload
```

`AllowGroups admins` lets administrators in and no one else; name
another group for the people who need a shell but no `elevate`. `sshd
-t` checks the file before sshd reads it, and a mistake keeps the
settings it had. If a server must take passwords, keep `AllowGroups` or
`AllowUsers` anyway: `dukovo-check` warns with SEC-111 when passwords
sign in and neither is set. If SSH is not needed, `elevate service sshd
disable` closes port 22.

To see it worked, `dukovo-check` passes the lines for `passwords` and
`who may sign in`.

## 3. Name the networks that must never be blocked

An address that gives 5 wrong passwords in 10 minutes is blocked from
the sign-in ports for 10 minutes, for twice as long each time it comes
back within a day, up to a day. That stops a guesser, and would stop
your office too if one person mistyped a password five times from a
network shared by everyone there. Name the networks you trust:

```
$ elevate firewall never-block 192.168.1.0/24
firewall: 192.168.1.0/24 is never blocked
```

While an administrator is signed in over SSH, their own address is never
blocked either, and the console and the serial port cannot be. If a
person is blocked anyway, `elevate firewall blocked` shows who and why,
and `elevate firewall unblock ADDRESS` lets them in at once. The numbers
are in `/system/settings/blocking.toml`; leave `on = true`, which
`dukovo-check` checks (SEC-112).

## 4. Choose when updates install

By default the machine checks for updates once a day and tells whoever
is signed in, and changes nothing until an administrator runs `elevate
update apply`. A server nobody watches is better off taking each update
by itself and restarting at an hour that suits its users, its update
window:

```
$ elevate nano /system/settings/sysupdate/updates.toml
unattended = true
restart = "04:00"
```

The machine stages each update by itself and restarts into it at 04:00.
The update has to prove itself before it is kept, and the machine falls
back to the version it had if it does not
([Updates](updates.md#trying-and-proving)). To keep a version for a
reason, `update hold` holds it; a held version gets no security fixes,
so hold it only for as long as you must. `dukovo-check`
warns with SEC-130 when a release is waiting and SEC-131 when the update
server was not asked for a week.

## 5. Keep snapshots and make backups

Snapshots live on the same disk as what they keep, so they save you
from a mistake and not from a failed disk or a stolen machine. Make
backups to another place, encrypted, and keep the recovery key
somewhere other than the computer:

```
$ elevate backup add stick /volumes/BACKUP
$ backup list
```

Servers that keep a database name a command in their service file that
brings it to a state it can start from again, before the snapshots and
the backups are made (service.toml(5)). The `dukovo-check` line for
`snapshots` warns with SEC-160 if the snapshots are off or leave out
`/users` or `/shared`. See [Updates, snapshots, backups and
recovery](updates.md#snapshots).

## 6. Send the log somewhere else

An intruder who gets in can clear the log of the machine they are on. A
copy on another machine, sent as it is written, stays. Name your syslog
server in `/system/settings/logforward.toml`, over TLS, and test it:

```
$ elevate service logforward reload
$ elevate logforward test
```

See [Sending the log to a syslog server](logs.md#sending-the-log-to-a-syslog-server).

## 7. Be told early, by email

The early warnings, such as a disk filling, a failing disk or a
certificate that ends soon, reach the screen and the sign-in. Nobody
reads those on a server. Set up email, or a webhook for a phone, so
that alerts reach you away from the machine, and send a test:

```
$ elevate alerts test
```

See [Alerts](alerts.md) and [Setting up alert email](alert-email.md).
`dukovo-check` lists each early warning that is active.

## 8. Keep certificates current

A certificate in `/system/settings/ssl` that ends is an outage for
whoever uses it. The early warnings say so 14 days before, and critical
3 days before; `certificate-days` in `/system/settings/metrics.toml`
changes the 14. Dukovo does not renew certificates by itself yet: copy
in the new one from your certificate authority before the warning
comes, and reload the service that uses it
([Running services](services.md#https)).

## 9. Keep off what you do not use

Each open port is a thing to keep current. `dukovo-check` lists, in its
line for `reachable`, the ports other machines reach, and `firewall
status` says which program listens on each. For each one you do not
need, turn its service off, which closes its port:

```
$ elevate service NAME disable
```

Allow a port from the networks that need it, not from everywhere:
`elevate firewall allow tcp 8080 from 192.168.1.0/24`. `dukovo-check`
warns with SEC-101 when a rule lets every port in from everywhere.
Give each person their own account, and take the password off none:
SEC-120 warns of an account with an empty password.

## What is on the way

Signing in to the web console with a passkey or an authenticator app's
code belongs after step 3, and this chapter gains it with the console.
