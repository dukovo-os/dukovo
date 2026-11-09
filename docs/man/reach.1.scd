reach(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

reach - who can reach a file: which accounts, and which of your apps

# SYNOPSIS

*reach* _file_ ...

*ls --grants* [_file_ ...]

# DESCRIPTION

reach says, for each _file_, who may read it or change it (ADR 0443).
*ls --grants* runs it, on the current folder when no _file_ is named.
For a folder, reading is listing its names, and changing is adding,
renaming and removing them.

```
Documents/plan.txt (/users/alice/Documents/plan.txt)
  Accounts:
    alice  read and write  owner
    bob    read            its access list
    Administrators reach every file with elevate: alice.
  Your apps:
    Notes  read and write  your Documents folder, allowed 2026-11-09
  An app reaches no more than the person who runs it; other people's apps are
  theirs to see.
```

The first line is the _file_ as named, and its real path when that
differs, with no link or *..* in it.

*Accounts* are the accounts that may read or change the file, by the
owners, modes and access lists (*getfacl*(1)) of the file and of every
folder on the way to it, as the kernel checks them: an account must be
let through each folder above the file before the file's own
permissions count. The owner comes first, even when it may do nothing,
since it may change that; then each other account that may do more than
everyone, with what ties it to the file: *its access list*, or *group*
and the file's group. A file in your own _/tmp_ is in no one else's
sessions, which each have their own (*authd*(8)), so only you reach
it, whatever its modes say. *everyone* is what an account with no tie to the
file may do, when that is anything. Administrators may read and change
every file with *elevate*(8), and are named on a line of their own;
with none, root is.

*Your apps* are the apps installed for you or for everyone that would
reach the file when you run them: by the folders each one's sandbox is
given, from what you allowed it (*bundle*(1)), and then by your own
account's permissions within those folders, since the sandbox acts for
you. Each app shows the most any of its programs reaches, why, and the
day you allowed the permission that gives it. *every app* is a file of
the system's that every app's sandbox holds, such as
_/system/commands_. An app with a command that runs with your access
reaches what you do, once you allowed that. Every program of an app
counts with each permission it could be given now, including one it is
not running with, so reach may say an app reaches more than it does,
and never less. An app that is not sealed does not run, and is left
out.

A note follows when nothing can change the file because its filesystem
or subvolume is read-only, and when the file has more than one name,
since another may be in a folder open to more.

reach reads nothing you could not read yourself. What you allowed your
apps comes from *bundled*(8), which tells each person their own, as it
does for *bundle run*; it is not stored where you can read it. Other
people's apps are not shown: an app reaches no more than the person who
runs it, which *Accounts* says, and what they allowed is theirs to see.

reach does not count a permission you allowed *once*, for one run
alone, which a program running now may have; files reached through a
capability a service holds, as file sharing's (*share*(1)); or programs
you run outside any app, which reach what you do.

# EXIT STATUS

0 if every _file_ was answered; 1 if one could not be reached, or for a
mistake in the command line.

# SEE ALSO

*ls*(1) (*coreutils*(1)), *getfacl*(1), *chmod*(1), *bundle*(1),
*bundled*(8), *manifest.json*(5), *elevate*(8), *systools*(1)
