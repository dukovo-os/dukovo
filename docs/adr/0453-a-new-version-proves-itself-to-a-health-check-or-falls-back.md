<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0453. A new version proves itself to a health check, or falls back

- Status: Accepted
- Date: 2026-11-11

## Context

GRUB gives an update three counted tries (ADR 0452). Something must then
decide that a try worked, make the update the default, and end the
tries; and decide that it did not, before the tries are spent on a
system that starts but cannot serve. ChromeOS counts a start once its
updater process runs, which a system with no network or no sign-in
passes. Fedora's greenboot runs required checks, which decide, and
wanted checks, which only report, and its boot-loader counters roll back
after several failed starts.

Spark already reports every service's state, restarts and last exit
status on its control socket (`service status`), and a service bundle's
update already proves itself to a health check of its own (ADR 0386).

## Decision

- **`sysupdate health`** is a service that Spark starts at every start,
  after every other (`order = 90`). It reports and ends.
- **Before it checks,** it removes a slot a stage left half built (ADR
  0451).
- **It watches the essential services** that `health.toml` names, through
  Spark's status, every two seconds. By default these are logd, authd,
  consoled, netd and sshd: logging, sign-in, the consoles, the network,
  and SSH. One that is turned off, or that ended with status 0 and is
  not meant to restart, does not count.
- **An update being tried passes** when every counted essential service
  has run for `stable` seconds, two minutes by default, with no restart,
  and then every required check passes.
- **It fails at once** when an essential service has restarted three
  times, or has stopped after a failure; when the essential services are
  not all running after `deadline` seconds, ten minutes by default; or
  when a required check fails.
- **Checks of the administrator's own** are `[[check]]` tables in
  `health.toml`: a name, a command that passes by exiting with 0, whether
  it is required, and a timeout, 60 seconds by default. A wanted check
  that fails is logged and changes nothing.
- **What it writes,** through the slot state on every EFI system
  partition:
  - An update that passes becomes the default, with the slot it updated
    as the previous version, and the start is seen through.
  - An update that fails gives up its remaining tries at once, keeps its
    start marked as not seen through, so the menu shows, and the health
    check asks Spark to restart the machine. GRUB then starts the
    default with `fallback=`, which puts the settings back (ADR 0454).
  - On the default after a fallback, the failed update is no longer
    named, and the log says it did not prove itself.
  - Any other start is seen through once the essential services run, so
    that the menu stays hidden at the next.
- **The result** is kept in `/system/state/sysupdate/health` for
  `sysupdate status`, and each step is a line in the log.
- **Panics and hangs restart the machine by themselves:** an installed
  system starts with `panic=10`, and Spark keeps the hardware watchdog
  (ADR 0143). The try those starts took is spent.
- **The health check waits for a stage** that holds the slot state's
  lock, for up to 30 minutes, so that the two never write at once.

## Consequences

- A broken update is undone within about a minute of its first start,
  with no keypress. The boot tests prove each way a try can fail:
  - `update-fallback`: an update whose system log service cannot start
    fails the check and falls back on its first try.
  - `update-panic`: a kernel that panics at every start spends its three
    tries.
  - `update-hang`: a Spark that stops answering at every start is
    restarted by the watchdog three times.

  In each, slot A then starts by itself with the settings from before
  the update, and `update-stage` proves a good update and rolls it back.
- An administrator can hold an update to the services they care about,
  such as a web site's answer, without writing code.
- A machine with no hardware watchdog spends a hung try only when it is
  restarted.
- **Not yet:**
  - Settings migrations before the services start, with the first
    release that changes a settings format.
  - A slot whose root will not mount leaves Spark with nothing to start,
    so its try ends only at a restart by hand.
  - The owner told at sign-in and on the web console of a fallback, and
    the offer to send a report (15d).
  - Keeping the failed attempt's crash log beside its settings.
