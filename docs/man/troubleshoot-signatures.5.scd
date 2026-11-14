troubleshoot-signatures(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

troubleshoot-signatures - the signatures *troubleshoot* matches

# DESCRIPTION

*troubleshoot*(8) names the likely cause of a start that went wrong by
matching what it knows of the start against signatures (ADR 0474). The
release ships a file of them, _/system/resources/troubleshoot/signatures.toml_,
signed with the release. The update check and an update file may bring a
newer one, kept in _/system/state/troubleshoot/signatures.toml_.

A file is data and nothing in it runs. It is TOML, and it is read by a
reader that holds every part of it to the limits below, as it is fetched
from the network: a file that is not whole and right in every part it
reads is refused, whole.

```
format = 1
version = 3

# A disk with no room for what the services write.
[[signature]]
id = "BOOT-104"
cause = "The disk that holds the system is full."
fix = "Make room: delete files and snapshots you no longer need."
match = [
  { full = "/system/state", percent = 98 },
  { log = "*No space left on device*" },
]
```

# THE FILE

*format*
	The number *1*. A file of another format is refused.

*version*
	A whole number, 0 or more. A file is kept only if its version is
	greater than that of the file the release ships and of the one
	kept: an older file, or the same one, is never taken, and what is
	taken replaces the one kept.

*signature*
	A list of tables, written *\[\[signature\]\]*, each with:

	*id*
		An error ID, such as *BOOT-104*: two to five capital letters, a
		hyphen and three digits. Its page of the support center is what
		_More_ names.

	*cause*
		What is probably wrong, in a sentence.

	*fix*
		What to do, in a sentence or two.

	*match*
		A list of conditions, from 1 to 8. The signature matches when
		every condition holds.

	*cause* and *fix* are one line of text of 1 to 500 bytes, with no
	control character and none that reverses or hides the text that
	follows it. Nothing else may be in a signature, nor in the file.

# CONDITIONS

A condition is a table with one of these keys, and the keys that go with
it.

*log* = "_glob_"
	A line of the start's log matches _glob_, whole. With *service* =
	"_name_", the line is one of the service of that name, or of the kernel
	when it is *kernel*; with *level* = "*error*" or "*warning*", it is of
	that level or one more urgent. A line is its text only, without who
	wrote it or when.

*start* = "*failed*" or "*unfinished*"
	The start ended so. With *error* = "_ID_", with that error ID, which
	the health check gave it.

*changed* = "_kind_"
	An item of this kind differs from the last good start's, or is new or
	gone: *system*, *kernel*, *firmware*, *microcode*, *options*, *card*,
	*root* or *app* (*changes*(1)).

*failed* = "_name_", "*\**" or "*essential*"
	The service of that name failed, or any service did, or one that the
	health check needs did (*health.toml*(5)). Services are known only
	for the newest start.

*full* = "_path_", *percent* = _N_
	The filesystem at _path_, which is absolute, of up to 200 bytes, is
	at least _N_ percent full, as *df*(1) says, from 1 to 100. This too is
	known only for the newest start.

# GLOBS

A _glob_ is of up to 200 bytes of printable ASCII, and not empty. *\**
matches any run of characters, including none, and *?* any one
character; every other character matches itself, capitals told apart
from small letters. There is no way to match a *\** or *?* themselves, and
nothing else is special: a glob is not a regular expression. A glob matches
a text in time that grows with the text alone, as a single pass over it,
whatever the glob.

Matching a file as a whole does a bounded amount of work, counted in
steps: one for each line of the log looked at, and one for each byte of
a line that a glob reads. When that is spent, the conditions on the log
that have not been looked through do not hold, and *troubleshoot* says
that matching was cut short. A signature is never matched that was not
shown to match.

# LIMITS

The file is at most 256 KiB, checked before it is read; it has at most
512 signatures, and each at most 8 conditions, of which at most 64 in
the whole file are on the log (*log*); a glob is at most 200 bytes. A
file over a limit is refused whole, and so is one with a key, a type or a
value out of its place, or two kinds in one condition. The reader also
refuses what is nested deeper than its parser allows. A signature that
this release skips, below, has its conditions on the log counted too, so
that every release refuses the same files.

# WHAT IT DOES NOT READ YET

A condition of a kind this release does not know (a key other than the
five above), with a key it does not know alongside a known one, or with a
value it does not know for *level*, *start* or *changed*, makes its
signature one this release skips, so that a file from a newer release
still helps an older system. *troubleshoot --about* counts those it
skipped.

# REFUSED FILES

A file that is refused is not used and not kept. The update check logs
why, once: a note of its SHA-256 is kept in
_/system/state/troubleshoot/refused_, the last 64, so that the same file
offered each day is logged on the first.

# FILES

_/system/resources/troubleshoot/signatures.toml_
	The release's signatures.

_/system/state/troubleshoot/signatures.toml_
	The newer file that was kept.

_/system/state/troubleshoot/refused_
	The SHA-256 of each file that was refused, a line each.

# SEE ALSO

*troubleshoot*(8), *sysupdate*(8), *update*(8), *changes*(1)
