<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# SSH and remote access

This chapter describes signing in to a Dukovo computer from another one
over SSH, as of Phase 13c of the roadmap: sshd, Dukovo's own SSH server,
built on the [russh](https://github.com/Eugeny/russh) library, and the
OpenSSH client that is already in the image. The full references are
sshd(8), sshd_config(5), authd(8), ssh(1), sftp(1) and scp(1).

## What works now

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
told at their next sign-in. An empty password never signs anyone in
over SSH. Root never signs in over SSH at all: an administrator signs
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

## Not yet

- Options in `authorized_keys`, RSA keys and certificates.
- X11 and agent forwarding, forwarding Unix sockets, and
  `PermitOpen` and `PermitListen`.
- `Match` blocks and the other keywords of OpenSSH's sshd_config.
- Kerberos sign-in, which may come after Kerberos for network file
  systems (13h).
