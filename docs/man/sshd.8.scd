sshd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sshd - let people sign in to this computer over SSH

# SYNOPSIS

*sshd*

*sshd* *-t* | *-T*

*sshd --connection --settings* _settings_

*sshd --forward*

# DESCRIPTION

sshd lets people sign in to this computer from another over SSH, with
a key their _~/.ssh/authorized_keys_ lists, or with their password once
*sshd_config*(5) lets them, and run a shell or a command there as
themselves, copy files with *sftp* and *scp*, and forward ports (ADRs
0347 to 0349). Spark runs it as the *sshd* service once it is turned
on:

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

# PASSWORDS

A password signs a person in only once *sshd_config*(5) says
*PasswordAuthentication yes*, as an administrator chooses:

```
$ sudo nano /system/settings/ssh/sshd_config
$ sshd -t && sudo service sshd reload
```

*authd*(8) checks it, as at the console. A wrong one is answered only
after a wait, which grows with each wrong one from that address, up to
a minute; *lastb*(1) shows it, and the person is told of it at their
next sign-in. An empty password never signs anyone in over SSH. sshd
keeps no password: authd answers a right one with a ticket for that
connection alone, which starts the person's sessions on it and goes
when it ends.

An address that keeps guessing is blocked: authd counts each wrong
password for the client's address, whatever account it names, and 5
within 10 minutes have the firewall drop that address's new connections
to the sign-in ports for 10 minutes, for twice as long each time it
comes back within a day, up to a day (*blocking.toml*(5), ADR 0479). A
connection that tried to sign in, was refused every time, and ended is
told to authd as it ends, and counts as one wrong password. A connection
that was open when its address was blocked has its next password refused
unread, and sshd ends it. An administrator signed in over SSH is never
blocked while the session lasts.

*AllowUsers*, *AllowGroups*, *DenyUsers* and *DenyGroups* in
sshd_config say who may sign in at all, with a key or a password.

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
terminal's type, or _dumb_ for a client with none, as one cron runs. The client's *LANG*, *LANGUAGE* and *LC_\** settings
are taken, as Debian's sshd takes them (*SendEnv* in *ssh_config*(5));
other variables a client sends are refused.

*sftp*(1) and *scp*(1) copy files as the person: the SFTP subsystem
runs OpenSSH's *sftp-server*(8) as a command of the session, which scp
uses too, and *scp -O*, OpenSSH's old protocol, runs *scp* itself as a
command. Other subsystems, X11 forwarding and agent forwarding are
refused.

# FORWARDING

A client may forward ports both ways, as OpenSSH's sshd lets it unless
*AllowTcpForwarding* in *sshd_config*(5) says otherwise (ADR 0349):

```
$ ssh -L 8080:localhost:80 alice@dukovo
$ ssh -D 1080 alice@dukovo
$ ssh -R 9000:localhost:3000 alice@dukovo
```

With *-L*, *-D* and *-W*, each connection the client forwards is made
from this computer, to where the client names. With *-R*, a port here
listens, and each connection to it goes to the client, which makes
the rest of the way. Either way the connections are the person's: the
connection's forwarder, *sshd --forward*, which *authd*(8) starts as
them the first time their client forwards anything, makes them and
listens, with the network a session of theirs has (ADR 0257). So, as
for anyone but root on Linux, a port below 1024 is refused, and names
are looked up as the person's own programs look them up.

A port forwarded from here listens on the loopback addresses,
127.0.0.1 and ::1, for this computer's programs alone, unless
*GatewayPorts* says otherwise. Even then, other machines reach it only
once the firewall lets them, as for anything a person runs:
*firewall allow program /system/commands/sshd* (*firewall*(8)). A
client that asks for port 0 is told the port chosen. A connection
forwards 128 connections at once, and 16 ports; forwarding Unix
sockets is refused.

# HELP SESSIONS

A person who is stuck may let a helper who has no account sign in with
*help-session*(1) (ADR 0540). The helper signs in as *help*, which is no
account and which no account may be named, with the code the owner gave
them, typed at the client's password prompt, or with the key the owner
named:

```
$ ssh help@dukovo
help@dukovo's password:
```

authd(8) checks the code or the key's signature. *help* passes
*AllowUsers* and *AllowGroups*, and while a help session waits for a code
(_/system/live/authd/help-waiting_ is there) sshd offers passwords for
*help*, and for no other name, even when *sshd_config* turns passwords
off. A code that is wrong counts against the client's address as a wrong
password does.

A helper's connection has one session channel, with a terminal and a
shell, which is the help screen: what the owner sees and types in, at
the owner's size, whatever the size of the helper's window. The helper
gets no command, no SFTP, no subsystem and no forwarding, and sshd says
so in its log. sshd passes the helper's bytes to the help screen and its
output back, and writes each line the helper types, with its time, to the
session's transcript, which authd gave it and which only root reads,
before the line goes on. It ends the connection when the transcript
would pass 1 MiB. The helper's screen starts when they sign in and shows
what changes from then on; Ctrl-L redraws it. When the help session
ends, sshd tells the helper, "The help session ended.", and closes the
connection; when the helper's connection ends first, sshd tells authd,
which ends the session.

With _/system/live/authd/help-only_ there, which authd makes when
*help-session start* with *--open-ssh* starts sshd for one session, sshd
signs in *help* and no one else, whatever *sshd_config* says, until it is stopped
at the session's end.

# RESCUE

In Rescue (*dukovo-rescue*(8)) *sshd* runs only when the installed system
had it on, and signs in that system's administrators (ADR 0540). It
reads the installed system's _sshd_config_, _passwd_, _group_ and host
key, but not from the root, which it cannot see: *dukovo-rescue* copies
those five files into _/system/state/sshd/rescue_, in Rescue's memory,
so that the program that faces the network never sees the password
hashes or a home. Rescue's *sshd* makes no host key, so a client sees
the fingerprint it knew. *authd*(8) checks the keys, passwords and
accounts, against the root's, and signs in the administrators alone.

A session in Rescue is *dukovo-rescue*'s menu, on the terminal sshd
made, as root, which authd starts. A command, SFTP, a subsystem and port
forwarding are refused, and a helper's sign-in fails, since Rescue starts
no help session.

# HOW IT WORKS

*sshd*, as Spark runs it, as root, reads *sshd_config*(5), makes the
host key if there is none, listens on port 22 of every address, IPv4
and IPv6, and starts *sshd --connection* for each client, with the
connection as its standard input and the settings it read. SIGHUP,
which *service sshd reload* sends, has it read sshd_config again for
the connections that come after; the ones open go on as they were, and
a file with a mistake leaves the settings as they were.

*sshd --connection* speaks SSH 2 with russh: the key exchange is
mlkem768x25519-sha256 or curve25519-sha256, the ciphers
chacha20-poly1305, AES-GCM and AES-CTR, the MACs SHA-2's. Before a
client signs with a key, sshd asks *authd*(8) whether the key may sign
the person in. To start each session, it hands authd what the client
signed and the signature, and authd checks them itself against the
key, and the key against _~/.ssh/authorized_keys_, or the ticket it
gave for the person's password, before it starts the session as the
person, on the terminal sshd made or on three pipes.
sshd then passes the bytes between the client and the session, and lets
the client send only as much as the session has taken.

The first time a client forwards a port, sshd asks authd for the
connection's forwarder too, with the same proof, and authd starts
*sshd --forward* as the person, on three pipes, outside any shell. sshd
passes it frames: the connections to make and the ports to listen on,
and the bytes of each, which go only as fast as the other side takes
them, a stream at a time. The forwarder ends with the connection, and
its connections and ports with it; should it end before, the
connection's forwarded channels close, and the log says why.

A connection has two minutes to sign in and six keys to try; at most
30 connections sign in at once, 10 of them from one address, and 100
are open. A connection holds four sessions at once, and the computer
32 SSH sessions. A client that sends nothing for a minute is asked
whether it is there; one that does not answer three times is let go.

# OPTIONS

*-t*
	Check _/system/settings/ssh/sshd_config_: say each mistake, a line
	each, and end with 1 if there is any, as OpenSSH's *sshd -t* does.

*-T*
	Show the settings sshd_config makes, each keyword once, with the
	defaults of those it does not give.

# FILES

_~/.ssh/authorized_keys_
	The keys that sign the person in.

_/system/settings/ssh/sshd_config_
	The settings (*sshd_config*(5)).

_/system/libraries/ssh/sftp-server_
	OpenSSH's SFTP server, which an SFTP session runs.

_/system/state/sshd/ssh_host_ed25519_key_
	The computer's own key, Ed25519, made the first time sshd starts,
	in OpenSSH's format: root's and of mode 0600. Removing it has
	sshd make another as it starts again, which clients that knew the
	old one then warn about.

_/system/state/sshd/ssh_host_ed25519_key.pub_
	Its public half, which anyone reads.

_/system/state/sshd/rescue/_
	In Rescue only: the five files of the installed system that sshd
	reads, laid out as the root's subvolumes are (_@settings/passwd_,
	_@settings/group_, _@settings/ssh/sshd_config_ and
	_@state/sshd/ssh_host_ed25519_key_ with its _.pub_), mode 0600, which
	*dukovo-rescue*(8) copies.

_/system/live/authd/help-waiting_, _/system/live/authd/help-only_
	Present while a help session waits for a helper with a code, and while
	sshd runs for help sessions alone (see HELP SESSIONS).

_/system/state/help/_*id*_.log_
	A help session's transcript, which authd opens and sshd writes the
	helper's lines to.

_/system/settings/spark/services/sshd.toml_
	sshd's service file (*service.toml*(5)), whose *enabled* *service
	sshd enable* and *disable* set; the system's own is
	_/system/resources/default-settings/spark/services/sshd.toml_.

# EXIT STATUS

sshd runs until Spark stops it, which ends the connections it serves
and their sessions; 1 if sshd_config has a mistake, or it cannot make
or read the host key, or listen on port 22. *sshd --connection* ends
with its connection, and *sshd --forward* as sshd closes its input.
*sshd -t* and *sshd -T* end with 0, or with 1 for a mistake.

# SEE ALSO

*ssh*(1), *sftp*(1), *scp*(1), *ssh-keygen*(1), *sshd_config*(5), *ssh-server*(8),
*sftp-server*(8), *authd*(8), *help-session*(1), *dukovo-rescue*(8), *login*(1), *service*(8),
*service.toml*(5), *blocking.toml*(5), *last*(1), *firewall*(8)
