<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0543. Rescue follows each release that proves itself, and reinstalls from the update server

- Status: Accepted
- Date: 2026-11-14

## Context

Rescue (ADRs 0459 and 0460) is a whole release kept on the EFI system
partition, which the boot menu starts when the system cannot. The
installer wrote it once, and nothing wrote it again: a machine installed
from 0.15.0 and updated for two years still had 0.15.0's Rescue. Its
reinstall then put 0.15.0 back, over settings and state that newer
releases had moved on, and its TUF root could be one the project has
since rotated away from, so that it trusted no current metadata.

Its reinstall had two sources: its own copy and an update folder or file
on a disk. A server in a rack, or a laptop with no stick at hand, has
neither of the second; it has a network, and the update server it
followed.

## Decision

### Rescue follows the release that runs

1. **When.** Once a start is seen through while Rescue holds an older
   release than the one that runs, the first of them being the start that
   proves an update, the health check asks the stage, `sysupdate-stage`,
   to write Rescue, by a `rescue` request (sysupdate(8)), and goes on
   without waiting. A start that is a try of a change search, or one
   after which a search restarts the machine, asks nothing. A firmware set
   staged alone changes no release. Rescue keeps a newer release after a
   rollback: its reinstall then gives the newer release, which the owner
   can stage again.
2. **What it writes.** The boot archive and kernel of the running slot,
   each file checked against the release's signed list as it is written;
   the signed metadata the stage kept of the release, or, for a release
   the stage did not keep, the client's own metadata with every root it
   walked, which the TUF client now keeps apart from its role files; and
   `release/version`, the release's name. Rescue therefore trusts the
   update server's keys after a rotation, as the running system does.
3. **How.** At idle disk priority, on each EFI system partition of the
   root's disks, with the stage's lock held. The copy is read back in
   1 MiB pieces, within the stage's 256 MiB memory limit, before it takes
   Rescue's place. A partition with room for two copies gets the new one
   beside the old, switched in by the renames of ADR 0460; one without,
   as the default 512 MiB partition with a 230 MiB release, gets it in
   the old one's place, once the slot it is written from has been read
   back whole, since the slot is then the copy the machine falls back to.
   A partition without room even for that, or whose write fails, keeps
   what it has: UPD-102 says why, and the next start tries again.
4. **What says so.** `sysupdate status` names the release Rescue holds
   (`/system/state/sysupdate/rescue`), or says it was not written since
   the install.

### Rescue reinstalls from the update server

5. **Where from.** The reinstall's prompt takes `server` beside Enter (its
   own copy) and a path. Rescue reads the server and channel from the
   root's `updates.toml`, and turns networking on, unless it is on, with
   the root's network and firewall settings, as SSH into Rescue does (ADR
   0540), so that a machine with fixed addresses reaches the server as it
   did. It turns networking off again after.
6. **Whom it trusts.** The root's own TUF metadata, which the installed
   system's updater kept, with its record of the newest timestamp,
   snapshot and targets, so that an older release cannot be passed off as
   current. When it is missing, or does not read, Rescue sets it aside
   and starts from the root its own release trusts, which, by 1 to 4, is
   the newest the system proved; each newer root is read from the server,
   signed by the one before, as an update reads it.
7. **Which release.** The channel's, when the release the default slot
   held updates to it directly or is it; else the bridge it needs first,
   as an update takes, and the system updates on from there. When the slot
   is gone, the release of Rescue's own copy stands for it. A channel that
   offers an older release than the slot held is refused. Waves (ADR 0469)
   and holds (ADR 0502) are for updates, and this is asked for.
8. **How.** Rescue says how many files it fetches, and their size, before
   it asks; it fetches only those the slot lacks, onto the root's disk in
   the root's `/system/state/sysupdate/files`, and builds the slot as its
   other reinstalls do, every file read back against the signed list.
   What it read of the server stays in the root's state, where the
   installed system's updater reads it next.

## What it is not

- Rescue does not update itself in place while it runs, and the boot menu
  does not choose between Rescue copies: there is one per partition.
- Rescue does not reach Wi-Fi: its networking is the wired cards'.
- The reinstall from the server is not a try: with no slot left to start
  there is nothing for a try to fall back to, and Rescue stays there if
  the reinstalled slot fails.

## Alternatives

- **Write Rescue at each stage, before the release proves itself.** A
  release that fails to start would then also be Rescue's, the one copy
  that must start.
- **Two Rescue copies, the old kept until the new one is proved.** Twice
  the room, which the default 512 MiB partition does not have, and the
  boot menu would need to choose between them.
- **Rescue trusts only its own copy of the metadata.** It would miss a
  rotation since it was written and lose the rollback protection the
  root's updater built up.

## Consequences

- A reinstall from Rescue puts back the newest release the machine
  proved, not the one it was installed with.
- Each proved update writes about 230 MiB to each EFI system partition,
  once, at idle priority.
- On a 512 MiB partition there is a short time, between the removal of
  the old copy and the rename of the new, when the partition holds no
  Rescue; the slot just read back is whole then, and a cut there leaves a
  Rescue that the next start writes again.
- The boot tests update-stage and update-online check that Rescue holds
  each release they prove, and the lab tests updates/online and
  updates/rescue-from-server check the same over the lab network and the
  reinstall from the server.
