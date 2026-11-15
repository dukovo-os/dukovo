logtrim(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

logtrim - trim the system's log

# SYNOPSIS

*logtrim* [*-n*] [*-s* _size_] [*-d* _days_]

*logtrim --show* [*--json*]

*logtrim set* _key_ _value_ [_key_ _value_]...

# DESCRIPTION

logtrim asks *logd*(8) to trim the system's log now, and says which day
files it removed and how much room that made (ADR 0211).

logd trims the log by itself, as _/system/settings/log_ says (*log*(5)):
when it starts, when a day begins, after each 64 MiB it writes, and
when its settings are read again. Without options, logtrim asks for the
same trim now. With *--size* or *--days*, it trims the log, not the
audit log, to limits of its own, once, as journalctl's *--vacuum-size*
and *--vacuum-time* do; the audit log keeps to its own setting.

logd removes whole day files, oldest first, and never the one it is
writing or today's. Each removal goes into the audit log, with the
process that asked for it.

*logtrim --show* tells how much the log takes and what its limits are,
which the web console's Storage page shows, and *logtrim set* changes
the limits in _/system/settings/log_ (*log*(5)). Both are for root:
logd tells the log's size to root alone, and only root writes the file.

# OPTIONS

*-n*, *--dry-run*
	Say what would be removed, and remove nothing.

*-s*, *--size* _size_
	Remove the oldest of the log's day files while they take more
	than _size_: bytes, or with *K*, *M*, *G* or *T* after them (KiB
	and so on), or *KB*, *MB*, *GB*, *TB* (powers of 1000).

*-d*, *--days* _days_
	Remove the log's day files older than _days_ days; with 0, all of
	them but today's.

*--show*
	Say how much the log and the audit log take, in how many files, the
	four limits the settings file gives (a limit of 0 is no limit), and
	the most the two may take on their file system, which is the smaller
	of *max-size* and *max-percent* of the file system's size. Mistakes
	in the settings file, which logd does not read, are listed. It takes
	none of the other options.

*--json*
	With *--show*, print one line of JSON (below).

# SET

*logtrim set* _key_ _value_ [_key_ _value_]... changes each setting named
in _/system/settings/log_ and has logd read the file again, which trims
the log by the new limits at once. The keys, as *log*(5) has them:

*keep-days* _days_, *audit-keep-days* _days_
	A number of days, 0 to 3650; 0 for no limit.

*max-size* _size_
	A size, as *500M* or *4G*, or 0 for no limit.

*max-percent* _percent_
	A share of the file system, 0 to 100; 0 for no limit.

A setting is changed on its line, the last one it has, since a later line
overrides an earlier one; a setting the file does not have is added at
its end. Every other line, the comments and the blank lines, stays as it
is. A change that leaves the text as it was writes nothing, and says so.
If logd is not running the file is changed all the same, and logd reads it
when it starts; *logtrim set* then says logd did not read it again, and
exits with 1.

# JSON

With *--show --json*, one line with an object of these fields, in this
order:

*usage*
	What the log takes: *bytes*, of the log and the audit log together;
	*files*; *audit_bytes*, the audit log's share; and *directory*, where
	the day files are.

*limits*
	*keep_days*, *max_bytes*, *max_percent* and *audit_keep_days*, as the
	settings file sets them, the defaults for what it does not; 0 for no
	limit.

*cap*
	The most bytes the log and the audit log may take, with these limits,
	on the file system the log is on; 0 for no limit.

*filesystem_size*
	The size of that file system in bytes, or *null* if it cannot be read.
	Without it *cap* counts only *max_bytes*.

*wrong*
	The mistakes in the settings file, a sentence each, as logd says them
	in the log.

# EXIT STATUS

0, or 1 if logd is not running or refuses, the command is not run by
root, a value is refused, or the command line is wrong.

# FILES

_/system/settings/log_
	The limits (*log*(5)), which *set* changes.

# EXAMPLES

```
logtrim -n -s 500M
logtrim --days 30
logtrim --show
logtrim set keep-days 60 max-size 20G
```

# SEE ALSO

*logd*(8), *log*(5), *log*(1), *journalctl*(1), *systools*(1)
