scrub(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

scrub - read and check every block of a Btrfs filesystem

# SYNOPSIS

*scrub start* [*-B*] [*-r*] [*-q*] [*--progress*] _path_

# DESCRIPTION

scrub reads every block of the Btrfs filesystem that holds _path_, as
Linux's *btrfs scrub start -B* does, with the same summary. Every copy
of every tree block and data sector the last commit holds is read from
the disk, past every cache, and checked against its checksum; a tree
block is checked as a read checks it, and the superblock's copies too.
A bad copy with a good one beside it, as the two copies of DUP
metadata are, is mended: the good copy is written over it (ADR 0150).

The filesystem stays in use while it runs; what is written after the
scrub has passed its place is left to the next one. The counts are a
sector at a time, as Linux counts them: a bad copy of a 16 KiB tree
block is four errors.

The summary names the filesystem, when the scrub started, how long it
took, how much it read and how fast, and what it found: _no errors
found_, or each kind of error with its count, then how many sectors
were corrected, how many no copy could mend (uncorrectable), and how
many were not checked (unverified).

A scrub is kept for the web console. When it ends, scrub writes what it
found to _/system/state/scrub/_*uuid*_.json_ (below), a file for each
filesystem, which *storage*(8) reads. A filesystem is scrubbed by one
scrub at a time: a lock in _/system/live_ holds a second one back
(*DSK-102*, with its page in the support center) until the first ends.
A lock that a crash left is taken over.

# OPTIONS

*-B*
	Wait for the scrub to finish, and print its summary. scrub always
	waits, so this is what it does without it too.

*-r*
	Only read: count what is bad, and write nothing. A read-only
	mount, such as */system*, can be scrubbed only so.

*-q*
	Print nothing but errors.

*--progress*
	While the scrub runs, print a line *progress* _P_*%* when the number
	moves, at most one a second, which the web console shows as the bar
	of a task (*admind*(8)), and *progress 100%* when it has ended. The
	kernel says nothing of a scrub until it is done, so the number is an
	estimate: the bytes its disks were read for since it began, of the
	bytes the filesystem holds times the copies its data profile keeps.
	It stays at 99% or below until the scrub has ended, and stays at 0%
	for a filesystem whose disks cannot be found. Other reads of the
	disks count too.

# THE KEPT RESULT

_/system/state/scrub/_*uuid*_.json_ is one line of JSON, which anyone may
read, with these fields in this order:

*at_ms*
	When the scrub ended, in milliseconds since 1970.

*seconds*
	How long it took.

*bytes*
	The data and tree bytes it read (the summary's _Total to scrub_);
	for a scrub that stopped, the bytes its disks were read for while it
	ran.

*errors*
	The sectors found bad on read, in a superblock, in what they held or
	in their checksums: _read_, _super_, _verify_ and _csum_ added up.

*fixed*
	How many of them were mended from a good copy.

*unfixable*
	How many no copy could mend (_Uncorrectable_).

*result*
	*ok* when it found no error that no copy mends, *errors* when it
	found some (the exit status is 3), and *stopped* when it was cut
	short by Ctrl-C or SIGTERM (the status is 128 and the signal's
	number) or failed after it began. A scrub that stopped has no count
	of what it found.

# DIFFERENCES FROM LINUX

- *status*, *cancel* and *resume* are not there yet, and scrub never
  runs in the background: it returns when the scrub is done.
- A filesystem with more than one device is scrubbed as one, with one
  summary; Linux prints one for each device with *-d*.
- *-c* and *-n* (the scrub's I/O priority) and the speed limits are
  not there yet.

# EXIT STATUS

0 when the scrub found no errors, or mended every one; 1 when it could
not be done, because a scrub of the filesystem runs (*DSK-102*), or for a
mistake in the command line; 3 when it found errors that no copy
mends, as Linux's *btrfs scrub* exits; 128 and the number of the signal
when it was stopped by one.

# FILES

_/system/state/scrub/_*uuid*_.json_
	The result of the filesystem's last scrub (above).

_/system/live/scrub-_*uuid*_.lock_
	The lock of a scrub that runs, with its process number.

# SEE ALSO

*radix-directory*(2), *radix-files*(7), *subvolume*(8), *storage*(8),
*systools*(1)
