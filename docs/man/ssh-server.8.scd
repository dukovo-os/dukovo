ssh-server(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

ssh-server - show SSH's settings, change who may sign in and how, and turn sshd on and off

# SYNOPSIS

*ssh-server* [*show*] [*--json*]

*ssh-server set* [*passwords* *yes*|*no*] [*keys* *yes*|*no*] [*allow-users* _name_...|*everyone*] [*allow-groups* _group_...|*everyone*] [*--try* _seconds_]

*ssh-server on*

*ssh-server off* [*--try* _seconds_]

*ssh-server keep*

*ssh-server undo*

# DESCRIPTION

ssh-server is the settings of *sshd*(8) in a person's words: whether
SSH is on, whether a password or a key signs a person in, and
which accounts and groups may. It reads and edits *sshd_config*(5) with
the parser sshd reads it with, so that it sees the file as sshd does,
and asks *spark*(8) for the rest. The web console's Security page reads
*show* as JSON and makes its changes with *set*, *on* and *off*.

*show* runs as anyone. *set*, *on*, *off*, *keep* and *undo* need root,
as the files and Spark's control socket do: run them with *elevate*(8).

*ssh-server --help* prints a summary. *--json* goes with *show* alone, and
*--try* with *set* and *off*; either may come before the words or after
them.

Root never signs in over SSH (*sshd_config*(5)), so none of these
settings has a word for it.

# COMMANDS

With no command, *ssh-server* does *show*.

*show* [*--json*]
	Whether sshd is on, whether it runs and on which port, what
	_sshd_config_ says, and the fingerprint of the key a client knows
	this machine by. Without *--json* it prints a list, and each mistake
	the file has, a line each. What the machine does not say is left as
	a dash, or null in JSON, and does not stop the command.

*set* [*passwords* *yes*|*no*] [*keys* *yes*|*no*] [*allow-users* _name_...|*everyone*] [*allow-groups* _group_...|*everyone*]
	Changes what is given, and nothing else, in _sshd_config_, then asks
	Spark to have sshd read the file again, which keeps the connections
	that are open as they are. At least one setting must be given, and
	each at most once. A list takes the names up to the next setting, so
	that an account called *passwords*, *keys*, *allow-users* or
	*allow-groups* cannot be named here: put it in the file by hand.

	- *passwords* sets PasswordAuthentication, and *keys*
	  PubkeyAuthentication.
	- *allow-users* sets AllowUsers to the accounts named, and
	  *allow-groups* AllowGroups to the groups named. Only they may sign
	  in. *everyone* in place of the names removes the keyword, and so
	  lets everyone in whom DenyUsers and DenyGroups do not keep out. It
	  stands alone.

	It edits the file as sshd reads it. A keyword given twice keeps its
	first value, so a setting is made on its first line; the names of
	AllowUsers and AllowGroups add up over all their lines, so a list is
	made on its first line and its other lines are removed. A keyword
	the file lacks is added at its end, and one that already says what is
	asked is left as it is. Every other line, comment and blank line
	stays exactly as it was. A line that is changed keeps its indent and
	the case of its keyword, and loses a comment after its values, since
	the comment spoke of the old ones. The file is written whole or not
	at all, with the mode it has, and nothing is written when it says so
	already.

	It refuses, and changes nothing:

	- A file that does not read as sshd reads it: the first mistake is
	  told with its line (*NET-183*). Mend it first, as *sshd_config*(5)
	  and *sshd*(8) say.
	- An account or a group the machine does not have, told by its name,
	  whatever else was asked.
	- A result in which neither passwords nor keys are on, since nobody
	  could sign in over SSH.

	When sshd is not running, the file is changed and sshd reads it when
	it starts, which the command says.

	With *--try* _seconds_ (10 to 3600) the change is Spark's trial *ssh*
	(*service*(8), ADR 0472): Spark keeps the file as it was, and puts it
	back and has sshd read it again after _seconds_ unless *keep* comes
	in time, so that a change that keeps the person out of SSH undoes
	itself. Neither the change nor its undo ends a connection that is
	open: test the change from a new one. It needs sshd running, as there
	is no connection for a trial to keep otherwise; when it is not, make
	the change without *--try*.

*on*
	Turns sshd on: its service file says it starts at boot, and Spark
	starts it now, as *service sshd enable* does.

*off* [*--try* _seconds_]
	Turns sshd off: its service file says it does not start at boot,
	and Spark stops it now, as *service sshd disable* does. Stopping it
	ends the connections it serves.

	With *--try* _seconds_ the service file is Spark's trial *ssh*, as for
	*set*: Spark puts the file back and starts sshd again after _seconds_
	unless *keep* comes in time. It needs sshd running.

*keep*, *undo*
	What the trial *ssh* comes to: *keep* ends it with the change in
	force, and *undo* has Spark put the file back and sshd take it up
	now. With no trial they say so (*NET-179*).

While the trial *ssh* runs, *set*, *on* and *off* refuse (*NET-182*),
since its undo would put back the file as it was before both changes.

# JSON

With *--json*, *show* prints one line with an object of these fields, in
this order:

*format*
	1. A reader that finds another number should stop.

*on*
	Whether sshd starts at boot, as its service file says: what the next
	boot does. Null when neither the file nor Spark says.

*running*
	Whether Spark has sshd running and ready. Null when Spark does not
	answer, or does not know the service.

*port*
	The port sshd's service file lets it listen on, from Spark's answer;
	null when Spark does not say, or the file names a range or several
	ports. sshd takes no *Port* of its own.

*passwords*, *keys*
	PasswordAuthentication and PubkeyAuthentication, as *true* or
	*false*, with the defaults of *sshd_config*(5) where the file does
	not say.

*allow_users*, *allow_groups*, *deny_users*, *deny_groups*
	The names of AllowUsers, AllowGroups, DenyUsers and DenyGroups
	added up over the file's lines, in its order: arrays, empty where
	the file has none.

*forwarding*
	AllowTcpForwarding: *yes*, *local*, *remote* or *no*.

*gateway_ports*
	GatewayPorts: *no*, *yes* or *clientspecified*.

*mistakes*
	An array with an object for each line of the file that sshd does not
	take, in the order of the lines, with *line* (from 1), *text* (what
	the line says) and *why*. Empty when the file reads. When it does not,
	*passwords*, *keys*, *allow_users*, *allow_groups*, *deny_users*,
	*deny_groups*, *forwarding* and *gateway_ports* are null: sshd goes on
	with the settings it read last, which the file no longer shows.

*host_key*
	The fingerprint of sshd's key, as *ssh-keygen -l* shows it
	(*SHA256:*...), which a client checks when it connects. Null until
	sshd has made its key, at its first start.

*trial*
	Null, or an object with *left_ms*, the time until Spark undoes the
	trial of SSH's settings unless it is kept (*spark*(8)).

```
{"format":1,"on":true,"running":true,"port":22,"passwords":false,
"keys":true,"allow_users":[],"allow_groups":[],"deny_users":[],
"deny_groups":[],"forwarding":"yes","gateway_ports":"no","mistakes":[],
"host_key":"SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA","trial":null}
```

# EXAMPLES

Let passwords sign in, but only alice's and bob's:

```
$ elevate ssh-server set passwords yes allow-users alice bob
ssh-server: /system/settings/ssh/sshd_config is changed, and sshd reads it for the
connections that come next; the connections that are open stay as they are
```

Let every account in again:

```
$ elevate ssh-server set allow-users everyone
```

Turn passwords off for two minutes, test a sign-in with a key from a new
connection, and keep the change:

```
$ elevate ssh-server set passwords no --try 120
/system/settings/ssh/sshd_config: trying the change for 120 s
ssh-server: sshd reads /system/settings/ssh/sshd_config for the connections that come
next, and the ones that are open stay as they are. Spark undoes the change at 21:42:10
unless it is kept, with `ssh-server keep`.
$ elevate ssh-server keep
Kept: SSH stays as it is now.
```

# FILES

_/system/settings/ssh/sshd_config_
	The settings (*sshd_config*(5)). A file that is not there says
	nothing, so its defaults hold: keys sign in, passwords do not.

_/system/settings/spark/services/sshd.toml_
	The service file, whose *enabled* says whether sshd starts at boot
	(*service.toml*(5)).

_/system/state/sshd/ssh_host_ed25519_key.pub_
	sshd's public key, whose fingerprint *show* gives.

_/system/settings/passwd_, _/system/settings/group_
	The accounts and groups whose names *set* accepts.

# EXIT STATUS

0 when it showed the settings, made the change or found it made, turned
sshd on or off, began a trial, and kept or undid one. 1 for a file with a
mistake (*NET-183*), a name the machine does not have, a result that lets
nobody in, a change while the trial *ssh* runs (*NET-182*), *keep* or
*undo* with no trial (*NET-179*), a trial while sshd is not running, a
tried change that sshd did not take (Spark undoes it at once, and the
command says what Spark did), a command that is not run as root, and a
refusal from Spark, with what it said; 1 for a mistake in the command
line, which also prints its usage.

# SEE ALSO

*sshd*(8), *sshd_config*(5), *service*(8), *service.toml*(5), *spark*(8),
*authd*(8), *elevate*(8), *systools*(1)
