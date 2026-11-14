<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# SSH and remote access

This chapter describes signing in to a Dukovo computer from another one
over SSH: sshd, Dukovo's own SSH server, built on the
[russh](https://github.com/Eugeny/russh) library, and the OpenSSH client
that is already in the image. The full references are sshd(8),
sshd_config(5), authd(8), ssh(1), sftp(1) and scp(1).

## What works

- **Signing in with a key**, from any computer with an SSH client:
  OpenSSH's on Linux, macOS and Windows, or any other. A shell, a
  command with its output, errors and exit status, or a terminal of the
  client's size, as on Linux
  ([ADR 0347](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0347-signing-in-over-ssh-with-keys.md)).
- **Passwords**, once an administrator turns them on; they are off
  until then, so keys alone sign in
  ([ADR 0348](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0348-passwords-and-sftp-over-ssh.md)).
- **Copying files** with `sftp`, `scp` and `scp -O`.
- **Forwarding ports** both ways, with `ssh -L`, `-D`, `-W` and `-R`
  ([ADR 0349](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0349-port-forwarding-over-ssh-through-a-forwarder-run-as-the-person.md)).
- **Who may sign in**, and how, set in `/system/settings/ssh/sshd_config`
  in OpenSSH's own format.

Each session runs as the person who signed in, with their groups and
permissions, as one at the console does: sshd itself holds port 22 and
nothing of anyone's files. authd(8) checks every key's signature and
every password itself, and starts each session as the person; sshd
never sees a password after it passes it on.

## Turning it on

sshd is off until an administrator turns it on, and port 22 is closed
with it:

```
$ sudo service sshd enable
sshd is running, and starts at each boot.
```

`sudo service sshd disable` turns it off again. While sshd runs, the
firewall lets other machines reach its port 22, as it does any
service's ports (see [Networking](networking.md)).

## The computer's key

The first time a client connects, it shows the fingerprint of the
computer's own key and asks whether to trust it. sshd makes that key
the first time it starts, and logs its fingerprint each time it starts,
in a line `log -s sshd` shows: `sshd: made the host key SHA256:…`, or
`sshd: the host key is SHA256:…`. ssh-keygen shows it too:

```
$ ssh-keygen -lf /system/state/sshd/ssh_host_ed25519_key.pub
256 SHA256:UjbA+QmmjWqgVWnAZl0NwRHJzSdXGFQJ3JCBKqLWvoA root@dukovo (ED25519)
```

Compare it with what the client shows before you answer yes. Removing
the key has sshd make a new one as it starts again, which every client
that knew the old one then warns about.

## Signing in with a key

A key is a pair of files on the computer you sign in from. Make one
there, if you have none, with OpenSSH's client:

```
$ ssh-keygen -t ed25519
```

Its public half, `~/.ssh/id_ed25519.pub` on that computer, goes into
`~/.ssh/authorized_keys` on the Dukovo computer, one key a line. Since
passwords are off at first, put the first one in at Dukovo's own
console, from a USB stick for example, which appears under `/volumes`
by its label (see [Files](files.md)):

```
$ mkdir -p ~/.ssh
$ cat /volumes/STICK/id_ed25519.pub >> ~/.ssh/authorized_keys
```

Then, from the other computer:

```
$ ssh alice@dukovo
```

Once one key signs you in, the same `cat` over SSH adds others:
`ssh alice@dukovo 'cat >> ~/.ssh/authorized_keys' < new_key.pub`.

Keys of the kinds `ssh-ed25519`, `ecdsa-sha2-nistp256` and
`ecdsa-sha2-nistp384` sign in; RSA keys, P-521 keys, security keys and
certificates do not yet. As OpenSSH's sshd checks, `authorized_keys`,
`~/.ssh` and your home must be yours, and neither your group nor
others may write them;
otherwise no key in the file signs you in, and the log says why. A line
with options before its key, such as `from=` or `command=`, is passed
over, since sshd honours none of them yet.

## Passwords and who may sign in

An administrator turns passwords on in sshd_config, checks the file,
and has sshd read it again:

```
$ sudo nano /system/settings/ssh/sshd_config
PasswordAuthentication yes
$ sshd -t && sudo service sshd reload
```

A wrong password waits as one at the console does, longer with each from
the same address, up to a minute; `lastb` shows it, and the person is
told at their next sign-in. Too many from one address block the address
for a while ([below](#blocking-addresses-that-guess-passwords)). An
empty password never signs anyone in over SSH. Root never signs in over SSH at all: an administrator signs
in as themselves and uses `elevate` or `sudo`, which asks on the
terminal.

`AllowUsers`, `AllowGroups`, `DenyUsers` and `DenyGroups` say who may
sign in, with a key or a password:

```
AllowGroups admins
DenyUsers test*
```

A keyword sshd does not carry out, such as `Port` or `Match`, is a
mistake rather than a line passed over, so a line meant to keep people
out never does nothing: `sshd -t` names the line, sshd does not start
with it, and a reload keeps the settings it had. `sshd -T` shows every
setting the file makes.

## Blocking addresses that guess passwords

Waiting slows a guesser down, but a program can guess at many accounts
at once, from one address, for as long as it likes. So authd(8) counts
the wrong passwords that come from each address, whatever account they
name, and blocks an address that keeps guessing. This is on from the
first start, and needs nothing set up (ADR 0479).

By default, 5 wrong passwords from one address within 10 minutes block
it for 10 minutes. If it comes back and guesses again within a day, the
next block lasts twice as long, and so on up to a day. A right password
from the address clears its count. Counted as one wrong password each
are a wrong sharing password, which a Windows PC types when it signs in
to the shared folders, and an SSH connection that tried to sign in,
was refused every time, and ended. An IPv6 address counts as its whole
network of 64 bits, since one computer holds all of it, so a guesser
cannot step through the addresses of its own network.

A block closes the ports people sign in at, and only those: port 22 for
SSH, and the ports of the sharing service and the web console. Other
machines still reach the rest of the computer, so a phone network that
shares one address among many people does not lose a web site because of
one guesser on it. The firewall drops the new connections before any of
its rules. A connection that was open already is not cut, but its next
password is refused unread, and sshd ends it.

These addresses are never blocked: the computer itself, the networks
you name, and, while the session lasts, the address an administrator is
signed in from over SSH, so that a mistyped password cannot lock out the
person who is fixing the computer. The console and the serial port are
not on the network, so they cannot be blocked: an administrator signed
in there can always lift a block.

An administrator sees the blocks, lifts one and names the networks that
are never blocked with `firewall`:

```
$ elevate firewall blocked
Blocked            Until  In a row  Why
203.0.113.9        14:32  2         5 wrong passwords over SSH in 2 minutes, for root, admin and test
2001:db8:4:7::/64  14:05  1         5 refused SSH sign-ins in 4 minutes
$ elevate firewall unblock 203.0.113.9
firewall: 203.0.113.9 is no longer blocked
$ elevate firewall never-block 192.168.1.0/24
firewall: 192.168.1.0/24 is never blocked
```

`never-block remove NETWORK` takes a network out of the list again. Each
block is a line in the audit log, `log -s authd`, with its reason, and
each administrator's next sign-in is told how many addresses were
blocked since their last.

The numbers are in `/system/settings/blocking.toml`, which holds these
defaults. authd reads the file again when it changes, so there is
nothing to restart:

```
on = true
tries = 5
within = "10m"
first = "10m"
longest = "24h"
never = []
```

`dukovo-check` warns when `on` is `false`. The references are
blocking.toml(5), firewall(8) and authd(8); [Hardening a
server](hardening.md) says where blocking fits among the other steps.

## Copying files

`sftp`, `scp` and `scp -O` copy files there and back as the person who
signed in, with their permissions:

```
$ scp report.pdf alice@dukovo:documents/
$ sftp alice@dukovo
```

The SFTP server is OpenSSH's `sftp-server`, which sshd runs as part of
your session.

## Forwarding ports

A client may forward ports both ways, as OpenSSH's sshd lets it:

```
$ ssh -L 8080:localhost:80 alice@dukovo
$ ssh -D 1080 alice@dukovo
$ ssh -R 9000:localhost:3000 alice@dukovo
```

With `-L`, `-D` and `-W`, each connection is made from the Dukovo
computer to where the client names. With `-R`, a port on the Dukovo
computer listens, and each connection to it goes back through the
client. Either way the connections are yours: a forwarder that authd
starts as you, with the network your own programs have, makes them and
listens. So a port below 1024 is refused, as it is to anyone but root
on Linux, and the firewall treats what is forwarded as it treats your
programs.

A port forwarded with `-R` listens for the Dukovo computer's own
programs alone, on 127.0.0.1 and ::1. To let other machines reach it,
an administrator sets `GatewayPorts yes` (or `clientspecified`) in
sshd_config, and lets them through the firewall:

```
$ sudo firewall allow program /system/commands/sshd
```

`AllowTcpForwarding` in sshd_config turns forwarding off, or one way
only: `local` for `-L`, `-D` and `-W`, `remote` for `-R`, or `no`.
As OpenSSH's manual says, this keeps nothing from someone who can run
their own programs on the computer; it is there for administrators who
want it anyway.

## Signing in to other computers

The OpenSSH client in the image signs in to other computers as on
Linux: `ssh`, `scp`, `sftp`, `ssh-keygen`, `ssh-agent` and the rest. It
is built without OpenSSL, so its own keys are Ed25519 keys
([ADR 0289](https://github.com/dukovo-os/dukovo/blob/main/docs/adr/0289-the-openssh-client.md)).
A key's passphrase, asked for once, is kept in your keyring, and
`ssh-add`, `ssh` and `ssh-keygen` take it from there
([Your keyring](keyring.md)).

## Letting a helper in

A person who needs help can let someone with no account sign in as `help`,
with a one-time code, and work in their own screen while they watch
(`help-session start`). `help` is no account, and no account may be named
it; sshd offers passwords for it only while a help session waits for its
code, even when `PasswordAuthentication` is `no`, and for no other name. A
helper gets one terminal and one shell, no commands, no SFTP and no
forwarding. Wrong codes count against the address as wrong passwords do.
[Getting help from someone](getting-help.md) tells the owner's side and
the helper's.

## When the system will not start

When a system that had SSH on fails to start, Rescue turns SSH on too, so
that its administrators can repair it over SSH from where they reached it
before (ADR 0540). It signs in the same administrators with the same keys,
by the same `AllowUsers`, `AllowGroups`, `DenyUsers` and `DenyGroups`, and
with passwords only if `sshd_config` let them in; the computer has the same
host key, so `ssh` shows no warning; and addresses that guess are blocked
as above. What an administrator gets is Rescue's menu, as root, and not a
shell of their own: there is no command to run, no `sftp` or `scp`, and no
forwarding in Rescue. A system that had SSH off stays off in Rescue.
[Updates, snapshots, backups and recovery](updates.md#ssh-in-rescue) says
what Rescue does and shows.

## When something goes wrong

sshd logs each connection: who signed in, from where and with which
key, what was refused and why, and what each forwarded port does.
`log -s sshd` shows it to an administrator, in lines such as these:

```
sshd: alice signed in from 192.168.1.20 port 51674 with key SHA256:rhRwCiraerdriJCRauMpXAXPW9i22+Bgu8u7KL1gJMo
sshd: no one signed in from 192.168.1.20 port 51690: key SHA256:u/Neh65Oe1HIDfpdCK1TdU5alzz3bF3sk7bL+K1IObA for root: there is no person's account root
```

`last` lists the sessions people had on a terminal, with the address
each came from, and `lastb` the passwords that were wrong. `ssh -v` on
the client says what it tried.

## Limitations

- Options in `authorized_keys`, RSA keys and certificates.
- SFTP in Rescue, to copy files off over the network.
- X11 and agent forwarding, forwarding Unix sockets, and
  `PermitOpen` and `PermitListen`.
- `Match` blocks and the other keywords of OpenSSH's sshd_config.
- Kerberos sign-in, which may come after Kerberos for network file
  systems (13h).
