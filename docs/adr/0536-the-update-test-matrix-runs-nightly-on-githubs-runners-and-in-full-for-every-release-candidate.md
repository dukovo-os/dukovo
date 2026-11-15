<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0536. The update test matrix runs nightly on GitHub's runners, and in full for every release candidate

- Status: Accepted
- Date: 2026-11-15

## Context

ADR 0450 has a test matrix behind the gates of promises 1, 2 and 7, which
a release candidate runs in full and each night runs a sample of, and
says it runs on a self-hosted runner. Three things argue against that:

- **Safety.** A self-hosted runner on a public repository runs whatever a
  workflow on it is told to. GitHub advises against it, since a pull
  request from a fork can reach it unless every workflow is locked down,
  and a mistake there gives a stranger a shell on a machine at home.
- **Reliability.** A machine at home sleeps, is shared with other work
  and is switched off now and then, and the matrix stops when it does.
- **Cost and scale.** GitHub's standard runners are free for a public
  repository, have KVM, which the boot tests already use, and run many
  jobs at once, so the matrix can be shared out in parts that each end
  within the six hours a job may take.

Promise 2, that a power cut at any instant leaves the old system or the
new one and never a mix, was proven by `update-power-cut` alone, which
cuts one stage at one point. The slot state is one small write kept in two
copies and a slot is sealed before anything names it (ADR 0451), but
no test covered every order in which a disk may have kept the writes of an
update. The first start of an update, which GRUB takes as a counted try
(ADR 0452), was only tested as it fails by its own doing (ADR 0453).

## Decision

- **The matrix runs on GitHub's standard runners.** `ubuntu-24.04`, and no
  larger, billed runner. Real hardware stays the lab's job, as the
  hardware campaign at the end of Phase 16 plans.
- **`.github/workflows/update-matrix.yml`.**
  - It runs each night on main, a sample, and by dispatch with `full` on a
    release branch, for a release candidate. It holds `contents: read`.
  - A `plan` job runs `./ob test matrix --list`, or with `--full`, which
    prints the jobs as one line of JSON. A `run` job takes each as an
    entry of its `strategy.matrix`, with `fail-fast` off, so that every
    job runs to its end and one failure hides none, each on a runner of
    its own, for up to 340 minutes.
  - An entry is a name, the `./ob` command it runs, and, if it needs one,
    the environment it runs in. What runs is written once, in
    `tools/xtask/src/test/matrix.rs`, whose unit tests check that no job
    is listed twice and that each case a job names exists.
  - A job that fails uploads its serial logs, the marks of its replay and
    the disks of the cuts that failed, compressed with `zstd -19` when
    they come to under 2 GB on disk.
- **The nightly sample.** Sixteen power cuts under UEFI and sixteen under
  BIOS, both first-start kill loops, and the update boot tests together
  (`update-stage`, `update-power-cut`, `update-panic`, `update-hang`,
  `update-fallback`, `update-offline` and `update-online`).
- **A candidate's full run.** Every power cut under each firmware type, in
  eight parts that run at once (`--part k/8`), both kill loops with twenty
  kills each, and the same update boot tests.
- **Power cuts are replayed from QEMU's write log.**
  - The boot disk of the `update-power-replay` boot test, and of its BIOS
    twin `update-power-replay-bios`, sits behind QEMU's `blklogwrites`
    driver, which logs every write, zero write, discard and flush that
    reaches it in the format of dm-log-writes, after the disk has done it.
    The log is therefore in the order the disk completed the writes, and
    every write the guest saw complete before it asked for a flush comes
    before that flush.
  - The test stages the update while slot A runs, restarts into slot B and
    lets it prove itself, and marks in a file beside the log how many
    entries the log held after a `sync` before the stage and after one at
    the end.
  - `./ob test replay` takes the entries between the marks and rebuilds
    the disk as each of these cuts would have left it, from a sparse copy
    of the disk as the recording started: at each flush, with everything
    logged before it; and, for each interval between two flushes that holds
    two or more writes, with everything up to the first flush and a part
    of the writes after it, never none and never all, since a disk may
    keep any of the writes it was not told to flush. A seed draws the part,
    and a sample, which always holds the first and the last flush, is drawn
    by it too. `--points all` boots every cut, `--points 16` a sample,
    `--part k/m` every m-th, and `--seed` repeats a run.
  - Each cut boots on a snapshot, with the same disk of test updates. It
    must start slot A or slot B and nothing else, with no kernel panic and
    no Btrfs error. `sysupdate status` names that slot and its version as
    the running one; `sysupdate verify` reads it back against its signed
    list; and on slot A a stage goes through, which first removes a slot
    the cut left half built.
  - A cut that fails keeps its disk, the serial log, the write log, the
    marks and the disk the recording started from in `out/test/replay`, and
    the run prints the command that boots that cut again without
    recording: `./ob test replay --case NAME --log PATH --point N --seed
    S`. Every cut runs, even after a failure.
- **The first start is killed again and again.** `update-first-start-kills`
  and its BIOS twin stage the update, restart into it, and reset the
  machine after a random 1 to 40 seconds, 6 times, or as many as
  `OB_TEST_KILLS` says. The seed of the times is printed, and
  `OB_TEST_SEED` sets it. The machine is then let start, and must start slot
  A, once the update has used up its tries, or slot B, if it proved itself
  between two kills, which `sysupdate status` and `sysupdate verify`
  confirm.
- **The updates' disk holds the installed release's own list.** Slot A is
  read back against a signed list of the release it runs, which a test
  image, with no Rescue, has nowhere to take from. The disk of test
  updates holds a repository of that release, `current`, beside theirs,
  with no files in its store, since `verify` reads the files of the slot.

## Consequences

- Every change to what an update writes, or the order it writes it in, is
  tested against every state a power cut can leave, on the next night's
  sample or a candidate's full run, where `update-power-cut` tested one.
- A random test is repeatable: the replay's seed and cut number, and the
  kill loop's seed, are printed, and a kept log boots one cut again in the
  time of one boot.
- A part of the writes between two flushes is stricter than most disks,
  which keep the writes of a flush interval in order. A failure there may
  be a request the filesystem made without a flush that it needed, and it
  is read as one before it is dismissed.
- The recording logs only the boot disk. The test updates' disk is left
  as it was.
- The first start's kills take their time from a draw, so a kill loop has
  no fixed length: its budget is the longest it can take with twenty
  kills.
- A run of the full matrix is nineteen jobs of the runners' time, and the
  nightly sample five.

- **Not yet** (the next part of this decision):
  - The upgrade paths from old releases, on a machine full of users,
    settings, apps and services, built once for each release as a fixture
    image.
  - The cases that break an update on purpose or hurt the machine around
    it: an essential service that fails, a network that stays down, a root
    that will not mount, a full disk, a bad network and clock skew, and the
    metadata attacks of `update-online` beside TUF's conformance suite.
  - Kani on the reader of the slot state, and the fuzz targets for release
    lists, TUF metadata, the rollouts, the advisories index and the canary
    report.
  - The gate: `ob release sign` checking that a candidate's full run is
    green for the exact commit.
