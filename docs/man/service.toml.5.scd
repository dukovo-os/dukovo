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
	*wants*, *after*, *give_up_after*, *[ready]*, *[health]* and *[stop]*; a
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
	gave up on, that has no file, or that Rescue, a start with only
	the essentials or a try of *changes bisect* leaves off keeps it from
	starting; and when one
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

*start* = *"at-boot"* | *"on-connection"*
	When Spark starts it, once it is enabled: at boot, the default, or
	at the first connection or datagram on one of its *sockets*, which
	Spark holds meanwhile, so that the connection waits and is served
	once the service runs (see SOCKETS SPARK HOLDS). *"on-connection"*
	needs at least one socket. Stopping it by request puts it back to
	waiting. ADR 0477.

*sockets* = [{ ... }, ...]
	The sockets Spark makes and holds for it, as *[[sockets]]* tables,
	at most 16 (see SOCKETS SPARK HOLDS). Each has one of *port* and
	*path*; under *[[sockets]]* each key is written without *sockets.*:

	*sockets.port* = _number_
		A TCP socket on that port, which must be one of
		*network.listen*'s: that list covers TCP and UDP alike.
		Without *address* it takes every address, IPv6 and IPv4
		together.

	*sockets.address* = _"0.0.0.0"_ | _"::"_ | _"address"_
		With *port*: every IPv4 address; every IPv6 address and no
		IPv4; or one IPv4 or IPv6 address, such as _"127.0.0.1"_.

	*sockets.udp* = *true* | *false*
		With *port*: a UDP socket instead of a TCP one. *false* unless
		set.

	*sockets.path* = _"name"_
		A Unix stream socket of that name in the service's live
		folder (see WHAT A SERVICE SEES). A plain name of letters,
		digits, underscore, hyphen and dot, not "." or ".." and with no slash.

	*sockets.name* = _"word"_
		The word for it in *LISTEN_FDNAMES*: letters, digits, underscore,
		hyphen and dot, with no colon; each once in the file. The
		port's number or the path unless set.

	*sockets.backlog* = _number_
		The connections the kernel keeps waiting for the service to
		accept, 1 to 65535: 4096 unless set.

	*sockets.non-blocking* = *true* | *false*
		Whether the socket is passed with *O_NONBLOCK* set, as
		systemd's *NonBlocking=* does; *false* unless set. A program
		that takes a socket as it inherits it and waits for it with
		*poll*(2), as nginx does, needs it: on a socket that waits,
		each of its processes that loses the race for a connection
		waits in *accept*(2), and no longer hears a request to stop.

	*sockets.to* = _"anyone"_ | _"system"_
		With *path*: who may connect besides the service. *"anyone"*
		makes the socket root's, mode 0666; *"system"* root's, mode
		0600, for the system's own programs. Spark also sets the
		service's live folder to 0755 so that a socket can be reached
		by name. Without *to* and *group* the socket is the
		service's account's, mode 0600. *bundled*(8) writes it for a
		socket a bundle offers (see *manifest.json*(5)).

	*sockets.group* = _"name"_
		With *path*: the group whose members may connect, as an
		account's name is written. The socket is root's, in that
		group, mode 0660. A group that does not exist is skipped, and
		Spark logs it. Not with *to*.

	```
	start = "on-connection"

	[permissions]
	network.listen = [8090]

	[[sockets]]
	port = 8090
	name = "web"
	```

*groups* = [_"group"_, ...]
	The groups Spark gives its process besides its account's, at most
	16, each once and named as an account is, which Spark gives the
	process, and the root its view acts through. *bundled*(8) writes it
	for a bundle that is given a socket another bundle offers to its
	group (see *manifest.json*(5)). None unless set.

*stop*
	How Spark stops it, in a table *[stop]* of its own (ADR 0477);
	version 2. Its keys:

	*stop.signal* = _"SIGTERM"_ | _"SIGINT"_ | _"SIGQUIT"_ | _"SIGUSR1"_ | _"SIGUSR2"_ | _"SIGWINCH"_
		The signal that asks the service to stop: *"SIGTERM"* unless
		set. *nginx*'s is *"SIGQUIT"*, its graceful stop. Not
		*"SIGHUP"*, which *service reload* sends to read settings
		again, and not *"SIGKILL"* or *"SIGSTOP"*, which a program
		cannot take as a request.

	*stop.timeout* = _seconds_
		How long Spark waits for the service to end after the signal,
		before it ends what is left of it: 5 unless set, 1 to 3600.

	```
	[stop]
	signal = "SIGQUIT"
	timeout = 30
	```

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

	*io_weight* = _number_
		The domain's share of a disk, 1 to 10000, 100 unless set. While
		services wait for a disk, those with the larger weight get the
		larger share of its requests: of weights 400 and 100 reading
		as fast as they can, the first gets four times the bytes
		(ADR 0476). A request of a more urgent *ionice*(1) class or
		level still goes first; weights share the disk between those of
		the same rank. With nothing waiting a service gets all of the
		disk, whatever its weight.

	*io_read* = _"size"_ | _bytes_ | *"none"*, *io_write* = _"size"_ | _bytes_ | *"none"*
		The bytes the domain may read from disks, and write to them, each
		second, written as *memory* is: *"50M"* is 50 MiB a second. The
		rate is for the domain across all disks together, and holds a
		reader or writer back, which its programs notice only as slower
		reads and writes. No limit unless set. Dirty pages the page
		cache writes back later are the kernel's and are held to no
		rate; *service*(8)'s *status* shows the time a service's rates
		held it.

	*io_read_ops* = _number_ | *"none"*, *io_write_ops* = _number_ | *"none"*
		The read and write requests the domain may make to disks each
		second, 1 or more; no limit unless set. A rate of requests holds
		a program that makes many small ones, which a rate of bytes
		does not.

	*threads* = _number_
		The most threads the domain may have, in all its processes
		together, 1 to 1048576, *4096* unless set. A *fork* or thread
		creation past it fails with EAGAIN, as Linux's *pids.max* does,
		so a fork bomb in a service fills its own limit and not the
		machine's; ADR 0476. The service's own first thread counts.

	*open_files* = _number_
		The files each of the service's processes may have open at
		once, 1 to 1048576: Spark sets the soft and the hard limit of
		RLIMIT_NOFILE to it before the program starts, and what the
		program starts keeps it. A program has 1024 unless this says
		more, so a server with many connections, as nginx with
		*worker_connections*, names its own (*open_files = 65536*).
		*service check* refuses a number outside 1 to 1048576, naming
		its line and column.

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
		service starts, or Spark does not start it and says which,
		except a place in _/system/state_ or deeper in _/system/live_
		than a folder of it: that is state or a socket another service
		makes, which may not be there yet, and the view goes without it
		until a later start (see below).

	*files.read-write* = [_"path"_, ...]
		Places it may read and change. Spark makes a directory for one
		that is not there yet, as systemd's StateDirectory= does, unless
		it is deeper in _/system/live_ than a folder of it, as a
		service's socket is: a directory there would keep its service
		from making the socket, so the view goes without it.

	A socket either list names, such as _/system/live/log/query_, is
	reached through its folder: a socket cannot be opened to be
	attached, so Spark attaches the folder with only the names the
	file gives in it, read-only unless one of them that is no socket
	is in *files.read-write*. Connecting needs no write to the folder,
	and a socket its service makes again, as *logd*(8) does when it
	restarts, is found at the same name.

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

	*files.disks* = *"read"* | *"read-write"*
		Every disk and partition, as _/devices_ shows them, as they
		come and go, with the four devices every service has and
		nothing else of _/devices_, no keyboard among it. Not set
		unless the file says so. *volumed*(8) holds
		it to mount the disks that are plugged in, and *diskhealth*(8)
		reads them. Not with *files.efi-system*, and no place of
		_/devices_ in *files.\** with it (ADR 0473).

	*files.efi-system* = *"read"* | *"read-write"*
		The EFI system partitions alone, found by their partition
		type, as _/devices_, in the same way, and the four devices
		every service has. *sysupdate*(8) holds it, to keep the
		update state where the firmware finds it, and no other disk.

	*files.root-subvolumes* = [_"name"_, ...]
		Names at the top of the root's Btrfs, such as
		*["@snapshots"]*: the top is at _/volumes/disk_, where disk is
		the one Spark started from (_/system/live/spark/root_
		says which), read-write, with these names in it and no other.
		A name that is not there is made nowhere, and none of the
		root's other names can be made, removed or renamed to. At
		most 16, each of letters, digits, *@*, *.*, *-* and *\_*.
		*@users*, *@shared* and *@applications* are refused: an update
		or a snapshot never needs the owner's files. The update
		service holds the slots, *@system.a* and *@system.b*, and
		*snap*(1)'s holds *@snapshots* (ADR 0473). On the live
		system and in Rescue there is no such top, and the view none.

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

	*admin.tune* = *true* | *false*
		Whether it holds the kernel settings capability, which lists the
		kernel's settings and sets them (*radix-kernel-settings*(2),
		ADR 0476); *false* unless set. Only the *tune* service's file
		sets it, so that *tune*(8) may give the kernel a performance
		profile at each boot. Anyone reads the settings' values in
		_/kernel/settings_.

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
Rescue, a start with only the essentials and a try of *changes bisect*
(for the services it leaves out) leave theirs off still. Each
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

# SOCKETS SPARK HOLDS

A service whose file has *sockets* does not make them itself: Spark
makes them before it starts the service, at boot for every enabled
service, so that services that talk to each other start in any order. A
socket Spark cannot make, because the port is taken, fails that
service's start, with the reason, and the others go on.

Spark passes them as *sd_listen_fds*(3) does: the descriptors from 3, in
the file's order, with *LISTEN_FDS* their number, *LISTEN_PID* the
process's own number and *LISTEN_FDNAMES* their names, separated by
*:*. The places of *files.mount* follow, from _3 + LISTEN_FDS_.

*service restart*, a restart after a failure and a bundle's update keep
the sockets open. While no process accepts, connections wait in the
backlog and are served once the new process runs: a client waits a
moment and is not refused. *service stop* of a service that does not
start *"on-connection"* closes them, so the port refuses; for one that
does, it puts the service back to waiting, with its sockets open. *service
disable* closes them. *service status* says "waits for a connection on
port 8080" of a service that waits.

A program that listens for itself and cannot take an inherited
descriptor is not helped: name no socket for it. *nginx*'s wrapper
turns the descriptors into its *NGINX* variable (see the handbook's nginx chapter).

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

# TIMERS

A service with a *timer*, a *[timer]* table, is started at times (ADR 0478): *timerd*(8)
asks Spark to start it, as *service start --by timer* does. A timer does
not turn a service on: an enabled service with a timer is off between
its times, and a start that comes while the last still runs is skipped.
*service status* says when it last started and next starts. A timer
needs *at* or *every*.

*timer.at* = [_"time"_, ...]
	Times of day: five fields as *crontab*(5) has them (_"30 3 \* \* \*"_
	is half past three each night), or a word: _"hourly"_, _"daily"_,
	_"weekly"_, _"monthly"_ or _"yearly"_. At most 32.

*timer.every* = _"duration"_
	So long after the last start: _"6h"_, _"90m"_, _"1d12h"_. The units
	are *w*, *d*, *h*, *m* and *s*, largest first, each once; from one
	second to 366 days.

*timer.after_boot* = _"duration"_
	With *every*: the first start comes this long after the machine
	starts, not *every* after the last start.

*timer.spread* = _"duration"_
	Put each time off by up to this much, by an amount that is the same
	for the timer each time and differs between machines, so that machines
	do not start together.

*timer.catch_up* = _true_ | _false_
	Whether a time that came while the machine was off makes the service
	start once, soon after the machine starts. True when left out.

*timer.time_zone* = _"name"_
	The time zone of *at*, a name of the time zone database; the
	system's when left out.

# LOG FILES

A service that writes its own log files names them in *logs*, a *[logs]*
table, and
*logturn*(8) turns them over (ADR 0478): it renames a file that is due,
has the service open its files again, and compresses the files turned
over before. The service writes on into the renamed file until it has,
so no line is lost. A file the service writes through the system's log
(standard output and error) is *logd*(8)'s, and needs no setting here.

*logs.files* = [_"pattern"_, ...]
	The files, relative to the service's data folder, which is the
	_HOME_ of its environment (for a bundle's service, the folder of its
	account; else _/system/state/_name). A \* may stand in the last part
	of the pattern only: _"logs/\*.log"_. At most 16.

*logs.turn_over* = _"daily"_ | _"weekly"_ | _"size"_
	When a file is due: each day, each week, or when it holds this
	many bytes (_"100M"_, with K, M or G). _"daily"_ when left out.

*logs.keep* = _number_
	How many turned-over files are kept, 1 to 100. 14 when left out.

*logs.compress* = _true_ | _false_
	Whether the files turned over are compressed with zstd, from the
	second on. True when left out.

*logs.reopen* = _"signal"_ | _"restart"_ | _"none"_
	How the service opens its files again: Spark sends it the signal,
	such as _"SIGUSR1"_, which nginx takes; restarts it; or does nothing,
	for a service that opens its file for each line. _"none"_ when left
	out. The signal can be neither _SIGTERM_, _SIGINT_ nor _SIGQUIT_.

# WHAT A SERVICE SEES

Each service's root is a view of the tree (ADR 0215, 0217), which Spark
builds each time it starts the service: the places its file names, each
at its own path, and what every service has without asking:

- _/system/commands_, _/system/libraries_, _/system/resources_ and
  _/system/settings_, read-only;
- _/kernel_, read-only, and _/processes_, read-only, which lists the
  service's own processes and those of the services it starts, and no
  others, unless its file names _/processes_ with *files.read*, or has
  *runs-with-your-access*, and then every process (ADR 0473);
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
A file that names a place of _/devices_ with *files.disks* or
*files.efi-system*, or a place within _/processes_, is refused: those
are made as a whole.

*service* _name_ *status* says what the view holds, in its *Holds:* lines
(*service*(8)).

A place it names that is not there, a secret that is not set, or a
certificate that certd does not keep, keeps it from starting, and Spark
says why in the log and in the answer to *service* _name_ *start*. A
folder of _/system/live_ is made instead, as is a place it may change
elsewhere. A place held read-only in _/system/state_, and one deeper in
_/system/live_, is left out of the view instead, and Spark says so in
the log: the update health check's view names troubleshoot's
signatures, which are not there until an update keeps a newer set, and
the log's query socket, which is not there while *logd*(8) restarts.

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
start. Its limits change then too (its CPU weight, its memory, its
disk weight and rates, its threads and its open files); its
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

*spark*(8), *service*(8), *timerd*(8), *logturn*(8), *cert*(8), *certd*(8),
*logd*(8), *e1000d*(8), *health.toml*(5)
