linux(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

linux - run a Linux program in a sandbox of its own

# SYNOPSIS

*linux run* [*--allow* _path_]... [*--allow-write* _path_]... [*--home*] [*--allow-jit*] [*--*] _program_ [_argument_...]

*linux help*

# DESCRIPTION

*linux run* runs _program_, an x86-64 program built for Linux, in a sandbox
of its own: a process whose root is a view (ADR 0215) that holds what a
Linux distribution's root holds, _/usr_, _/proc_, _/sys_, _/dev_, _/etc_
and _/tmp_, and of your files only what the program needs. It waits for
the program and exits with its status. The program makes Linux's system
calls and the kernel serves them (*radix-linux*(7)); the ones the kernel
does not serve go to *linuxd*(8), which *linux* starts beside it.

A static program, or one built for Dukovo, needs none of this and runs by
its name, as any program does. *nukesh*(1) runs a dynamically linked
program that has no Dukovo note through *linux run* itself, so that typing
the program's name is enough.

_program_ is a path, made absolute from the directory *linux* runs in.
It must be an ELF file for x86-64, an executable or a position-independent
one. Anything else is refused in a line that says why, and a file that is
not an executable of yours fails as any program does.

# THE LINUX BASE

A dynamically linked program needs the libraries it was linked with and
its dynamic loader. They are in the Linux base, the bundle
*org.dukovo.linux-base*, which holds glibc, libstdc++ and the other
libraries most programs take from a distribution, in a _usr_ folder laid
out as Linux's. Without it, *linux* says so and runs nothing:

```
$ linux run ./hello
./hello needs the Linux base: bundle install org.dukovo.linux-base
```

The base is found and checked as *bundle run* finds and checks a bundle
(*bundle*(1), RUNNING): still sealed, its signed list the one *bundled*(8)
installed. Its _usr_ is the program's, read-only. A static program runs
without it.

# THE VIEW

The program's root holds, each at its path:

- _/usr_, the base's, read-only, and the links _/lib_, _/lib64_, _/bin_ and
  _/sbin_ to its _usr/lib_, _usr/lib64_, _usr/bin_ and _usr/sbin_, for the
  ones the base has. The time zone database of Dukovo is at
  _/usr/share/zoneinfo_ if the base has none.
- a shell and the core commands, which are Dukovo's own: _/usr/bin/sh_ and
  _/usr/bin/bash_ lead to *nukesh*(1), which runs as POSIX's shell by the
  name *sh* and as bash by its own; _/usr/bin/env_, _ls_, _cp_, _cat_ and
  the other utilities of uutils' coreutils lead to its one program, and
  _find_ and _xargs_ to uutils' findutils. They are read-only at their
  paths in _/system/commands_, with _/system/libraries_, which they load,
  and _/bin_ leads to _usr/bin_. So a script's _#!/bin/sh_ or
  _#!/usr/bin/env node_ line, and a program that runs *sh -c*, find them. A
  Linux program the shell starts runs in the same view. A base with a
  _usr/bin_ of its own keeps its own commands, and the view adds none.
- _/proc_ and _/sys_, which linuxd serves, in Linux's formats. The
  program's own links in _/proc_, _/proc/self/fd_ and the others, are the
  kernel's (*radix-linux*(7)).
- _/dev_: _null_, _zero_, _full_, _random_ and _urandom_ as far as this
  system has them, _shm_, a memory filesystem of the run's own, as
  _/devices/shm_ is, whose files the program's processes map and share
  and which goes when the run ends, and the links _fd_, _stdin_,
  _stdout_ and _stderr_. When one of the descriptors 0 to
  2 is a terminal it has _tty_, and in a pseudo-terminal, as every pane
  and SSH session has, _ptmx_ and _pts_, so that the program may make a
  terminal of its own. They are in _/devices_ too, which the view holds
  with those names alone.
- _/etc_: _passwd_ and _group_, which hold root, you and *nobody* (your
  home there is the program's data folder, and your shell _/bin/sh_),
  _hosts_ and _hostname_, _resolv.conf_ (the resolver's, as
  _/system/settings/resolv.conf_ names it), _nsswitch.conf_, _os-release_
  (Dukovo's), _machine-id_, _localtime_, a link into the time zone
  database to the zone Dukovo is set to, and _ssl/certs/ca-certificates.crt_,
  the root certificates the system trusts (*cert*(8)). Each is a file,
  attached by itself, which the program may read and not change; the
  other names of _/etc_ are not there.
- _/tmp_, a folder of its own, made for the run in _/tmp/.linux-PID_ of
  yours and removed when the program ends. The files of _/etc_ are in
  a folder beside it, which the program's _/tmp_ does not show.
- the directory *linux* was started in, which the program may change, at
  its real path, where it starts. When that folder is one of the places
  above, or holds your home and *--home* does not give it, it is left out
  with a line that says so, and the program starts in its data folder.
- its data folder, _~/app-data/linux/NAME_, where _NAME_ is the program's
  file name, which it may change, made if it is not there, mode 0700. It
  is the program's *HOME*, with *XDG_CONFIG_HOME*, *XDG_DATA_HOME*,
  *XDG_CACHE_HOME* and *XDG_STATE_HOME* beneath it.
- the folder the program's file is in, read-only, at its real path, unless
  the folder it was started in holds it.
- what you allow it (below).
- the folders of the network that a bundle that connects out and reaches
  the local network is given (*bundle*(1), PERMISSIONS), and the network
  itself: it may connect out and reach the machines on your networks. It
  may not listen on a port.

Nothing else of your files is in it: not your home, not other programs'
data, not the drives you plug in. It acts for you, with the rights your
files give you, never with the right to pass them (*radix-directory*(2)),
and nothing can be added to its view once it runs.

# OPTIONS

*--allow* _path_
	Give the program _path_, a file or folder, read-only, at its real
	path. It may be given more than once. A path in one of the program's
	own places, _/usr_, _/etc_, _/tmp_, _/dev_, _/proc_, _/sys_ or
	_/system_, is refused. A place the program may change inside a folder given to read
	is refused too, as the kernel does not attach beneath what is
	attached: give the outer folder with *--allow-write*.

*--allow-write* _path_
	The same, and the program may change it.

*--home*
	Give the program all of your home, which it may change, in place of
	its data folder and the folder it was started in, which it holds as
	part of it. Its *HOME*, and the home in _/etc/passwd_, are still its
	data folder.

*--allow-jit*
	Let the program make its private memory writable and executable at
	once, as JavaScript engines such as Node.js's V8, LuaJIT, the JVM
	and .NET do as they compile what they run (memory.jit,
	*radix-jit*(2), ADR 0624). Without it, *mmap* and *mprotect* of such
	memory fail with *EACCES*, and when a program that asked for it
	ends with a status other than 0, *linux* says:

	```
	linux: the program asked for memory that is writable and executable, as a JIT compiler does: run it with linux run --allow-jit
	```

	A file's pages and shared memory are never writable and executable,
	with the option or not. The option needs the jit capability, which
	every sign-in session holds (*authd*(8)); where there is none, as in
	a service, *linux run --allow-jit* refuses to start the program.

*--*
	Ends the options. What follows _program_ is the program's own, so
	*--* is only needed when _program_ starts with a dash.

*--help*
	Print the usage and exit, as *linux help* does.

# ASKING

When the standard input is a terminal, *linux* looks at the program's
arguments before it starts it. An argument that names a file or folder
that exists, as an absolute path or one from the directory it was started
in, and that the view does not hold, is asked about on the terminal, once
each, a folder for the files in it that were named too:

```
Let hello read /users/alice/docs/report.txt? [y/N]
```

*y* or *yes* gives it to the program, read-only, at its real path; anything
else, an empty line included, does not, and the program starts without it.
A folder that holds a place the program may change cannot be given
read-only, and *linux* says so and leaves it out. A program run with no
terminal is asked nothing.

# ENVIRONMENT

The program's environment is clean, as a bundle's is: *HOME*, *USER*,
*LOGNAME*, *PATH*=_/usr/local/bin:/usr/bin:/bin_, the four *XDG* folders,
*TERM* (*dumb* when yours is not a terminal type), and *LANG*, *LANGUAGE*
and *LC_\** as yours are, with *LANG* *C.UTF-8* when it is not, and
*DUKOVO_LINUX_VIEW*=_1_, which tells Nukesh it is in a view. *TZ* is not
set: the zone is _/etc/localtime_'s. Nothing else of yours passes.

Its first argument, _argv[0]_, is _program_ as given. The kernel shows the
program's path, with no links, as _/proc/self/exe_.

# LINUXD

*linux* starts *linuxd*(8) before the program, as you, with no view, and
gives it the connection of the filesystem for _/proc_ and _/sys_, that
filesystem's root, and the endpoint that the kernel sends the calls it
does not serve to. If linuxd
ends while the program runs, *linux* starts it again, on the same
filesystem and endpoint, at most 5 times in a minute; after that the
program goes on without it, and what it asks of linuxd fails as an unknown
call does, with ENOSYS, and the filesystem's files with an I/O error.
*linux* says so on the standard error each time.

When the program ends, *linux* ends linuxd with SIGTERM, removes
_/tmp/.linux-PID_, and exits.

# JOB CONTROL

The terminal's Ctrl+C is the program's: *linux* ignores SIGINT and SIGQUIT
while it waits. When the program stops, as with Ctrl+Z, *linux* stops too,
so that the shell that ran it has the terminal back, and sends the program
SIGCONT when *fg* has *linux* go on.

# EXIT STATUS

The program's: its exit status, or 128 and the signal's number for one a
signal ended. *linux* itself gives 125 when it could not run the program,
as *env*(1) does: a mistake in the command line, a path it cannot give, or
a view it could not build. It gives 126 when the file cannot be run (not
an ELF file for x86-64, not a file, or a program that needs the Linux base
when it is not installed) and 127 when the file is not there, as a shell
does.

# FILES

_~/app-data/linux/NAME_
	A program's data folder.

_~/app-data/linux/.machine-id_
	The machine ID that every program's _/etc/machine-id_ holds: 32
	hexadecimal digits, made from random numbers the first time and the
	same since.

_/tmp/.linux-PID_
	The run's private folder: _tmp_, _etc_ and _mounts_, the
	mounts _/proc/mounts_ shows. It is removed when the run ends; one that
	is still there belongs to a *linux* that was killed, and may be
	removed.

_/system/commands/linuxd_
	The server.

# EXAMPLES

```
$ linux run ./hello world
hello, world
$ linux run --allow ~/Pictures --allow-write ~/out ./convert in.png out.png
$ linux run --home -- ./tool
```

# SEE ALSO

*radix-linux*(7), *linuxd*(8), *bundle*(1), *bundled*(8), *nukesh*(1),
*radix-directory*(2)
