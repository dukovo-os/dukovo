service.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

service.toml - the files that describe Spark's services

# SYNOPSIS

_/system/settings/spark/services/name.toml_

# DESCRIPTION

Each file describes a service, a program *spark*(8) starts at boot and
keeps running. The file's name without _.toml_ is the service's, and its
resource domain's and its log lines' too: 1 to 23 letters, digits, *-*,
*\_* and *.*, starting with a letter, a digit or *\_*. The files are TOML
(https://toml.io), with comments after *#*, read by the same code in
Spark and in *service*(8), so *service check* says what Spark will say
(ADR 0216).

The system's own files come from
_/system/resources/default-settings/spark/services_, which Spark copies
in when they are missing; to turn one off, set *enabled = false* in its
file rather than removing it, or the next boot brings it back. Each
shipped file lists its settings with comments.

Write a new file with *service new* _name_, which writes every setting
explained, and edit one with *service edit* _name_, which checks the file
as you save it and puts it in place only once it reads. It opens
*nano*(1) unless *VISUAL* or *EDITOR* names another editor, and nano
highlights TOML.

A setting the format does not know is an error, so a misspelt one is
never silently ignored. *service check* and *service edit* show where
an error is and how to fix it, as a compiler does, with the file's
path, the line and the column, the line itself and a fix where there is
one (*service*(8)), whether the mistake is in the file or in its
override:

```
/system/settings/spark/services/web.toml:6:1: `discription` is not a setting here; service.toml(5) lists them
  |
6 | discription = "Web"
  | ^^^^^^^^^^^
  = fix: did you mean `description`?
```

# SETTINGS

*service_version* = _1_ | _2_
	The version of this format the file was written for; every file has
	it. A later Dukovo reads every earlier version. A file for a version
	newer than this Dukovo knows is refused, with a message to update
	Dukovo. Version 2, which Dukovo 1.0 and later read, adds *requires*,
	*wants*, *after*, *give_up_after*, *[ready]* and *[health]*; a
	version 1 file with one of them is refused with that fix. A file
	that uses none of them may stay version 1, which every Dukovo reads.

*description* = _"text"_
	What it is, in a few words, for *service status*.

*program* = _"path"_
	The program's absolute path. Every file has it.

*arguments* = [_"word"_, ...]
	Its arguments, after the program's name; none unless set.

*environment*
	A table of more of its environment, a variable a line, such as
	*TZ = "UTC"*, over the environment Spark gives every service. A
	name holds no *=*. Spark passes them in the order of their names.

*order* = _number_
	When it starts among those ready to start at once: lower first, 50
	unless set. Shutting down stops the highest first, and a service
	before those it requires, wants or starts after whatever their
	*order*, so the services that others need want a low one. The
	system's own take 5 to 50. A service that must wait for another
	says so with *requires*, *wants* or *after*, which *order* alone
	does not: one with a lower *order* is started first but not waited
	for.

*enabled* = *true* | *false*
	Whether Spark starts it at boot; *true* unless set. *service* _name_
	*start* starts one that is not.

*restart* = *"always"* | *"on-failure"* | *"never"*
	When it is started again after it ends: whatever its end (the
	default), only after an exit code other than 0 or a signal, or not
	at all. A start that failed (see STARTING AND READINESS) counts as a
	failure, whatever its status. Spark waits 0.1 s before the first
	start again, doubling each time up to 30 s while it keeps ending
	within 10 s.

*requires* = [_"service"_, ...]
	The services it requires, by name: each is started first, and this
	one starts only once each is ready. One that is stopped, that Spark
	gave up on, that has no file, or that Rescue or a start with only
	the essentials leaves off keeps it from starting; and when one
	stops for good, Spark stops this one too (see STARTING AND
	READINESS). None unless set; version 2.

*wants* = [_"service"_, ...]
	The services it wants: each is started first, and this one waits
	until each is ready or has stopped, then starts whether or not they
	run. None unless set; version 2.

*after* = [_"service"_, ...]
	The services it starts after: when one of them is starting too,
	this one waits until it is ready or has stopped. They are not
	started for it. None unless set; version 2.

	The three lists name at most 32 services each, none twice across
	them and never the service itself; *requires* and *wants* include
	*after*.

*give_up_after* = _number_
	The failed starts in a row after which Spark stops starting it
	again and marks it failed, 1 to 1000; 0, the default, for never
	(see GIVING UP). Version 2.

*ready*
	A table of when it is ready, for the services that wait for it, and
	how long it has to get there; version 2. Its keys:

	*ready.when* = *"started"* | *"listening"* | *"socket"* | *"healthy"* | *"ended"*
		What makes it ready: its process running (the default); a
		socket listening on *ready.port*, or on any port of
		*network.listen*; a socket at *ready.path* made since it
		started; its health check passing for the first time; or its
		process ending with status 0, for a setup step that others
		need done first.

	*ready.port* = _port_
		For *"listening"*: the port, one of *network.listen*'s.

	*ready.path* = _"path"_
		For *"socket"*, which needs it: the socket's absolute path,
		such as _/system/live/services/web/web.sock_.

	*ready.timeout* = _seconds_
		How long it has from its start to get ready, 1 to 3600: 90
		unless set. Past it, the start has failed, and Spark stops it.

*health*
	A table of its health check: a command Spark runs every so often
	while it runs, which passes by exiting with 0 in time (see HEALTH
	CHECKS); version 2. Its keys:

	*health.command* = [_"program"_, _"argument"_, ...]
		The program's absolute path and its arguments. Every *[health]*
		has it.

	*health.every* = _seconds_
		Seconds from the end of one check to the start of the next, 1
		to 86400: 30 unless set.

	*health.timeout* = _seconds_
		How long a check may take before Spark stops it and it fails,
		1 to *health.every*: 10 unless set.

	*health.failures* = _number_
		Failed checks in a row after which Spark restarts the service,
		1 to 100: 3 unless set.

	A dotted name is a key of a table, so *ready.when = "listening"* at
	the top of the file is *when = "listening"* under *[ready]*.

*account* = _"name"_
	The account it runs as, from _/system/settings/passwd_; root's
	unless set, without root's right to pass the owners and modes of
	files. A bundle's service has an account of its own, which
	*bundled*(8) makes as it installs the bundle and names here (ADR
	0385). Spark starts the program as the account and builds its view
	as it: each place in the view is opened as the account, so the
	owners and modes of the files narrow what it reaches as they would a
	person's, and what it makes is the account's. Its _/tmp_, the places
	it may change that Spark makes, its secrets and its copies of its
	certificates are given to the account. Not *"root"*, and not with
	*runs-with-your-access*.

*secrets* = [_"name"_, ...]
	The names of its secrets, such as a database's password or an API
	token, each written as a service's name is; none unless set. Set
	each with *service secret set* _service_ _name_ (*service*(8)).
	Spark hands them over as read-only files, never in its environment
	(see SECRETS).

*certificates* = [_"name"_, ...]
	The names of the certificates it uses, such as *"this-machine"*,
	from those *certd*(8) keeps (*cert list*); none unless set. Spark
	copies each, its chain and its private key, into a folder of the
	service's own, read-only, and has the service read them again with
	SIGHUP when one changes (see CERTIFICATES). A name has 1 to 64
	lowercase letters, digits, *-* and *.*, starts with a letter or a
	digit, and has no *..*. A service that names any starts after certd,
	as if its *after* named certd, when both start. certd's own file
	names none.

*limits*
	A table of what its resource domain may use:

	*cpu_weight* = _number_
		The domain's CPU weight, 1 to 10000, 100 unless set: a busy
		domain of weight 200 gets twice the CPU time of one of 100
		when they compete (*radix-scheduling*(7)).

	*memory* = _"size"_ | _bytes_ | *"none"*
		The domain's memory limit: a number of bytes, or a size with
		*K*, *M* or *G* after it (KiB and so on), such as *"64M"*; no
		limit unless set.

	*oom* = *"service"* | *"app"*
		The domain's out-of-memory class: *"service"*, the default, is
		stopped only after every app domain when memory runs out
		(*radix-resource-domain*(2)).

*permissions*
	A table of what it may do beyond running, by the names of the
	security design's permission list, in groups. A service has only
	what its file gives it:

	*network.outbound* = *true* | *false*
		Whether it may connect out, to any address and port, and send
		datagrams, to the networks the machine is on and beyond them
		(the network capability's *send* and *local* rights,
		*radix-capabilities*(7)); *false* unless set.

	*network.listen* = [_port_ | _"first-last"_, ...]
		The ports it may bind to and listen on, as numbers and ranges,
		such as *[22, "8000-8009"]*; none unless set. Ranges may not
		overlap, and with *network.outbound* and *admin.network* there
		are at most eight. Other machines reach the service there while
		it runs, through the firewall (*firewall*(8), ADR 0334): its
		capabilities for these ports have the inbound right.

	*files.read* = [_"path"_, ...]
		Places of the tree it may read, directories or files, each at
		its own path; none unless set. Each must be there when the
		service starts, or Spark does not start it and says which.

	*files.read-write* = [_"path"_, ...]
		Places it may read and change. Spark makes a directory for one
		that is not there yet, as systemd's StateDirectory= does.

	*files.mount* = [_"path"_, ...]
		Places it may read, change and mount filesystems beneath, as
		*volumed*(8) does _/volumes_; at most 4. Nothing is mounted
		through a view, so Spark also hands each over as a descriptor,
		from 3 on, and *MOUNT_PLACES* names them in that order, with *:*
		between them. Each wants to be the root of a mount, as
		_/volumes_ is, or the kernel cannot name by path what is
		mounted beneath it.

	*files.shared* = [_"name"_, ...]
		Folders of _/shared_ it may read and change, by name, such as
		*["media"]*.

	*runs-with-your-access* = *true* | *false*
		Whether it holds the whole tree as Spark does, past the owners
		and modes of files, in place of a view; *false* unless set, and
		not with *files.\**. Only a service that starts other people's
		shells needs it, as *authd*(8) does.

	*names* = *true* | *false*
		Whether it holds the names capability (*radix-names*(2)),
		which gives a process it starts, or a capability, another
		user's name; *false* unless set. Spark hands it over as the
		descriptor after those of *files.mount*, and
		*NAMES_DESCRIPTOR* says which. Only *authd*(8) needs it, to
		sign people in; a service that held it could act for anyone.

	*admin.network* = *true* | *false*
		Whether it may set the network interfaces up, their addresses
		and routers, as *ip*(8) does, with a network capability of only
		the control right (*radix-network*(2), ADR 0190); *false* unless
		set. *consoled*(8)'s file sets it, so that root's shells may.

	*admin.firewall* = *true* | *false*
		Whether it holds the firewall capability, which gives the packet
		filter its rules (*radix-firewall*(2), ADR 0334); *false* unless
		set. The *firewall* service's file sets it, to load the rules at
		boot, and *authd*(8)'s, so that an administrator's *firewall*(8)
		inside *elevate*(8) may change them.

	*admin.hardware* = *true* | *false*
		Whether it may set the machine's fans, their curves, its LEDs
		and its ARGB header, with the platform capability
		(*radix-platform*(2), ADR 0208); *false* unless set.
		*consoled*(8)'s file sets it, so that root's shells may use
		*ectool*(8), and *authd*(8)'s, so that an administrator's
		commands inside *elevate*(8) may.

	*admin.time* = *true* | *false*
		Whether it may set the time of day, slew it and write the
		machine's real-time clock, with the clock capability
		(*radix-clock*(2), ADR 0194); *false* unless set. *timed*(8)'s
		file sets it, and *authd*(8)'s, so that *date*(1)'s *--set*
		works inside *elevate*(8).

	*devices.pci* = [_"vendor:device"_, ...]
		The PCI cards it drives, for a driver program, each by its
		vendor's and its device's ID in hexadecimal, as *lspci*(8)
		writes them: *"8086:100e"*, or *"8086:\*"* for any of the
		vendor's; none unless set, and none twice (ADR 0198). Spark
		gives it a capability to each card the kernel left for a
		program that one names, at most 4, in the slots from
		SLOT_DEVICE on (*radix-device*(2)), and keeps none itself, so
		a card is free again for the service's next start once it
		ends. A service that names cards of which the kernel left none,
		as on a machine where the kernel drives them itself, is not
		started. *e1000d*(8)'s file names *"8086:\*"*.

	Paths are absolute, without *.*, *..* or *//*; none is _/_ itself
	or within another, none is within a place every service reads
	(name that whole place instead), and none is or holds
	_/system/state/spark_, where Spark keeps its files and the secrets,
	or _/system/live/certificates_, where it keeps the services'
	copies of their certificates' keys.

	A dotted name is TOML's for a table in a table: *network.listen
	= [80]* under *[permissions]* is the same as *listen = [80]* under
	*[permissions.network]*.

*backup*
	What it runs around the snapshots of *backup*(8) (ADR 0463), in a
	table *[backup]* of its own, as the service runs: with its account,
	its view and its limits. Spark runs every running service's
	*before*, one at a time, by *order*, as a round of backups is about
	to make its snapshots, and every *after*, highest *order* first,
	once they are made, so a service waits for the snapshots, which take
	a moment, and not for the copy. A hook that fails or runs out of time
	is told of in the round's output and the log, and the backup goes on.
	Its output goes to the log after the service's name.

	*backup.before* = [_"program"_, _"argument"_, ...]
		The program's absolute path and its arguments, to bring what
		the service keeps to a state it can start again from; none
		unless set.

	*backup.after* = [_"program"_, _"argument"_, ...]
		The same, once the snapshots are made; none unless set.

	*backup.time_limit* = _seconds_
		How long each may run before Spark stops it: 300 unless set,
		at most 3600.

	```
	[backup]
	before = ["/system/commands/psql", "-c", "CHECKPOINT"]
	```

# STARTING AND READINESS

At boot, Spark wants every service that is turned on (*enabled*), and
*service* _name_ *start* wants one; a wanted service wants those it
requires and wants, whether turned on or not, and so on along them.
Rescue and a start with only the essentials leave theirs off still. Each
wanted service waits until those it names are settled, and starts once
it may:

[[ *The other service is*
:- *requires*
:- *wants* or *after*
|  ready, or a setup step that is done
:  starts
:  starts
|  starting, or waiting to start
:  waits
:  waits
|  stopped, failed, off, or without a file
:  does not start
:  starts

Among the services that may start at once, the lower *order*, then the
name, goes first. A service is starting from its start until it is
ready, as *ready.when* says, and *service status* says what it is
waiting for:

```
   Active: activating (waiting for postgres)
```

A service that does not start says why in the log and in *service
status*, as does one stopped because a service it requires stopped for
good:

```
spark: not starting web: it requires postgres, which Spark gave up on
spark: stopping web: it requires postgres, which was stopped
```

Stopping a service by hand stops those that require it; starting it
again does not start them, but starting one of them starts it.
Restarting a service, or Spark starting it again after it ended, leaves
those that require it running: they reconnect, as a client of any
service must.

A start has failed when the program cannot be started, when it ends
before it is ready, when it is not ready in *ready.timeout*, when its
health check restarts it, or when it fails within 10 s of getting ready.
Spark stops a service that is not ready in time as *service stop* would,
and logs what it was waiting for:

```
spark: web did not get ready in 90 s: nothing listened on port 8080; stopping it
```

*"listening"* reads the sockets of _/processes/net/tcp_ and
_/processes/net/tcp6_, which do not name their process, so it counts a
socket on the port whoever listens; a port of the service's own file is
one only the service may listen on, unless another file names it too.
*"socket"* counts the socket only once it is made or changed after the
start, so one left by the last run does not count.

Services that wait for each other, through any of the three lists,
never start; Spark logs the circle once, and each says so:

```
spark: a and b wait for each other, through requires, wants or after: none of them starts until one of their files is changed
```

# HEALTH CHECKS

Spark runs a running service's *health.command* *health.every* seconds
after it gets ready, then *health.every* seconds after the end of each
check, as the service runs: with its account, its view, its
environment, its secrets and its limits, in its resource domain. A
check that exits with 0 within *health.timeout* passes; one that exits
otherwise, or that is still running then and is stopped, fails. Its
output is not logged; the last line a failed check wrote is kept for
*service status*:

```
   Health: failing, 2 checks in a row (exited with code 0x1: no database)
```

Spark logs the first failure in a row, and a pass after failures. At
*health.failures* failures in a row it restarts the service as *service
restart* would, and the end counts as a failed start whatever its
status; with *restart = "never"* it stays stopped. With *ready.when =
"healthy"* the check runs once a second from the start until it first
passes, and the service is ready then.

# GIVING UP

With *give_up_after* set, Spark counts a service's failed starts in a
row, and once it has counted that many, does not start it again: the
service is failed, those that require it are stopped, and the log and
*service status* say why:

```
spark: gave up on web after 5 failed starts in a row (the last: it exited with code 0x1); `service web start` tries again
```

A service that has been ready for 10 s, or a setup step that is done,
starts the count again. *service* _name_ *start* starts a failed service
afresh.

# WHAT A SERVICE SEES

Each service's root is a view of the tree (ADR 0215, 0217), which Spark
builds each time it starts the service: the places its file names, each
at its own path, and what every service has without asking:

- _/system/commands_, _/system/libraries_, _/system/resources_ and
  _/system/settings_, read-only;
- _/processes_ and _/kernel_, read-only;
- _/devices/null_, and _/devices/zero_, _/devices/random_ and
  _/devices/urandom_ read-only;
- _/system/live/notify_, read-only, as notifications are always allowed
  (*notifyd*(8));
- _/system/live/alerts_, read-only, so that the system's services raise
  alerts (*alertd*(8)), which alertd takes from root's processes alone;
- _/system/live/resolved_, read-only, whose _resolv.conf_
  _/etc/resolv.conf_ leads to, so that the C library finds the DNS
  resolver (*resolved*(8)), and _/system/live/netd_, which an
  _/etc/resolv.conf_ from before resolved leads to (*netd*(8));
- its program;
- the folders of its secrets and of its copies of its certificates,
  read-only, if it has any;
- a _/tmp_ of its own, which is _/system/live/spark/tmp/name_ in the
  tree and is emptied at each boot;
- the symbolic links of _/_ and _/system_, so _/bin/sh_ and
  _/usr/bin/env_ work.

Nothing else of the tree is there: not the homes, not _/shared_, not
another service's files, not Spark's. It acts for root, or for its
*account*, but without the right to pass the owners and modes of files,
so a file whose mode closes it to root is closed to it too. A place its file names that is one of
the list takes that one's place, so notifyd's
*files.read-write = ["/system/live/notify"]* lets it change its folder.

A place it names that is not there, a secret that is not set, or a
certificate that certd does not keep, keeps it from starting, and Spark says why in the log and in the answer to
*service* _name_ *start*.

# SECRETS

A service's secrets are in _/system/state/spark/secrets/service/name_,
mode 0600 in a folder of mode 0700, which only root reads. *service
secret set* _service_ _name_ reads the value from the terminal without
showing it, or from its input:

```
service secret set web db-password
printf '%s' "$TOKEN" | service secret set web api-token
```

Spark attaches the service's folder of secrets read-only at that same
path in its view, given to its *account* if it has one (mode 0500, and
0400 for each secret), and names it in *SECRETS_DIRECTORY*, so the service
reads _$SECRETS_DIRECTORY/db-password_. A value is never put in a
variable, which _/processes_, crash reports and logs could show. A
service whose secret is not set is not started:

```
spark: could not start web: its secret db-password is not set: set it with `service secret set web db-password`
```

# CERTIFICATES

*certd*(8) keeps the machine's certificates by name, and makes the
machine's own, *this-machine*, which names the machine, its name with
*.local* and *localhost*; *cert add* adds one from elsewhere (*cert*(8)).
A service names those it uses in *certificates*, and at each start Spark
copies each into _/system/live/certificates/service/name_: the
folder holds *fullchain.pem*, the certificate and then the chain that
leads to its authority, and *key.pem*, its private key, both PEM. The
folder of the service's copies is given to its *account* if it has one
(mode 0500, and 0400 for each file), attached read-only at that same
path in its view, and named in *CERTIFICATES_DIRECTORY*, so nginx reads:

```
ssl_certificate     /system/live/certificates/nginx/this-machine/fullchain.pem;
ssl_certificate_key /system/live/certificates/nginx/this-machine/key.pem;
```

Each copy is the service's own: another service's key is never in its
view, and _/system/live_ is emptied at each boot. When certd makes a
certificate anew, or *cert add --replace* replaces one, Spark copies it
again for each running service that names it, and sends it SIGHUP,
which every server Dukovo ships takes as "read your certificates again".
A service gets at most one such SIGHUP in 10 seconds, and none in its
first 10 seconds or before it is ready: changes in that time are
copied at once, and one SIGHUP for them all follows. The two files
change together, so a service never reads a new certificate with the
old key. A service that cannot reload restarts with *service restart*
_name_ instead.

A certificate certd does not keep keeps the service from starting:

```
spark: could not start web: its certificate shop.example.com does not exist: certd makes this-machine; add others with `cert add`
```

# WHEN SPARK READS A FILE

Spark reads every file at boot, and a service's file again each time it
starts the service, by *service* _name_ *start* or *restart* or after
the service ended: a changed file takes effect at the service's next
start. Its CPU weight and memory limit change then too; its
out-of-memory class stays its domain's until the next boot. A file added
since boot is read when its service is first started.

A file that does not read never takes a service down. Spark keeps a copy
of each file that read in _/system/state/spark/good_, and when a file
does not read, it logs which line is wrong and why, once, and goes on
with that copy, at boot too:

```
spark: /system/settings/spark/services/web.toml: line 6: restart must be always, on-failure or never, not `sometimes`; it runs its last good copy
```

A service whose file never read is not started.

# OVERRIDE FILES

_/system/settings/spark/services/settings.d/name.toml_ changes a
service without touching its file, which the system or *bundled*(8)
wrote and an update may write again (ADR 0386). Its keys take the place
of the file's: a table's key by key, any other value whole, a list too.
Spark and *service check* read it over the file each time they read the
file, and an error names both. A greeting for a bundle's service and
more memory:

```
[environment]
GREETING = "hello"

[limits]
memory = "512M"
```

Spark keeps a good copy of it beside the file's, and a file and override
that do not read together run the last good copy of both.

# FILES OF BEFORE PHASE 13

Until Phase 13, a service file was _name_*.service*, lines of
_key_ *=* _value_ (ADR 0101). Spark converts each it finds that has no
_name_*.toml* beside it, at boot before the new defaults come in, or
when its service is started: it writes _name_*.toml*, with every comment
above the setting it was above, and keeps the old file as
_name_*.service.old*. *weight* becomes *cpu_weight*, and with *memory*
and *oom* goes in *[limits]*; *environment* becomes a table; and
*network.outbound*, *network.listen*, *admin.network* and
*admin.hardware* go in *[permissions]*, with *runs-with-your-access = true* and a comment, since
a service of before Phase 13 held the whole tree: narrow it with
*files.\** instead. A file with an error is not converted, and Spark
logs why.

# EXAMPLE

A web server on ports 80 and 443, which requires its database and starts
after the network, is ready once it listens on 443 and is checked every
minute, serves the folder _/shared/web_, keeps its logs in
_/system/state/web_ and reads its TLS key from its secrets; Spark gives
up on it after 5 failed starts in a row:

```
service_version = 2
description = "Web server"
program = "/applications/commands/nginx"
arguments = ["-g", "daemon off;"]
order = 60
restart = "on-failure"
secrets = ["tls-key"]
requires = ["postgres"]
after = ["netd"]
give_up_after = 5

[ready]
when = "listening"
port = 443

[health]
command = ["/system/commands/curl", "-fsS", "-o", "/dev/null", "https://localhost/"]
every = 60

[environment]
TZ = "UTC"

[limits]
memory = "256M"

[permissions]
network.listen = [80, 443]
files.read-write = ["/system/state/web"]
files.shared = ["web"]
```

# FILES

_/system/settings/spark/services/name.toml_
	The services.

_/system/resources/default-settings/spark/services_
	The system's own, copied in when missing.

_/system/settings/spark/services/settings.d/name.toml_
	A service's override file.

_/system/state/spark/good/name.toml_
	The last copy of each that read, with its override in
	_settings.d_ beside it.

_/system/state/spark/secrets/name_
	A service's secrets.

_/system/live/certificates/name_
	A service's copies of its certificates.

_/system/live/spark/tmp/name_
	A service's _/tmp_.

# SEE ALSO

*spark*(8), *service*(8), *cert*(8), *certd*(8), *logd*(8), *e1000d*(8),
*health.toml*(5)
