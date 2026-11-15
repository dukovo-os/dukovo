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
  BIOS, both first-start kill loops, the update boot tests together
  (`update-stage`, `update-power-cut`, `update-panic`, `update-hang`,
  `update-fallback`, `update-offline`, `update-online`,
  `update-network-down`, `update-root-missing`, `update-disk-full`,
  `update-bad-network`, `update-clock-ahead` and `update-clock-behind`),
  the TUF
  conformance suite (`tuf-conformance`, `./ob test tuf`), and each fuzz
  target for ten minutes.
- **A candidate's full run.** Every power cut under each firmware type, in
  eight parts that run at once (`--part k/8`), both kill loops with twenty
  kills each, the same update boot tests and conformance suite, and each
  fuzz target for an hour.
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

- **Kani proves the slot state's rules.**
  - `userspace/programs/slots/src/proofs.rs` states each rule that the
    comments of `State`'s functions promise as a function over one state.
    A Kani harness runs it over a symbolic state whose fields hold every
    value a copy can hold, and a unit test runs it over each of the 648
    states, so that `cargo test` checks the same logic without Kani.
  - The rules are: a slot being tried starts only while it has tries left
    and a start takes exactly one try from it and never adds one
    (`starts`, `started`); a start chosen by hand takes none (`chose`);
    of two copies one generation apart the newer is read, whichever file
    it is in (`newest`); the running slot is never staged or unstaged, and
    a rollback swaps the default and the previous version and is undone by
    a second one (`stage`, `unstage`, `rollback`); a check that passes
    makes the tried slot the default, one that fails leaves it no tries,
    and a check never adds a try (`checked`); and a slot forgotten is
    named nowhere (`forget`).
  - No harness formats a string, so that each ends in seconds. That a copy
    reads back as it was written is checked by the unit test of every state
    and by the `slot-state` fuzz target.
  - `./ob test kani` checks `slots` besides the kernel's libraries, with a
    host build of the programs' workspace in `target/kani-programs`, and
    the `kani` job of `ci.yml` runs when `userspace/programs/slots/`
    changes.
- **Fuzzing.**
  - cargo-fuzz runs nine targets, in `userspace/programs/fuzz`, a
    workspace and a lock of their own: `release-list`, `tuf-root`,
    `tuf-targets`, `tuf-snapshot`, `tuf-timestamp`, `rollout`,
    `advisories`, `canary-report` and `slot-state`. The TUF targets feed
    the trusted-metadata update, from a fixed root with fixed keys and
    the metadata that follows from it, so that the signature checks run on
    mutated input. A target with an encoder checks that what it reads it
    writes back and reads again as it was.
  - A seed corpus for each target is kept in git, written from the
    fixtures by `cargo run --bin seeds`.
  - `./ob test fuzz [--target NAME] [--seconds N]` runs the targets, 60
    seconds each without `--seconds`, on the corpus in
    `.cache/fuzz/NAME/`, which starts from the seeds. A crash fails the
    run and leaves its input in `out/fuzz/NAME/`. `tools/xtask/src/doctor.rs`
    pins cargo-fuzz as it pins Kani, and `ob doctor` says how to install
    it.
  - `update-matrix.yml` has a job for each target, which keeps its corpus
    in the Actions cache, by target, so that it grows from night to night,
    and uploads the input of a crash. Its `tuf-conformance` job runs
    `./ob test tuf` beside them. The jobs that need no QEMU skip the
    boot tests' packages and ports.

- **Updates meet a broken machine.**
  - An essential service that fails is `update-fallback`'s case: the
    update's logd is broken, its health check fails, and slot A starts
    again with the settings from before it.
  - `update-network-down`: the machine's network card is on a QEMU hub
    with no other port, so netd never gets an address. `sysupdate check`
    against a server it cannot reach fails within its time limit, naming
    the server and why, and changes nothing. An update staged from the
    updates' disk then proves itself, as the health check does not wait
    on the network.
  - `update-root-missing`: the test update `noroot` names, among its
    flags, a `root=UUID=` that no disk holds. GRUB puts a slot's flags
    after its own `root=`, and Spark takes the last one, so each start of
    slot B finds no root, says so and restarts after 30 seconds. The
    three tries are used up, and slot A starts with the settings from
    before the update.
  - `update-disk-full`: a file written as root takes the root's free
    space. `sysupdate stage` adds up the sizes of the files the release
    changes, an eighth more for Btrfs's metadata and 16 MiB, and refuses
    before it writes anything when the root has less free, saying what it
    needs and what is free. With the file removed, the stage goes through
    and the update proves itself. The stage reads each file of the
    running slot once, for this sum and to choose what it writes.
  - `update-bad-network`: `update-online`'s test server is at fault for
    the whole fetch. Every answer comes after a wait of a second, the
    first 64 KiB of its body 2 KiB at a time, 50 ms apart, which is far
    below the 30 seconds the client gives a read. The first request for
    each file of the release is answered 503, and the connection of every
    third answer with a body of more than 300 bytes is dropped after 300.
    Each file takes its four tries at most: a 503 and a dropped
    connection count as a try, and a dropped file goes on from where it
    stopped. The update stages the files the slot lacks, each once,
    starts at the restart and proves itself. The test checks that the
    server did each of these things.
  - `update-clock-ahead` and `update-clock-behind`: timed is given no
    servers, so the clock stays as QEMU's `-rtc base=` sets it, a year
    (365 days) ahead or behind the host's. A year ahead, `sysupdate
    check` refuses the timestamp, which is good for a day, and says when
    it expired, what the computer's clock says and that the clock is to be
    set with `timedatectl set-time`; no file of a release is fetched, and
    `sysupdate status` is as it was. A year behind, TUF has no rule that
    refuses the metadata, so the check works, a replayed older timestamp
    is still refused by its version, and the update stages, starts and
    proves itself.

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
- A run of the full matrix is twenty-nine jobs of the runners' time, and
  the nightly sample fifteen.

- **Not yet** (the next part of this decision):
  - The upgrade paths from old releases, on a machine full of users,
    settings, apps and services, built once for each release as a fixture
    image.
  - The gate: `ob release sign` checking that a candidate's full run is
    green for the exact commit.
