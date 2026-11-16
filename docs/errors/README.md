<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Error pages

An error that a person can act on carries a stable ID, such as `UPD-103`:
the letters of the area that gives it, a hyphen and three digits. The ID
stays the same from release to release, so it can be searched for, quoted
in a bug report and linked to. Each ID has a page here, named for it
(`UPD-103.md`), which the support center shows under "Error messages" and
which its search finds by the ID ([ADR 0420](../adr/0420-docs-are-tested-like-code.md)).

`ob docs --check` reads the code for the IDs of the areas below. It fails
when the code gives an ID that has no page here, and when a page here is
for an ID that no code gives any more.

## A page

```markdown
# UPD-103: The update does not fit on the disk

What the message says, when it appears, and what it means.

## What to do

The steps that fix it, in order, and how to tell that they worked.

## See also

The man pages and handbook chapters that explain more.
```

## Areas

An area adds its row before its first ID. The letters are new to this
table and say what the area is, in two to five capitals.

| Letters | Area | Since |
| --- | --- | --- |
| `ACC` | Accounts: user(8) and authd's accounts | 0.16.0 |
| `ADM` | The web console, Dukovo Admin: its sign-in, second steps, unlock and requests | 0.16.0 |
| `BAK` | Snapshots and backups: snap(1), backup(8) and moving a machine | 0.16.0 |
| `BOOT` | Starting the computer: the health check of each start | 0.15.0 |
| `DSK` | Disks and storage: storage(8), the disks' health and their checks | 0.16.0 |
| `FW` | Firmware sets and the firmware the kernel loads | 0.15.0 |
| `HW` | Hardware: the checks of `hardware check` | 0.16.0 |
| `MET` | The machine's history and its early warnings: a full disk, a missing one, memory, heat and certificates | 0.16.0 |
| `NET` | Networking: the layers `netcheck` checks, the network settings and the firewall | 0.16.0 |
| `SEC` | Security and health checks: the findings of `dukovo-check` | 0.16.0 |
| `UPD` | Updates of the system | 0.15.0 |
