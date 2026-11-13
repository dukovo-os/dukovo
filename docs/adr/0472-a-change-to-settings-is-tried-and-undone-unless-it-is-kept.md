<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0472. A change to settings is tried, and undone unless it is kept

- Status: Accepted
- Date: 2026-11-13

## Context

Most of Dukovo's servers are reached over the network. A change to the
settings of the SSH server, the network or the firewall that goes wrong
cuts off the session that made it, and the machine then needs a person
at its screen. Cockpit and Webmin apply such changes blindly. Network
gear has long had the answer: Junos's `commit confirmed` rolls a change
back unless it is confirmed within minutes, and Ubuntu's `netplan try`
does the same for a network's settings alone. systemd has nothing like
it for a unit.

The web console's network and firewall pages (Phase 16, C5) need the
same safety, so the mechanism must not belong to one command.

## Decision

1. **Spark keeps and undoes trials.** A `try` request names settings
   files, a time and the services that read them. Spark keeps a copy of
   each file before the change is written, and puts the copies back
   unless `keep` comes within the time: each copy over its file, whole
   or not at all and with its mode, owner and group; each file that was
   not there removed. It then restarts each service that ran when the
   trial began, stops each that did not, and asks each `reload` service
   to reload. `undo` does this at once. Spark, which runs whatever
   happens to a session, holds the trial, not the program that began
   it, so a session that a change cuts off leaves the undo to Spark.
2. **Failure undoes at once.** A service the trial restarts or reloads
   that fails to start, or fails its health check, with the change
   undoes it at once rather than at the end of the time.
3. **A restart undoes it too.** The copies and a record of the trial are
   kept on the disk, in `/system/state/spark/trials`, which only root
   reads, the record written last. A start of the machine that finds a
   trial undoes it before it reads any service's file, so a change that
   hangs the machine is undone by a power cut. Rescue undoes nothing,
   leaving it for the next start.
4. **Only settings.** A trial names at most 16 files, of at most 1 MiB,
   beneath `/system/settings`, by plain paths: no `..`, no hidden file,
   no space. A file that is a link, or beneath one, is refused, so what
   Spark writes back is the file the path names. Four trials run at
   once at most, a file is in one at a time, and the time is 10 seconds
   to an hour. Each trial's beginning, keeping and undoing goes to the
   audit log.
5. **`service try NAME`** edits a service's file as `service edit` does,
   begins the trial `service.NAME` with the file, restarts the service
   and waits for it as `service restart` does, then asks whether to keep
   the change. `service keep NAME` keeps it from any session, and
   `service undo NAME` undoes it. Without a terminal it returns once the
   service has restarted, for a script to keep the change once it has
   checked it. `show` lists the trials that run and the last eight
   undone, so `service status`, the web console and alertd can say what
   is being tried and what was undone.

## Consequences

- A change that cuts an administrator off comes back by itself within
  a minute, or at the next start; Dukovo is the first server system to
  do for its services' settings what routers do for theirs.
- The web console's network and firewall pages begin their own trials
  with the same request, and so would any later settings page.
- A service whose first start with a change fails, but would have
  passed on a later try, has the change undone; the person tries again.
- With service check (ADR 0471), Spark is 775 KiB, 42 KiB less than before
  them; the startup archive stays well under its 1 MiB.
- What Dukovo does better: Junos and netplan try one kind of settings
  each; Dukovo tries any settings with the services they belong to,
  undoes the change as soon as a service fails with it rather than only
  on a timer, and survives a power cut during the trial.
