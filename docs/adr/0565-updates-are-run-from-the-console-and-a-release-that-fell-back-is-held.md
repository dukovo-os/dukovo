<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0565. Updates are run from the console, and a release that fell back is held

- Status: Accepted
- Date: 2026-11-16

## Context

The system updates with one command, `update`, and one service,
updated(8) (ADR 0502); bundled(8) updates the apps (ADR 0384); a start
that fails its health check goes back to the version before, and
`changes bisect` finds the change that broke a start (ADR 0475);
hardware(8) and the start menu's memtest86+ test the machine (ADR
0488). Each is run at a terminal. The web console's Updates, Apps and
Hardware pages run them for an administrator, often from a phone, and
need what a terminal reads off the screen: what the last check found,
the settings with their defaults, the checks after an update, and a
restart that a person can see coming and stop.

Putting the updates on a page also showed two gaps in what the machine
does by itself:

- A fallback held nothing. A machine that updates by itself staged the
  same release again at its next daily check and fell back again, each
  day.
- A wanted check of health.toml(5), one that is not required, was only
  logged when it failed after an update: the update stayed and nobody
  saw why a service was not answering.

Five more things were wanted that the commands could not do: keep a
reason with a hold, put a reminder off for a day or a week, see what an
app's data takes on disk, have an app updated in the night rather than
at once, and test the memory once at the next restart.

## Decision

### Reads and changes

- Every part of the pages is an action of admind that runs the command
  a person would type. Reads run as the person: `update last`, `update
  settings`, `update checks`, `update check`, `changes --failed`,
  `changes bisect --status`, `bundle list`, `bundle info` and `shutdown
  --show`, each with `--json`. The slots (`sysupdate status --json`) are
  an administrator's read, with a line in the journal, never on a
  timer. Every change is an administrator's, through elevate.
- `update last` prints what the last check run as root found, kept by
  updated's daily check and by `elevate update check` in
  `/system/state/sysupdate/last-check.json` (0644, replaced whole). A
  page that loads shows it and does not ask the update server; Check
  now asks.
- `update set` writes the keys of updates.toml(5) that the page offers
  with the writer that keeps the file's comments and order, and refuses
  a restart time without `unattended`. Whether the apps update by
  themselves is said twice, by updates.toml's `apps` and by bundled's
  own switch (`bundle update --automatic`), from before one command
  ran both; each is read by a different program and by older releases.
  `update set --apps` writes both, so that they agree, and `update
  settings` says when they do not.
- `update checks add`, `remove` and `essential` write health.toml(5)
  with the same writer: a URL to fetch, a service that must run, or a
  command, each required or wanted.
- `update apply --only ITEM` stages the system, the firmware or the
  apps alone, so that the page can offer each where it is shown.

### Restarts with a minute's notice

- Restart and Shut down (`power.restart`, `power.shutdown`) run
  shutdown(8) for a minute from when they answer, or for a time asked
  for. The minute lets admind answer and the page count down before
  Spark stops admind, and gives a person who clicked by mistake the
  time to cancel (`power.cancel`). Spark's read socket says what is
  set, and the open pages are told when it changes (`event: power`).
- An action that restarts the machine says so (`RESTARTS`): Restart,
  Shut down and the search's actions. admind refuses it while a trial
  of settings runs, which a restart would undo half way (ADM-142), and
  while the other kind of restart is set or runs (UPD-106): a restart
  while a search waits on a try, or a search while a restart is set.
  When admind cannot tell, because Spark does not answer or the
  search's state does not read, it refuses all the same.
- `GET /api/1/actions` says `"restarts": true` for such an action. The
  Ansible task of a restart or a search is followed by a wait for the
  machine to answer again; a shut down's is not, since nothing turns
  the machine on again but its power button.
- The page then waits for the console to answer: ten minutes before it
  says the machine has not come back, and longer when the memory is
  tested at that start (below).

### A release that fell back is held

- On the path that tells alertd `update-fell-back`, the health check
  also holds the release it fell back from: `system` under `[hold]` in
  updates.toml, written with the same writer as `update hold`, unless a
  hold there names that version or a newer one; and
  `/system/state/sysupdate/held.json` (0644), which says that the hold
  came from the fallback, of which start and when. The failure notice
  and the alert gain one line: the release is held until someone
  releases it (UPD-105). `update unhold system` removes both.
- `update check --json` says why a held offer is held (`held_because`,
  `fell-back` or `asked`) and since when.
- A hold holds one version. A newer release is taken as usual, so the
  release that fixes what made the start fail is not blocked.
- A hold that cannot be written is logged and said in the notice, with
  the command that holds it; the fallback has happened all the same.
- The health check's service view holds `/system/settings` to write, as
  a whole: every service reads the settings, and a view cannot hold one
  folder of them apart. It adds little to a service that already writes
  the system slots and the EFI system partitions.

### A wanted check that fails is told

- When a wanted check fails at the start that tries an update, the
  health check tells alertd the event `update-check-wanted` (a warning,
  with the check's name, the version and UPD-104) through the same
  client as the fallback's alert. An event, not an alert: the update
  stays, and there is nothing to clear. A start that is not an update's
  try only logs it, as before.

### The search from a page

- The Updates page begins, stops and confirms `changes bisect` for an
  administrator. The first try's restart is asked for a minute later,
  so that the page is answered and counts down; the health check
  restarts for each try after that by itself, as at a terminal. The
  search's state is read as the person after each restart.
- A plan reads the slot state, which only the system may read, so the
  health check keeps the search it plans when a start fails beside the
  search's state, and `changes bisect --status --json` gives that plan's
  changes before the search begins: the page offers the search from it.

### A hold keeps its reason

- `update hold ITEM --why TEXT` and `bundle hold ID --why TEXT` keep a
  reason with the hold: one line of 1 to 200 characters with no control
  character, and who gave it. The system's and the firmware's are kept
  in `/system/state/sysupdate/hold-reasons.json` (0644, replaced whole)
  and belong to the version the hold names: a hold at another version
  or a release drops it, and a hold again with no reason keeps it. An
  app's is the note of the `Held` event in bundled's history, so no
  column is added to the registry's files.
- Not in updates.toml: an older sysupdate reads that file strictly and
  would refuse a key it does not know, and `update undo system
  --keep-settings` hands today's file to an older release. The state
  under `/system/state/sysupdate` is never read by one.

### Remind me later

- `update later ITEM --for 1d | --for 7d | --off` puts off the reminder
  of the version offered now, for a day or a week, and keeps it in
  `/system/state/sysupdate/later.json` (0644), with who asked and when.
- One rule decides whether it stands, for every reader: only while the
  time has not passed, the version offered is the one put off, and no
  advisory says that version fixes a flaw. When the advisories cannot
  be read, it does not stand. updated's daily check does not tell of an
  offer that is put off; dukovo-check's `waiting` passes while it
  stands, going by the last check root kept, and warns (SEC-130) when
  that check is missing or did not read the advisories; the security
  notice never reads it. A newer release than the one put off ends the
  reminder by itself.
- An offer that fixes an advisory is refused, and so is one whose
  advisories could not be read (UPD-107).

### An app's data size

- bundled measures the data folders of each app installed for everyone,
  those of the accounts its services run as, once a day, in a child process with the idle I/O class and the lowest
  priority; if either cannot be set, it does not measure that day. It
  counts allocated bytes, a file with several links once, follows no
  symbolic link, enters no other filesystem, reads no file's content
  and keeps no name. Each app has at most 10 minutes and 5,000,000
  entries, all apps 60 minutes a day; one cut short is shown as "at
  least".
- The totals are kept in `/system/state/bundled/sizes.json` (0600,
  bundled's alone) and given by `bundle info --json` as `data_size`.
  Nothing measures on demand: a click would start a walk of a folder of
  any size. A person's own apps are not measured: their data is in
  their home.

### An app's update in the night

- `bundle update … --tonight` asks bundled to install an app's update in
  the night's window, and `--not-tonight` takes the request back. The
  window is the hour that ends at updates.toml's `restart` time, so
  that the apps are done before a staged update restarts the machine,
  or 2:00 to 3:00 without one. The requests are kept in
  `/system/state/bundled/tonight.json` (0600).
- bundled installs them one at a time, as an update by itself that
  nobody is there for: it takes more than the version it replaces asks
  for only if the person said yes when asking, and starts none in the
  window's last five minutes. The history's row says it was asked for
  and by whom. A window that passed while the machine was off, or
  bundled was not running, is missed, and the request waits for the
  next: an app's service restarting in the
  day is what the person put off.

### The memory tested at the next start

- `hardware memtest --next-start` writes the search's one-start try
  (ADR 0475) for the running slot, with this start's options and
  `memtest=once`. It is refused (HW-105) while an update waits to be
  tried, while a search waits on a try, or when the running slot is not
  one the start menu tries; `changes bisect` is refused while it is
  set.
- At that start the kernel tests every free page but a reserve of 64
  MiB before the first program starts, with the patterns of `hardware
  check memory`, each word written and read back twice. A page that
  reads back wrong is taken out of use for that start, as a machine
  check takes one. The kernel pets the watchdog, stops after an hour,
  and leaves the result in `/kernel/boot/memtest`; the system then
  starts as usual and the result is recorded.
- The Hardware page waits for that restart for up to 70 minutes: ten,
  and one more for each 4 GiB of memory.

## Consequences

- An administrator updates, holds, goes back, searches and restarts
  from any browser, phone or computer. Each change is the command a
  terminal would run, recorded in the journal with who asked.
- A machine that updates by itself and falls back stays on the version
  that works until someone releases the hold or a newer release comes,
  and says why on the screen, at sign-in and by its channels.
- A restart from the console is never sudden: a minute to cancel, and
  never while a trial of settings or a search runs.
- An older release reads the settings files as they are: nothing new is
  kept in them. What is new is state under `/system/state`, which an
  older release does not read.
- A hold's reason and a reminder put off can be read by everyone who
  can sign in; the dialogs say so.
- An app's size is at most a day old.
- The memory test at a start tests less than memtest86+: the free
  memory at the kernel's start, over 95% of a machine of 4 GiB or more,
  with three patterns. It works the same under a BIOS as under UEFI,
  comes back by itself, and takes a bad page out of use. memtest86+
  stays in the start menu as the full test for someone at the machine.
- Tests: units of each command, the fallback's hold, the wanted
  check's event, the reminder's rule, the walk's bounds, the night's
  window and the kernel's test over a fake memory with stuck, flipped
  and aliased bits; the boot tests `admin-updates`, `admin-hardware`
  and `memory-test-start` (a planted fault); the lab's browser tests of
  each page.

## Alternatives considered

- **No hold after a fallback.** The machine would stage the same
  release and fall back every day.
- **Hold every release after a fallback.** The release that fixes it
  would wait for a person; a hold of one version lets it come.
- **New keys in updates.toml for reasons and reminders.** An older
  sysupdate would refuse the whole file and stop updating.
- **One setting for whether apps update by themselves.** It would
  change what older releases read; writing both and saying when they
  differ keeps them working.
- **A restart at once.** admind could not answer before it stopped, and
  a mistaken click could not be undone.
- **Measuring a size when a page asks.** A folder of any size would be
  walked from a click.
- **Installing a missed night's update at the next start.** The app's
  service would restart in the day, which is what the person put off.
- **memtest86+, patched to run one pass at a try and leave its result.**
  It would need a firmware call after the boot services are gone, keep
  no result under a BIOS, could be reset mid-test by a running watchdog,
  and would be a patch to carry at each of its updates.
