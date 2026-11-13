<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# What each alert means

This page lists the alerts Dukovo raises, under the words of their
titles, with what each means and what to do. An alert's own detail, in
the message or in `elevate alerts list --json`, names the first command
to run. The chapter "Alerts: email, ntfy and webhooks" explains how
alerts reach you. Each heading is a title as the alert says it; the
words in capitals stand for a name or a number that differs.

An alert's key, which `alerts list --json` shows, is the part before the
colon in its section's note. An alert clears by itself once what caused
it is gone, and you hear that it has, unless a channel says
`cleared = false`.

## A disk

### Disk NAME needs a look

A warning, key `smart:SERIAL`. The disk's own health record has something
to say that is not yet a failure: sectors were reallocated, are waiting
to be, or could not be read; SMART is off; an NVMe drive is too hot, has
used its rated endurance or has had media errors. Run
`elevate diskhealth NAME` for the reasons and the attributes. Back up
what is on it if you have not, and watch it: disks often fail weeks after
these counts start to rise. The alert clears when the verdict is
*PASSED* again.

### Disk NAME is failing

Critical, key `smart:SERIAL`. The disk says it is about to fail: an
attribute that warns of failure is at or below its threshold, or an NVMe
drive reports a critical warning. Copy what matters off it at once
(`elevate backup run` if a destination holds its backups), and replace
it. For a disk in a Btrfs volume, device(8) adds a new disk and removes
this one.

### Disk NAME has N more reallocated sectors since DATE

A warning, key `smart-rising:SERIAL`; the count named can also be sectors
waiting to be reallocated, offline uncorrectable sectors, or an NVMe
drive's media errors. It is an early warning: the disk still passes, but
a count that tells of a failure to come went up between two looks. It
stays for a week after the last rise. Treat it as the warning above:
back up, run `elevate diskhealth NAME`, and plan to replace the disk.

### Disk NAME has used 90% of its rated endurance

A warning, key `smart-rising:SERIAL`. An NVMe drive reports it has written
about as much as its maker rated it for. It keeps working for some
time, but plan its replacement before it stops.

## Backups

### The last backup to DESTINATION failed

A warning, key `backup-failed:DESTINATION`. The last run to that
destination did not finish. `elevate log backup` says why, and `backup
show DESTINATION` shows the destination. The usual causes are a disk or
share that is not there, a full destination, and a server that cannot be
reached. Fix it and `elevate backup run DESTINATION`; the alert clears
when a backup finishes.

### Restoring DESTINATION's newest backups to check them failed

A warning, key `backup-failed:DESTINATION`. The check that restores
the newest backups to prove they can be read did not pass. Treat the
destination's backups as unproved: `elevate backup verify --restore
DESTINATION` runs the check again, and `elevate log backup` says what it
found.

### The last backup to DESTINATION was N days ago

A warning, and critical once it is twice as old as the sign-in allows
(6 days for a destination that backs up every day, 28 for one every
week); key `backup-overdue:DESTINATION`. Nothing has backed up to it for
longer than it should. Check that its disk is plugged in or its server is
up, then `elevate backup run DESTINATION`.

### Nothing has been backed up to DESTINATION in the N days since it was added

The same alert for a destination that has not had its first backup.
Run `elevate backup run DESTINATION` and read `elevate log backup` if it
fails.

## Updates and starts

### An update failed its health check, and Dukovo went back to RELEASE

Critical, an event with the key `update-fell-back`. The machine started
the new release, a service it needs did not come up, and the system
restarted into the release before it. The machine is working, on the old
release. The detail names the check that failed. `elevate sysupdate
status` shows where the update stands; report the failure with the log
(`elevate log sysupdate`) before trying again.

### This start of RELEASE failed its health check, and goes on

A warning, an event with the key `start-failed`. A start that was not an
update failed the health check of an essential service, and the machine
went on running. `service --status-all` shows what is not running, and
`elevate log sysupdate` says what the check saw. Until a start is seen
through, the boot menu shows at the next start, so the machine can be
started otherwise (dukovo-rescue(8)).

### An update failed its health check, and RELEASE could not be started

Critical, an event with the key `update-failed`. The update failed its
check and the system could not restart into the release before it. The
detail says why and names `elevate reboot`, which starts the previous
release by hand. If that does not work, Rescue reinstalls the release
(dukovo-rescue(8)).

### An update could not be staged

A warning, an event with the key `update-not-staged`. An unattended
machine found a newer release and could not prepare it in the spare slot.
The detail says why; the next check tries again. `elevate sysupdate
status` and `elevate log updated` say more.

### The system's files are not the release's

Critical, an event with the key `update-not-verified`. The weekly check
that reads the running system back against its release's signed list found
files that differ. Run `elevate sysupdate verify`, which says what
differs, and if it fails again, Rescue reinstalls the release
(dukovo-rescue(8)).

### The machine restarted after a kernel panic

Critical, an event with the key `kernel-panic`. The kernel stopped itself
and the machine restarted. The detail names the saved dump in
`/system/state/crash`; `elevate dumps show` shows it, and "Debugging the
kernel" says what to put in a report.

## Services

### Service NAME failed; Spark gave up on it

Critical, key `service:NAME`, raised when a service has not been as asked
for a minute. Spark tried to restart the service and stopped trying.
`service NAME status` and `elevate log NAME` say why it stopped. Fix the
cause and `elevate service NAME start`.

### Service NAME stops and waits to start again

A warning, key `service:NAME`. The service ends and Spark waits before it
starts it again, as a service that keeps failing does. `elevate log NAME`
shows why it ends. If you stopped it on purpose, `elevate service NAME
disable` ends the alert.

### Service NAME is stopped and should run

A warning, key `service:NAME`. Spark refused the service or it ended for
good, and its file says it should run, for more than a minute. The
detail says why, such as a service it requires that is stopped.
`service NAME status` shows the same, and `elevate service NAME start`
starts it once the cause is fixed.

### Service NAME has not started

A warning, key `service:NAME`. It is waiting for a service it needs, and
has waited more than a minute. `service NAME status` names what it waits
for; that service is the one to look at.

### Service NAME is not ready

A warning, key `service:NAME`. The service runs, but has not said it is
ready for more than a minute. `elevate log NAME` shows what it is doing.

### Service NAME is not done stopping

A warning, key `service:NAME`. Spark asked it to stop more than a
minute ago and it is still not stopped, though Spark ends one by force 5
seconds after it asks nicely. Something holds it. `elevate service NAME
stop` asks again, and `elevate reboot` clears it if that does not.

### Service NAME fails its health check

A warning, key `service:NAME`. The service runs, but the check its file
names (`health` in service.toml(5)) fails. `service NAME status` shows the
last failure.

## The machine's history

The alerts that metricsd raises from the history it keeps are described in
metricsd(8) and metrics.toml(5), which also set their levels:
`fs-full:MOUNT` and `fs-filling:MOUNT` for a file system that is nearly
full or will be soon (`df -h` and `du` show what is using it),
`btrfs-missing:VOLUME` and `btrfs-behind:VOLUME` for a Btrfs volume that
lacks a disk or has one that is behind, `memory:low`, `temp:SENSOR` for a
part that is hot, and `certificate:FILE` for a certificate in
`/system/settings/ssl` that ends soon.
