<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0475. changes bisect halves the changes between two starts with one-start trials

- Status: Accepted; its try also carries the memory test at the start since [0565](0565-updates-are-run-from-the-console-and-a-release-that-fell-back-is-held.md)
- Date: 2026-11-13

## Context

Each start is recorded, and `changes` lists what changed between the
newest failed start and the last good one (ADR 0500). When the list
holds one change, the owner knows what to undo. When an update, a boot
option and two apps came in between, it holds four, and finding which
one broke the start means undoing them one at a time by hand, with a
machine that does not start. The failure notice and Rescue say what
failed (ADR 0501); neither says which change did it.

`git bisect` halves commits, and Windows' Startup Repair and System
Restore go back whole. A trial of a change must not make the change
for good, so that a power cut, a hang or a panic during it costs only
the try, and it must not touch the slot state, which ADRs 0451 to 0453
keep for updates and are held to a model.

## Decision

1. **A lever is a change a start can be made without.** `changes bisect`
   compares the newest failed start, F, with the last good start before
   it, G. The slot is one lever, when F ran one slot and the other slot
   still holds what G ran (its version, kernel hash and firmware set)
   and the boot menu still starts it. The boot options are one, when F's
   differ from G's: a try starts with G's. Each app installed for
   everyone that is new or changed in F and has services is one, which a
   try leaves out. They are ordered the slot, the options, then the apps
   by ID. The rest cannot be tried apart, and the plan lists each with
   why: the machine's hardware (cards, the root's partitions), an app F
   no longer has, an app that runs nothing at start, a slot rebuilt
   since G, options a try cannot hold (400 bytes at most) and anything
   else the record holds.
2. **The owner starts it.** `elevate changes bisect` shows the plan
   (the failed start and its reason, the levers, what cannot be tried,
   the most restarts it takes, that the machine restarts by itself) and
   asks to go on; `--yes` goes on without asking. The failure notice
   gains the line ``To find which change did it: `elevate changes
   bisect`, up to N restarts.``, for an ordinary failed start only.
   Rescue's menu gains "Find the change that broke the start", which
   cannot run the search itself, since each try is a start of the
   installed system, judged by that system's own health check, and
   Rescue keeps no `/system/state` of the installed root: it says which
   start failed, which was the last good one, and what to run once the
   system starts.
3. **The search halves.** Each try undoes the first ceil(n/2) of the n
   candidates left. A try that starts well puts the cause in the half it
   undid; one that fails as F did (the same error ID, by the same
   service or check) puts it in the half it kept. Four levers take two
   tries and eight three. A cause is **confirmed** when a good try undid
   it alone, and named **by elimination** otherwise; for that the result
   offers `changes bisect --confirm`, one more restart that undoes only
   it. A try that fails otherwise than F ends the search with both
   failures shown, since its halves cannot be judged.
4. **A try is a one-start trial.** `boot/grub/trial` on each EFI system
   partition, beside the slot state, is a GRUB environment block of its
   own: `trial_number`, `trial_slot`, `trial_options`, `trial_check` and
   `trial_taken`. GRUB's Dukovo entry, when the block is whole, no update
   is being tried, the slot is the default or the previous one and
   `trial_taken` is not the block's number, writes `trial_taken` first,
   reads the block back, and only if it reads back taken starts the
   trial: that slot, `trial=N` first on the kernel's command line, then
   the menu's own words and the trial's options in place of the slot's.
   So a try starts at most once, and a hang, a panic or a power cut
   during it leaves the next start the usual one. A torn or unreadable
   block is no trial. A trial changes no part of the slot state; the
   menu marks it as a start chosen by hand. `SlotState.tla` models the
   trial, and CI's TLA+ job checks that no trial changes `default`,
   `previous`, `trying` or `tries`.
5. **Spark leaves out a try's apps.** With `trial=N` on the command
   line, Spark reads `/system/state/bisect/try-N`, the names of the
   services to leave out, one to a line, and holds them off as meant,
   with the reason "left out for try N of changes bisect": they show
   Stopped, `service list --failed` and `service status` do not count
   them, and the health check does not wait for one that is essential.
   A file that is missing or unreadable leaves nothing out, a name no
   service has is ignored, and without `trial=N` the file is never read.
   The reading is a few lines of `spark-api`, which the health check's
   writer shares, host-tested.
6. **The health check judges each try** as any start, and records it
   with `trial = N`. `last_good` skips trial records, so a try never
   becomes the last good start, and `changes --list` marks each ("slot A,
   try 2 of changes bisect: good"). The search lives in
   `/system/state/bisect/state.json`, written whole or not at all. Its
   next step follows the verdict: 60 seconds later the health check
   writes the next try and restarts, saying so on the console and in a
   notification, and `changes bisect --stop` stops it before; or the
   search ends. A try with no record, which never reached its health
   check, is tried once more as a new try with the same levers; unfinished
   twice counts as failed. A try the boot menu did not start (the system
   partition is read-only, or the block was spoiled) ends the search,
   which says so.
7. **The end names the change and changes nothing.** "The start fails
   with org.nginx.nginx 1.30.6, installed 2026-11-12 14:03, first
   started at start 41, and starts without it." and, by kind, what
   undoes it: for an app, `elevate bundle rollback ID --for-everyone`
   (its data as it was then) and `elevate bundle hold ID
   --for-everyone`, or `bundle remove` for one that was new; for the
   system, `elevate update undo system` and `elevate update hold
   system`; for the boot options, the two strings and where each is
   set. The result is printed, kept as the sign-in notice `bisect` (ADR
   0504, which gains this writer) with a notification, and shown again
   by `changes bisect --status`. The machine is left running the last
   try; its next usual start is F's again until the owner takes a fix.
8. **Refusals.** No failed start after the last good one; a good start
   since F; no good start kept before F; F's slot is not the default or
   the previous one (a slot G ran that is neither only drops the slot
   lever); an update waiting to be tried (the menu starts it in place of
   any try); a search already waiting on a try; no lever; and
   `--confirm` with no search or with nothing named by elimination.

## Consequences

- Finding which of an update, a boot option and two apps broke a start
  takes at most ceil(log2 n) restarts and no rescue shell: four changes
  take two, and a power cut costs only the try it cut.
- Nothing changes for good while it runs: the slot state, the apps and
  their data are as they were after every search, which the `bisect`
  boot test checks after each of its three runs, and the lab's
  `startup/bisect` on an installed system.
- A try is one start of a slot, so a cause that shows only after hours,
  or only with two changes together, is not found: a try that leaves out
  one change and fails as F did says so, and `changes --failed` lists
  every change.
- The 60 seconds between a try's verdict and its restart are for whoever
  is at the machine to stop it. They are fixed; a setting to shorten
  them for a machine nobody sits at is deferred (docs/deferred.md).
- What Dukovo does better: on Linux, finding which of an update, a
  kernel option and two packages broke a boot is hours with a rescue
  shell, and no system halves the changes between two starts. Dukovo
  names it in at most log2 restarts, with no change to the system until
  the owner picks the fix, and a power cut mid-try costs only that try.
