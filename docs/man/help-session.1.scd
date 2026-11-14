help-session(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

help-session - let a helper who signs in over SSH work in the owner's screen

# SYNOPSIS

*help-session start* [*--for* _duration_] [*--key* _key_ | *--key-file* _file_] [*--open-ssh*]

*help-session end*

*help-session log* [_id_]

*help-session allow-elevate*

# DESCRIPTION

*help-session* lets a person get help from someone who has no account on
the computer, as Windows' Quick Assist does, with nothing leaving the
network and no service in between (ADR 0540). The owner starts a help
session in their terminal and gives the helper an address and a code;
the helper signs in with *ssh*, and sees and types in the same screen
as the owner, in a shell that runs as the owner. The owner watches
everything the helper does, can type too, and ends the session with one
key.

A help session belongs to a person: it is refused for root, for a
service, and from inside a help screen. A person has one at a time.

# COMMANDS

*start* starts a help session and shows its screen in the terminal until
it ends. It needs a terminal for its standard input and output, since it
takes the terminal over while the session runs. It asks *authd*(8) for
the session, and prints what the helper needs: an address, and the code,
which is made for the session, 16 letters and digits with no *I*, *O*,
*0* or *1*, which look alike, shown in groups of four:

```
$ help-session start
Help session for alice until 15:04 (1 hour). Give your helper:
  ssh help@192.168.1.20
  the code KQ7R-M2XW-9HTP-4BZE
  this computer's key, which their ssh asks them to check:
    SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f
They see and type in this screen, as you. Ctrl-] ends it at any time.
Everything they type is kept: help-session log shows it.
Waiting for your helper...
```

Each address of this computer that another machine can reach has its own
*ssh* line: IPv4 and IPv6 ones, but no link-local address. The key is
the fingerprint of *sshd*(8)'s host key; when sshd has made none yet,
*start* says so in its place. With *--key*, the code's line says that
only that key signs in.

From then on the terminal shows the help screen: the shell, started as
the owner on a terminal of its own, which has the size of the owner's
terminal and follows its changes. What happens to the session shows on
lines of their own, marked *[help-session]*: that the helper signed in,
and from where; that wrong tries spent the code; that elevate was allowed;
and, on the helper's screen as well, that 5 minutes and that 1 minute are
left. Ctrl-] ends the session at once. When it ends, for any reason,
*start* puts the terminal back, says why, and names the transcript:

```
Help session k7q2mx ended: the owner ended it. What your helper typed is kept: help-session log k7q2mx.
```

When a helper had signed in, a second line says that what they changed in
the owner's files and settings stays, as the owner's own changes would.

*end* ends the owner's help session from another of their terminals, as
Ctrl-] does in the session's own, and says *Ended your help session.*

*log* [_id_] lists the owner's kept sessions: each one's ID, owner, start
and end in local time, with *running* for one that has not ended. With an
_id_, it prints that session's transcript, a line for each entry with its
time of day in local time, its kind and its text: who started it and until
when, who signed in and from where, each line the helper typed, when
elevate was allowed, and why it ended. Run through *elevate*(8), it lists
and prints every owner's.

*allow-elevate* lets *elevate*(8) run in the help screen for 15 minutes,
without a password. It is for an administrator, in a terminal that is not
the help screen, and asks for the owner's password there, with no echo,
as *elevate* does, up to three times. It says *elevate runs in your help
screen for 15 minutes, without a password.* The helper cannot ask for it
from the help screen.

# OPTIONS

*--for* _duration_
	How long the help session lasts: *90m*, *2h*, *1h30m*, or a number
	alone for minutes, as *45*. At least 5 minutes and at most 8 hours;
	1 hour without it. Another length is refused before authd is asked.

*--key* _key_
	Lets only the helper's own key sign in, in place of a code. _key_ is
	a public key line, as *ssh-keygen* writes a _.pub_ file, quoted as
	one argument: *'ssh-ed25519 AAAA... comment'*. The kinds sshd takes
	are *ssh-ed25519*, *ecdsa-sha2-nistp256* and *ecdsa-sha2-nistp384*;
	another kind is refused, naming it. No code is given then.

*--key-file* _file_
	As *--key*, with the key line read from _file_, which holds one key
	line and may have blank lines and lines that start with *#*.

*--open-ssh*
	Starts *sshd* for this session when it is off, for an
	administrator, who is asked for their password at the terminal, with
	no echo, and up to three times. sshd then takes sign-ins as *help*
	and no other, and stops at the end of the session. Without it, a
	session cannot start while sshd is off, and says how to.

*--help*
	Print the usage.

*--version*
	Print the version.

# THE HELPER'S SIDE

The helper needs *ssh* and the address; no account and no software of
Dukovo's.

```
$ ssh help@192.168.1.20
The authenticity of host '192.168.1.20 (192.168.1.20)' can't be established.
ED25519 key fingerprint is SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f.
Are you sure you want to continue connecting (yes/no/[fingerprint])? yes
help@192.168.1.20's password:
```

The first time, *ssh* shows the computer's key and asks whether to trust
it. The helper compares it with the one the owner read from the
*help-session start* screen, and answers *yes* only if they are the same:
that is what tells them the machine is the owner's, and not another that
answered for it. The password prompt then takes the code, typed with or
without its dashes and in any case. The first right code spends it, so a
second helper, or the same one again, needs a new session. Five wrong codes
spend it too, and the owner is told; they can start another session. Each
wrong code counts against the address it came from, as a wrong password
does (*blocking.toml*(5)).

With *--key*, *ssh* signs in with that key and no password is asked; a
helper whose key is another's is refused.

The helper gets the owner's screen: one terminal and a shell on it, with
no commands of their own, no SFTP and no port forwarding. The size of
their window is ignored, as the screen keeps the owner's. Ctrl-] from the
helper is a key like any other: only the owner's ends the session.

SSH must reach the computer. The helper must be on the same network, or
connected by a VPN, or the router must forward a port to the computer;
reaching a computer behind a home router from anywhere is not what
*help-session* does.

# SAFETY

A help session gives a stranger the owner's rights for as long as it
lasts, so each part is built to leave the owner in control.

- It never starts without the owner, who is at the terminal it runs in,
  and ends with one key. It also ends when its time is up, when the
  owner's session ends or *help-session start* is stopped, when the
  helper's connection or the shell closes, and when *authd* or *sshd*
  restarts.
- Nothing in the help screen asks for a password, since the helper could
  fake the question and read what the owner types. *unlock*, *passwd*,
  the keyring's unlocking and *help-session start* are refused there,
  saying so, and the keyring stays shut.
- *elevate* in the help screen runs only while the owner allows it, with
  *help-session allow-elevate* from another terminal, for 15 minutes, and
  then without a password. Without it, *elevate* says so, and names the
  command that allows it.
- Everything the helper types is kept, a line at a time with its time,
  in a file only root reads: the helper cannot read or erase it. The
  helper's connection ends before the file would pass 1 MiB, so nothing
  they type goes unkept. The 20 newest sessions of each owner are kept.
  *authd* adds a line to the audit log for each step: the start, the
  sign-in, the wrong codes, the allowing of elevate, and the end.
- The end stops every process started in the help screen, as a service's
  stop does, including any the helper left in the background. What the
  helper changed in the owner's files, crontab or settings stays, as
  the owner's own changes would, and the transcript shows what was typed.

# FILES

_/system/state/help/ID.log_
	A session's transcript, mode 0600, root's. A line is *seconds kind
	text*: the time in seconds since 1970, then the kind, and the text.
	The kinds are *start* (who, until when, and a code or the key's
	fingerprint), *helper* (the address and the code or key they signed in
	with), *typed* (a line the helper typed, with control characters as
	*^X*, other bytes that are not printable ASCII as *\\xNN*, and a
	backslash doubled), *elevate* (until when it was allowed), and *end*
	(why).

# EXIT STATUS

0
	The command did what it was asked. For *start*, the session ended
	for any reason but a start that was refused.

1
	It could not: the standard input or output is not a terminal, it was
	run as root, authd refused it, with the reason, or went away before
	the session ended, three wrong passwords were typed, or the command
	line is wrong.

# SEE ALSO

*sshd*(8), *elevate*(8), *authd*(8), *blocking.toml*(5)
