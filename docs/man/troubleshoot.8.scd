troubleshoot(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

troubleshoot - the likely cause of a start that went wrong

# SYNOPSIS

*troubleshoot* [*--start* _start_] [*--all*] [*--json*]

*troubleshoot* [*--facts* _folder_] [*--save-facts* _folder_]

*troubleshoot --about*

# DESCRIPTION

troubleshoot names the likely cause of a start that failed, did not
finish or went wrong in a way the computer knows (ADR 0474). It gathers
what is known of one start, and matches it against signatures: each
signature is a cause, the error ID whose support page tells more, what to
do, and the conditions that must all hold for it. A signature whose
conditions all hold is a match. Nothing is sent anywhere, and nothing
runs: the signatures are data (*troubleshoot-signatures*(5)).

With no option it looks at the newest start that was not good, or the
newest start if every one was. While the health check is still checking
this boot's start, that start is not good only because it has no outcome
yet, so it looks at the one before it and says so: "Start 43 is still
being checked; showing start 42." A start of an earlier boot that did not
end is looked at, as is this boot's when the health check is not running.
What it gathers about a start is its record and what changed since the
last good start (*changes*(1)); its log, the kernel's lines among them,
from logd's record of the boot the start was made in, up to an hour after
the health check began, the newest 20000 lines of that, each cut to 400
bytes, so that a computer that has run for days still gives the lines of
the start; and, for the newest start only, since they are read as they
are now, which services Spark says have failed and how full each mounted
filesystem is. Whoever is not an administrator reads only the log lines
of their own programs (*logd*(8)), and so may find less. What it cannot
gather it says at the end.

Matching does a bounded amount of work, however many signatures there
are and whatever they ask of the log. When that is spent before every
signature has looked through the log, it says so at the end: what it has
said is right as far as it went, and a cause that needs a line of the log
may be missing.

It says the best three matches, each with its cause, its fix and the
page of the support center for its error ID. The start's own error ID
comes first, then the signature with most conditions, then by ID:

```
$ troubleshoot
Start 42 on 2026-11-11 21:04, Dukovo 0.15.2, slot B: failed: sshd has restarted 3 times (BOOT-102)

Likely cause: An essential service ended, kept restarting or did not start in time (BOOT-102)
Fix: The start's message names the service: service NAME explain says why it is in its state, and service NAME logs what it wrote. changes shows what is new since the last good start.
More: https://dukovo.org/support/0.15.2/errors/BOOT-102.html
```

The failure notice that the sign-in screens show carries a line from the
best match, "Likely cause: CAUSE (ID). Fix: FIX". The notification that
tells whoever is signed in carries only the first line of the notice, which
says that the start failed its health check and why. Rescue's menu has "Find the likely cause" (*dukovo-rescue*(8)), which
works from the root's records, or from what Rescue knows when the root
does not mount. *service explain* (*service*(8)) puts the fix of a
signature that matches the service at the root of its chain in place of
the generic one.

# OPTIONS

*--start* _start_
	Look at start _start_ instead, by number (*changes --list* lists them).

*--all*
	Say every match, not the best three.

*--json*
	Write one line of JSON: the start, the version of the signatures in
	use, how many were skipped, and the matches, best first, each with
	_id_, _cause_, _fix_, _help_ and whether it is the start's _own_
	error ID, and what could not be gathered or searched as _missing_.
	When the start passed over is this boot's, still being checked, its
	number is _checking_.

*--save-facts* _folder_
	Write the facts it gathered to _facts.json_ in _folder_, then match
	them as usual. The folder is made if it is not there, and must not
	be a symbolic link: nothing is written through one, so that a
	folder someone else left in a shared place cannot send what an
	administrator writes elsewhere. The facts go to a new file that only
	their owner can read, which then replaces _facts.json_. The facts of
	a fault that was planted are what the tests of each shipped
	signature are made from.

*--facts* _folder_
	Match the facts in _folder_/_facts.json_, which *--save-facts*
	wrote, instead of gathering them.

*--about*
	Say which signatures are in use: their version, whether they are the
	file the release ships or a newer one kept in the state, how many are
	read and how many were skipped because they have a condition this
	release cannot read, and any file that was left out and why.

# EXIT STATUS

0 when at least one signature matches, 1 when none does, no start has
been recorded yet, or the only one is this boot's, still being checked.

# FILES

_/system/resources/troubleshoot/signatures.toml_
	The signatures the release ships.

_/system/state/troubleshoot/signatures.toml_
	A newer file the daily update check, or an update file, brought.

_/system/state/starts/_
	The record of each start (*changes*(1)).

# SEE ALSO

*troubleshoot-signatures*(5), *startup*(8), *changes*(1), *log*(1),
*service*(8), *sysupdate*(8), *dukovo-rescue*(8)
