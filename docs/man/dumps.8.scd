dumps(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dumps - the kernel's crash dumps: save, list, show and delete them, and
test that they are saved

# SYNOPSIS

*dumps* [*list*]

*dumps show* [_name_]

*dumps path* [_name_]

*dumps delete* _name_... | *--all*

*dumps save*

*dumps test* [*--yes*]

# DESCRIPTION

When the kernel panics it writes a text dump of what it knew into the
RAM that the crash log keeps across the restart (*radix-debug*(7), ADR
0483): the panic's report, the CPUs, every thread and its state, the run
queues, the locks held, memory, and the raw kernel stack of each CPU
around where it stopped. The next boot gives that dump, then a line
*== Log* and the whole console log of the boot that crashed, as
_/devices/crashdump_, which is empty when the previous boot left no
dump. The kernel says at boot, after the crash log's lines, whether it
kept one:

```
Crash log: the previous boot's dump is kept for dumps(8) (38 KiB)
```

*dumps* copies the dump to a file in _/system/state/crash_ and works on
the files there. A dump is plain text: read it with *dumps show*, or
copy it into a report with *elevate dumps show > crash.txt*. It is kept
on the machine and sent nowhere.

Only root reads the dumps, since one holds what the kernel held,
including what was on its stacks: the device and every saved file are
mode 0600, and the folder is mode 0700. Anyone else is told to run it
with *elevate*(8):

```
dumps: only root reads the crash dumps; try elevate dumps
```

# COMMANDS

With no command, *dumps* does *list*. *dumps --help* prints a summary.

*list*
	The saved dumps, newest first, as a table: _NAME_, _TIME_ (the
	time of the panic, in local time), _BOOT_ (the number of the boot
	that crashed), _SIZE_ and _PANIC_, the panic's message, which is
	the first line after *== Panic* without its *Radix panic on CPU*
	_n_ *at* _s_ *s:* start; on a terminal it is cut to the terminal's
	width. A file of the folder with a dump's name whose content is no
	dump shows *not a dump* and the reason in the last column. With none
	saved, *dumps* says *no crash dumps saved*.

*show* [_name_]
	Write the dump _name_, or the newest, to the standard output. When
	that is a terminal, the text goes through the pager *$PAGER* names,
	or *less*.

*path* [_name_]
	Print the path of the file of the dump _name_, or the newest.

*delete* _name_... | *--all*
	Delete the named dumps, or all of them, saying *Deleted* _name_ for
	each. A name that no saved dump has fails before any is deleted.

*save*
	Copy the previous boot's dump from _/devices/crashdump_ to the
	folder, and keep only the newest dumps there (see *SAVING*). It
	says nothing when the previous boot left no dump. The *dumps*
	service runs it at each boot.

*test* [*--yes*]
	Panic the kernel on purpose, to check that dumps are kept and
	saved on this machine (see *TESTING*). *dumps* asks first, at the
	terminal, *This panics the kernel and restarts the computer now.
	Type panic to go on:*, and goes on only if *panic* is typed.
	*--yes* goes on without asking, and is needed where there is no
	terminal.

A _name_ is a file's name as *list* shows it, without the folder.

# SAVING

The service runs *dumps save* right after *logd*(8), so that the line it
logs is in the log. The file is named by the time of the panic, in UTC,
and the boot's number:

```
2026-11-13T10-02-11Z-boot42.txt
```

When the kernel did not know the time of day (the dump's third line
says *Panic at an unknown time of day*), the name has the boot's number
and eight hexadecimal digits of a hash of the dump in place of the time,
so that the same dump gets the same name:

```
unknown-time-boot42-9f3a07c1.txt
```

The boot's number alone is not enough: it starts again at 1 when the RAM
was not kept.

A dump that is saved already is not saved again; *dumps save* says *the
previous boot's dump is already saved as* _name_. A new one is written to
_name_*.new* beside it, mode 0600, synced, renamed, and the folder synced,
so that a file of the folder is whole whatever happens. Then the oldest
dumps past the tenth, or past 64 MiB together, are deleted; the one just
saved is not.

At warning level, the service logs the panic's message and the file:

```
dumps: the previous boot ended with a kernel panic: selftest crash-reboot: deliberate panic; the dump is /system/state/crash/2026-11-13T10-02-11Z-boot42.txt (dumps show)
```

*log -s dumps* shows it (*log*(1)).

# TESTING

*dumps test* has the filesystems write out the changes they hold in
memory, as *sync* does, then writes *panic* and a line end to
_/devices/crashdump_, which only root can open. The kernel logs a warning that names the
process, panics with the message *dumps test: a panic asked for on
purpose*, writes its report on every console and serial port that
carries the log, and restarts the machine, after the key or the delay
that *panic*=_seconds_ (*dukovo-boot*(7)) gives. The boot menu of an
installed system gives *panic=10*. At the next boot the dump is kept,
saved, named in the log, and listed by *dumps list*: the whole chain, as
it will run for a real panic.

A real panic writes nothing out: what the filesystems held in memory
and had not yet written is lost. A machine whose firmware clears the RAM when it
restarts keeps no dump, and neither does one that was powered off before
it started again; *dumps list* then shows nothing new, and the report on
the serial port or Serial-over-LAN (*serial-console*(8)) is what the
panic left.

# FILES

_/devices/crashdump_
	The previous boot's dump, then *== Log* and that boot's log. Root's,
	mode 0600; *fstat* gives its length. Writing exactly *panic* and a
	line end to it panics the kernel, which *dumps test* does; any other
	write fails.

_/system/state/crash_
	The saved dumps, mode 0700, each a file of mode 0600.

_/system/settings/spark/services/dumps.toml_
	The service. It holds no permission beyond the device it reads and
	the folder it writes (*service.toml*(5)).

# EXIT STATUS

0 when it did what was asked; 1 for a mistake in the command line, a
refusal because it was not run as root, a name that no saved dump has,
*no crash dumps saved* from *show* or *path*, a *test* that was not
confirmed or whose filesystems could not write their changes out, and an
error reading or writing a file.

# EXAMPLES

The newest dump, and where its file is:

```
elevate dumps list
elevate dumps show
elevate dumps path
```

Check on a new machine that a panic leaves a dump:

```
elevate dumps test
```

# SEE ALSO

*radix-debug*(7), *dukovo-boot*(7), *service*(8), *logd*(8), *log*(1),
*elevate*(8), *spark*(8), *serial-console*(8), *systools*(1)
