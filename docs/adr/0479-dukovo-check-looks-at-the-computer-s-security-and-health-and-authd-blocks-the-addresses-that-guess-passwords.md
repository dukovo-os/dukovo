<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0479. dukovo-check looks at the computer's security and health, and authd blocks the addresses that guess passwords

- Status: Accepted
- Date: 2026-11-14

## Context

Two things a server owner on Linux sets up by hand were missing.

The first is a look at the machine: which ports other computers reach,
whether SSH takes passwords from anyone, whether an account has no
password, whether an update waits, whether the clock was ever set, whether
a service gave up. Each answer lives in a different tool (`firewall`,
`sshd -T`, `service list`, `update`, `hardware check`, the early warnings
of metricsd), and nothing said which of them matter. The web console's
health score (thread C) needs one such answer to read, kept where it can.

The second is a stop to password guessing. authd slowed wrong passwords
(ADR 0218), but only for one account from one address, so a program that
tries many accounts at once, or changes the name each time, was never
slowed more than the first try. Nothing blocked an address.

On Linux, Lynis and OpenSCAP report hundreds of items against a generic
system and leave the reader to judge which matter. fail2ban and CrowdSec
read the log after the fact with patterns that break when a message
changes, and need setting up before they do anything.

## Decision

1. **One command, one report, for security and health.** `elevate
   dukovo-check` runs 15 checks and prints a line as each ends: the area,
   the check, `passed`, `warning`, `FAILED` or `skipped`, and a detail. A
   warning has a second line with its ID and the one command or edit that
   fixes it. The report looks like `hardware check`'s, so that a person who
   has read one reads both. `FAILED` appears only when a check repeats a
   failure another tool found, a hardware check that failed or an early
   warning that is critical. The exit status is 0 when every check that ran
   passed, 1 for a failure, 2 for a warning and no failure, and 3 when it
   could not run, which includes being run without elevate: it reads the
   password hashes and the firewall.
2. **The checks** are one function each over a context of the files and
   answers they read, so that the unit tests plant the files and a later
   check is one more entry in the list. They are: network (what other
   machines reach, a rule that lets every port in, the firewall service),
   ssh (passwords, who may sign in, blocking), accounts (empty passwords,
   administrators), updates (a newer release waiting, the last look at
   the update server), time (the clock), services (any that gave up),
   health (early warnings, the last hardware check) and backups (snapshots
   of `/users` and `/shared`). The two update checks read
   `/system/state/sysupdate/checked`, which each read of the update
   channel writes with the release the system ran and the release offered,
   and compare it with the release of the running slot, so that a look
   made before an update says nothing about the one after it. The SSH
   checks pass with "SSH is off" while the service is off. A check of
   something that happens now and then, the update look and the time
   server, is skipped in the first hour after the computer starts. The
   hardware check is never run by it, since that takes minutes; it reads
   the last result and skips when there is none.
3. **Every warning has an ID and a page.** A new error area, `SEC`, holds
   twelve: SEC-101 and SEC-102 (the firewall), SEC-110 to SEC-112 (SSH),
   SEC-120 and SEC-121 (accounts), SEC-130 and SEC-131 (updates), SEC-140
   (the clock), SEC-150 (services) and SEC-160 (snapshots). A finding that
   repeats another tool's keeps that tool's ID (a failing disk is HW-113), so one fault has one page.
4. **A default install has no warnings,** so a warning is real: SSH is off,
   the firewall is on, the update check runs daily, timed runs, blocking is
   on, the snapshots cover the people's files, and the installer's
   administrator has a password. The boot test `dukovo-check` holds this.
5. **The result is kept, and read.** Each run under elevate, and a daily run
   by the `dukovo-check` service's `[timer]` (05:00 and up to an hour
   after, by an amount that differs between machines), writes
   `/system/state/dukovo-check/last.json`, mode 0640 and group `admins`,
   replaced whole. `dukovo-check --last` prints it for any account in
   `admins`, without checking again, and `--json` prints it as the file
   holds it: a version, the time, the release, each result with its ID and
   fix, and the counts. The console's health score counts warnings and
   failures from it; its weights are the console's, and the file says
   nothing about a score. `dukovo-check` changes nothing but this file.
6. **authd counts failed sign-ins by address, across names and services.**
   One failure is counted for the client's address for each of: a wrong
   password over SSH, a wrong sharing password (smbd passes the PC's
   address), a wrong help-session code (ADR 0540), and an SSH connection
   that tried to sign in, was refused every time, and ended, which sshd
   tells authd once as it ends with `ssh-failed`, unless authd counted a
   password of it already. An address is an IPv4 address or an IPv6
   network of 64 bits, since one computer holds a whole /64, so a guesser
   cannot step through its own network; an IPv4 address written as IPv6 is
   the IPv4 one. The count is in memory, bounded at 4096 addresses with
   the oldest dropped, and the slowing per account and address stays as it
   was.
7. **Five failures in ten minutes block an address for ten minutes, and
   each block that comes within a day of the end of the one before lasts
   twice as long, up to a day.** A right password from an address clears
   its count but not its record of blocks. `/system/settings/blocking.toml`
   (`on`, `tries`, `within`, `first`, `longest` and `never`) sets the
   numbers, and the default file is on with an empty `never`. authd reads
   it again when its time of change or length changes, before the next
   request it answers; a file that does not read is logged and the settings
   stay as they were.
8. **A block closes the sign-in ports of the address, and no more.** The
   ports are those that `sshd`, `smbd` and `admind` listen on, from their
   service files and the overrides over them (port 22 when none names one),
   joined into at most four ranges. A computer behind a phone network's
   shared address does not lose its web site because of a guesser on that
   address. A connection already open is not cut: authd refuses its next
   password unread, with "this address is blocked for guessing passwords"
   and the time left, and sshd ends the connection.
9. **The kernel drops what a block stops.** radix-firewall(2), version 2,
   gains `set_blocked` (the control right), which takes the whole list each
   time it changes, as `set_rules` takes the rules, and `blocked` (the read
   right), which gives the blocks whose time is not up. A block is an
   address family, an address or /64, the seconds it has left and up to
   four ranges of ports, none meaning every port. The kernel keeps up to
   1024 blocks in order of address, asks them with a binary search before
   the rules, and ends a block when its time is up even if authd is late.
   A block lasts 30 days at most. A dropped packet is counted and logged
   with the others, with "the address is blocked" as its reason. While no
   address is blocked, `admit` asks nothing.
10. **Never blocked:** loopback and this computer's own addresses; the
    networks in `never`; and, while the session lasts, the address an
    administrator is signed in from over SSH, so that a mistyped password
    cannot lock out the person fixing the machine. A network in `never`
    that meets a source spares it in the count and in the kernel's list,
    so adding one lifts a block at once. The console and the serial port
    are not on the network and are never blocked.
11. **Blocks survive a restart.** authd keeps them in
    `/system/state/authd/blocked` (mode 0600, a line `ADDRESS UNTIL
    STARTED TIMES REASON` each), gives the kernel those whose time is not
    up as it starts, and keeps a block for a day after it ends so that the
    next one lasts longer.
12. **Seeing and lifting blocks.** authd answers `blocked`, `unblock
    ADDRESS` and `never-block NETWORK [remove]`, for an administrator
    inside elevate, and `firewall` fronts them: `elevate firewall
    blocked` lists each block with its address, when it ends, how many in
    a row it has had and why; `unblock` lifts one and forgets the blocks
    before it, so that the next is a first; `never-block` adds a network
    to `never`, keeping the rest of the file, comments included.
13. **Told and logged.** Each block, unblock and change of the settings is
    a line of the audit log, with the reason, such as `authd: blocked
    203.0.113.9 for 10 minutes: 5 wrong passwords over SSH in 2 minutes,
    for root, admin and test`. Each administrator's next sign-in says how
    many addresses were blocked since their last.
14. **The boot tests `dukovo-check` and `blocking`, and the lab tests
    `security/dukovo-check` and `security/blocking`, cover them.** The
    handbook's "Hardening a server" chapter puts them among the other
    steps, and dukovo-check(8), blocking.toml(5), firewall(8), authd(8)
    and the SEC pages describe them.

## What it is not

- A failed sign-in at the web console does not count yet: the console's
  sign-in (thread C) passes the browser's address to authd when it is
  built. Its address is not spared either.
- No "fix it for me" mode: each fix is one command the line gives, and the
  console's buttons run them.
- A block does not close a port that no sign-in service listens on, and
  does not end a connection already open.
- The checks look at what Dukovo ships and offers, not at software a person
  installed.

## Alternatives

- **A report of hundreds of items** against a generic Linux, as Lynis and
  OpenSCAP give, leaves the reader to judge which matter. Here the checks
  describe the system Dukovo ships, so a default install reads clean, every
  warning is real, and each has a stable ID, a page and its fix.
- **Reading the log for failures,** as fail2ban and CrowdSec do. The
  program that checks the password counts it, so nothing is parsed, and
  the count cannot break when a log message changes.
- **Blocking in authd's own sockets or in each service.** Only the
  firewall drops a connection before the service reads it, and one block
  list serves every sign-in port.
- **Blocking every port of an address.** A shared address would lose the
  web site and every service because of one guesser; the sign-in ports are
  what guessing needs.
- **Counting per account only,** as the slowing did: a guesser who changes
  the name starts each name's run afresh.

## Consequences

- A person runs one command to see what to fix, a script reads the same
  result as JSON, and the console reads the file the command keeps, so the
  screen and the command do not disagree.
- Blocking is on from the first start and needs no setup. An IPv6 guesser
  is blocked as a whole network, and an administrator cannot be locked out
  by their own typing from the address they are signed in from.
- A person on a network that is not in `never` can be blocked by their own
  wrong passwords for ten minutes, and by a repeat for twice that; an
  administrator at a console, or signed in from an address that is never
  blocked, lifts the block with `firewall unblock`.
- The kernel's list is replaced whole on each change, so authd and the
  kernel never disagree about what is blocked, at the cost of up to 1024
  entries copied on each change.
- `admit` asks one binary search of at most 1024 entries for each new
  connection once something is blocked, and nothing before.
