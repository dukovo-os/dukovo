sshd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sshd - let people sign in to this computer over SSH

# SYNOPSIS

*sshd*

*sshd --connection*

# DESCRIPTION

sshd lets people sign in to this computer from another over SSH, with
a key their _~/.ssh/authorized_keys_ lists, and run a shell or a
command there as themselves (ADR 0347). Spark runs it as the *sshd*
service once it is turned on:

```
$ sudo service sshd enable
sshd is running, and starts at each boot.
```

It is off until then, and port 22 with it; *service sshd disable*
turns it off again (*service*(8)). Then, from another computer, with
OpenSSH's client or any other:

```
$ ssh alice@dukovo
```

The first time, the client shows this computer's key's fingerprint and
asks whether to trust it. sshd logs the fingerprint as it starts
(*log -s sshd*, *log*(1)), and
*ssh-keygen -lf /system/state/sshd/ssh_host_ed25519_key.pub* shows it;
compare the two.

# KEYS

A key signs a person in when their account is a person's and not
disabled, and their _~/.ssh/authorized_keys_ names it, a line each, as
OpenSSH writes them:

```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... alice@laptop
```

*ssh-copy-id* on the other computer, or copying its
_~/.ssh/id_ed25519.pub_ into that file, adds one. Keys of the kinds *ssh-ed25519*,
*ecdsa-sha2-nistp256* and *ecdsa-sha2-nistp384* are taken; RSA keys,
P-521 keys, security keys (*sk-*) and certificates are not. A line
with options before its key, such as *from=* or *command=*, is passed
over, and the log says so: sshd honours none of them yet.

The file, _~/.ssh_ and the home must be the person's or root's, and
neither their group nor others may write them, as OpenSSH's sshd checks
with StrictModes; otherwise no key in the file signs anyone in. Links
are not followed, and the file is read up to 1 MiB.

Passwords are refused: only a key signs anyone in.

# SESSIONS

A session runs as the person, with their groups and permissions, as
one at the computer's console does (*authd*(8)): their login shell when
the client asks for one (*ssh alice@dukovo*), or their shell running
the command the client sends (*ssh alice@dukovo uptime*), with *-c*.
A client that asks for a terminal, as *ssh* does for a shell, gets one,
of its size and with its settings, and the session is recorded for
*last*(1), from the client's address; a shell on it shows the last
sign-in and the notices as at the console. A command without a
terminal runs on pipes:
its output and errors come back apart, and the client hears its exit
status, or the signal that ended it.

Besides what every session gets, a session has *SSH_CLIENT* and
*SSH_CONNECTION*, and on a terminal *SSH_TTY* and *TERM*, the client's
terminal's type. The client's *LANG*, *LANGUAGE* and *LC_\** settings
are taken, as Debian's sshd takes them (*SendEnv* in *ssh_config*(5));
other variables a client sends are refused.

SFTP and *scp*, port forwarding, X11 forwarding and agent forwarding
are refused.

# HOW IT WORKS

*sshd*, as Spark runs it, as root, makes the host key if there is none,
listens on port 22 of every address, IPv4 and IPv6, and starts
*sshd --connection* for each client, with the connection as its
standard input.

*sshd --connection* speaks SSH 2 with russh: the key exchange is
mlkem768x25519-sha256 or curve25519-sha256, the ciphers
chacha20-poly1305, AES-GCM and AES-CTR, the MACs SHA-2's. Before a
client signs with a key, sshd asks *authd*(8) whether the key may sign
the person in. To start each session, it hands authd what the client
signed and the signature, and authd checks them itself against the
key, and the key against _~/.ssh/authorized_keys_, before it starts
the session as the person, on the terminal sshd made or on three pipes.
sshd then passes the bytes between the client and the session, and lets
the client send only as much as the session has taken.

A connection has two minutes to sign in and six keys to try; at most
30 connections sign in at once, 10 of them from one address, and 100
are open. A connection holds four sessions at most, and the computer
32 SSH sessions. A client that sends nothing for a minute is asked
whether it is there; one that does not answer three times is let go.

# FILES

_~/.ssh/authorized_keys_
	The keys that sign the person in.

_/system/state/sshd/ssh_host_ed25519_key_
	The computer's own key, Ed25519, made the first time sshd starts,
	in OpenSSH's format: root's and of mode 0600. Removing it has
	sshd make another as it starts again, which clients that knew the
	old one then warn about.

_/system/state/sshd/ssh_host_ed25519_key.pub_
	Its public half, which anyone reads.

_/system/settings/spark/services/sshd.toml_
	sshd's service file (*service.toml*(5)), whose *enabled* *service
	sshd enable* and *disable* set; the system's own is
	_/system/resources/default-settings/spark/services/sshd.toml_.

# EXIT STATUS

sshd runs until Spark stops it, which ends the connections it serves
and their sessions; 1 if it cannot make or read the host key, or
listen on port 22. *sshd --connection* ends with its connection.

# SEE ALSO

*ssh*(1), *ssh-keygen*(1), *authd*(8), *login*(1), *service*(8),
*service.toml*(5), *last*(1)
