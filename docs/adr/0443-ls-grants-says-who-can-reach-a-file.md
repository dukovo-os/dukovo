<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0443. ls --grants says who can reach a file

- Status: Accepted
- Date: 2026-11-09

## Context

This is the last of the new `ls` features (ADRs 0440 to 0442 have the
others). It answers one question of a file: who can reach it? On Linux the answer is spread over `ls -l`,
`getfacl` and every folder above the file, and nothing says what a
person's apps may do. On Dukovo the second half matters as much as the
first: an app runs in a sandbox that holds only the folders it was
given (ADR 0225), from what the person allowed it (ADR 0224).

What a person allowed is kept in `/system/state/grants/UID`, which is
root's and 0600, and only bundled(8) reads it. It already tells each
person their own grants for an app, read-only, when `bundle run` asks
it to find the app (the `run id ID` request), and lists the bundles
installed for them and for everyone (`list`). Neither request changes
anything or writes to the log.

## Decision

A new command of systools, **reach(1)** (`systools/src/reach.rs`), says
who can reach each file it is given, and **`ls --grants`** runs it, on
the current folder when no file is named, in a fourth patch of the port
(`ports/uutils-coreutils/patches/0012`, code in `grants.rs`). ls passes
`--` and the names, and reach's output and exit status are ls's; the
other options of ls do not count, and nothing changes without the flag.

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

- **Accounts**, by the owners, modes and access lists (ADR 0256) of the
  file and of every folder on the way to it, read with the caller's own
  rights and checked as the kernel checks them (radix-vfs's
  `Who::check`, which reach mirrors in `may` and `permits`): each
  account in the password file, in the groups the group file gives it,
  must be let search each folder above the file; then the file's own
  bits or access list say whether it may read it, and whether it may
  change it (for a folder, change and search). A filesystem or
  subvolume that refuses writes (`EROFS`) lets no one change it. The
  owner comes first, even when it may do nothing, since it may change
  that; "everyone" is what an account with no tie to the file may do,
  and only accounts that may do more are named, each with what ties it
  to the file. Administrators pass every check through `elevate`, so
  they are named on a line of their own; root alone, when there are
  none. A file in a person's own `/tmp` (ADR 0255) is in no one else's
  sessions, so only that person's account reaches it, from that
  folder down, whatever its modes say; at its real path in authd's
  folder, as root sees it, the same.
- **Your apps**: each bundle bundled lists for the caller, the caller's
  own before everyone's of the same ID, found as `bundle run` finds it,
  with the caller's grants for it. One that is not sealed does not run,
  and is left out. For each program, reach gives it every permission
  the runner would give it now (`run::kept` and `grants::step`, the
  runner's own code), lays out the folders its sandbox would attach as
  `bundle run` does, and, when one holds the file, checks the caller's
  own permissions from that folder down, since the sandbox acts for
  them and an attached folder is reached without the folders above it.
  A folder attached read-only caps what it gives at read. The app shows
  the most any of its programs reaches, why, and the day the permission
  that gives it was allowed. A command that runs with the caller's
  access reaches what they do, once they allowed that. A file every
  sandbox holds, as `/system/commands`, is shown once as "every app".

reach errs toward saying more can reach a file, never less: a program
counts with each permission it could be given, and the folders above an
attached folder are not asked to let the app through, as the app never
passes them. What it cannot know, it says it does not count: a
permission allowed once, for one run, which a program running now may
have; other people's apps; files reached through a capability a service
holds, as file sharing's; and, for a file with hard links, its other
names, in folders that may be open to more (it says when a file has
more than one).

Other people's apps are left out on purpose. An app runs as the person
who runs it and reaches no more than their account, which the accounts
part already shows; what each person allowed their apps is theirs, and
bundled tells no one else.

## Consequences

- No new request of bundled, and nothing new it says: reach is a
  program of the caller's, reading what they could read anyway, and
  asking bundled only what `bundle run` asks. A sandboxed app cannot
  reach bundled's socket, so it cannot use reach to learn its own
  grants.
- The apps half follows the runner's code for which permissions a
  program gets, and copies its list of what the sandbox attaches; a
  change to the sandbox's view needs the same change in `attached` in
  reach.rs, which says so.
- Each of the caller's apps costs a request of bundled and a read of its
  sealed manifest, for each file named.
- `--gr` no longer abbreviates `--group-directories-first`, as `--gro`
  does; GNU's own option names are unchanged.
- The `ls-grants` boot test has alice install a bundle that asks for
  her Documents folder, allow it, share a file with bob by its access
  list, and check each line of `ls --grants` and `reach`; the lab's
  `commands/ls-grants` test does the same as the lab account, with
  the reach of a file in /system and in the lab account's home.

## Alternatives considered

- **A bundled request that answers "which of my apps reach this
  path?".** It would put path checks into a root service, which could
  then be asked about any path; the grants it needs are already the
  caller's to see, so the program that asks can work it out.
- **Showing everyone's apps.** It would need bundled to tell one person
  what another allowed, and says nothing more about the file than the
  accounts part does.
- **Putting the code in ls.** ls is a port; reach shares the runner's
  code for permissions, which is Dukovo's, and is a command of its own
  for scripts and for the files ls is not asked about.
- **Counting only the permissions a program asked for when it last
  ran.** The runner keeps no record of a run; a permission allowed is
  one it may use the next time it starts.
