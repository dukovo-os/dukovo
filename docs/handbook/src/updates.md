<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Updates, snapshots, backups and recovery

This chapter describes how Dukovo updates its system, and what happens
when an update goes wrong. The full references are sysupdate(8),
health.toml(5), spark(8) and dukovo-boot(7); ADRs 0450 to 0459 record
the design.

An update never leaves the machine unable to start, never touches your
files, and never installs or restarts without your say. A new version is
built beside the running one, tried at a restart you choose, and kept
only if it proves itself; anything short of that brings the version you
had back, by itself.

## Two slots

An installed Dukovo keeps its whole system, everything under `/system`
except the settings and the state, in a slot: a read-only Btrfs
subvolume, `@system.a` or `@system.b`. One slot runs. The other holds
the version before, or an update waiting for a restart. Each slot holds
its own kernel, startup archive and boot menu, so a slot changes all at
once.

`sysupdate status` shows where things stand:

```
$ elevate sysupdate status
Running:  slot A, Dukovo 0.15.0
Slot A:   Dukovo 0.15.0, the default
Slot B:   Dukovo 0.14.1, the previous version
Next start: slot A
Last health check: passed, 2026-11-11 09:14: slot A (Dukovo 0.15.0)
Slot state: on nvme0n1p1 (generation 1)
```

## Staging an update

`sysupdate stage` builds a release into the spare slot while the system
runs. It starts from a snapshot of the running slot, so unchanged files
cost no space, writes only the files that changed, reads every file
back against the release's signed list, and seals the slot read-only.
It also snapshots the settings and the state. Until its very last step,
a power cut leaves the machine exactly as it was.

```
$ elevate sysupdate stage /volumes/UPDATES/dukovo-0.15.1
sysupdate: checking the release against the root this system trusts
sysupdate: building slot B from slot A and the release's changes
sysupdate: wrote 41 changed files; reading the slot back against the signed list
sysupdate: snapshotting the settings and the state, for a fallback
sysupdate: staged Dukovo 0.15.1 in slot B: it starts at the next restart, with 3 tries to prove itself, and Dukovo 0.15.0 starts again if it does not
```

Nothing else happens until you restart. To change your mind first,
`elevate sysupdate unstage`.

A release must be signed by the root your system trusts and newer than
the version that runs. Releases come from a repository folder today, as
`ob release` makes one; downloading them from the update server comes
with the update command.

## Trying and proving

At the restart, the boot loader starts the update as a try, and counts
the try before it starts it, so a hang, a panic or a power cut each
spend one of its three tries. Then the health check watches the
essential services: logging, sign-in, the consoles, the network and SSH.
When they have all run for two minutes with no restart, and any checks
you require pass, the update proves itself. It becomes the default, and
the version before stays in the other slot as the previous version.

You can ask more of an update in `/system/settings/sysupdate/health.toml`,
such as your web site answering:

```toml
[[check]]
name = "web site"
command = ["curl", "-fsS", "http://localhost/"]
required = true
```

health.toml(5) describes every setting.

## When an update fails

An update falls back to the version you had when:

- an essential service keeps restarting, or stops after a failure;
- the essential services are not all running within ten minutes;
- a required check fails;
- or it runs out of tries: a kernel that panics restarts by itself after
  10 seconds, and a system that hangs is restarted by the hardware
  watchdog, where the machine has one.

The version before then starts with no keypress, and before any service
starts it puts back the settings from before the update. The settings
the failed update had are kept in `@snapshots/update/settings-failed`,
for a look at what went wrong, and the log says why it failed:

```
$ elevate log sysupdate
sysupdate: slot B (Dukovo 0.15.1) failed its health check: logd has restarted 3 times
sysupdate: the update gives up its tries; restarting into slot A (Dukovo 0.15.0)
```

## Going back later

If you find a problem days later, `elevate sysupdate rollback` makes the
previous version the default from the next restart on. Your settings
stay as they are: each release writes only what the release before it
can read, so nothing you changed since the update is lost.

## The boot menu

The boot menu stays hidden unless you hold Shift as the computer starts,
or the last start did not get as far as the health check seeing it
through; then it shows for 10 seconds and starts Dukovo by itself.
Besides the version that starts, it offers the previous version, which
starts once without becoming the default, and, while an update is being
tried, the version without the update.

## What is on the way

Rescue, a repair system on the EFI system partition for a machine whose
two slots are both broken; snapshots with `snap`; backups; and
downloading updates from the update server in waves, through one
`update` command for the system, firmware and apps. This chapter grows
with each.
