<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0533. Canaries report each hour, and a failing or silent canary holds the waves

- Status: Accepted
- Date: 2026-11-15

## Context

A release reaches machines in waves, and a wave opens on schedule only
while every canary is green and no one has paused the release (ADRs
0456 and 0469). Until now a canary's colour was whatever a person set
with `rollout canary NAME red|green`: a canary that failed at 3 a.m.
held nothing until someone noticed, and a canary that had stopped
working looked as green as one that had passed every check.

Progressive delivery tools treat a missing measurement as a reason to
stop: Argo Rollouts pauses a rollout whose analysis is inconclusive
rather than going on. CrowdStrike's review of July 2024 moved its
content updates to a canary first, with monitoring of the canaries
deciding whether the next phase goes out. Ubuntu's phased updates stop
on a rise in error reports, which are telemetry; Dukovo collects none
(ADR 0450), so its signal can come only from machines the project
owns.

The update server signs the rollouts with its online key and otherwise
serves static files. Its SSH is closed to the internet. Dukovo's sshd
does not honour `command=` in authorized_keys, so SSH with a forced
command would only work on an Ubuntu server, and cloud canaries would
need holes in the firewall.

## Decision

- **Each canary signs a report every hour,** and once soon after it
  starts, so that a restart into a new release is reported at once.
  - The report says:
    - the release the canary runs and the one staged
    - its channel
    - its last health check
    - an update that failed and fell back
    - what dukovo-check found
    - which services have failed
  - `sysupdate canary report --dry-run` prints every byte sent.
  - The key is Ed25519, made by `sysupdate canary enrol`. It never
    leaves the canary.
  - The envelope carries the exact bytes signed, so no canonical form of
    JSON stands between signing and checking.
  - A canary checks for updates every hour instead of every day.
- **Reports go over HTTPS to the update server,** `POST
  /canary/v1/report`, through the web server it already runs.
  - A small receiver, `system-release canary-receive`, answers on a unix
    socket as a user of its own. It has:
    - no key
    - no network (unix sockets still reach it)
    - a sandbox
    - write access to one folder
  - It reads the canary list for each request. A report must pass, in
    order:
    - the size and time limits
    - a key on the list
    - the signature
    - the name that key was enrolled under
    - a time no more than 5 minutes ahead
    - a time newer than the report kept
  - Only then is it kept, as that canary's one file. A request with any
    other key writes nothing.
- **The canary list belongs to root** and is changed only by
  `ob release canary add|remove|NAME red|green|auto` over ssh with
  sudo or elevate. The receiver only reads it.
  - The jobs that sign check every kept report again against the list.
  - So a receiver that someone took over can withhold or replay reports,
    which ends in a hold, but never forge a green one.
- **The update server judges each canary,** on each new report and
  every 15 minutes. A canary is not green when any of these holds:
  - it is red by hand
  - it has never reported
  - its newest report is more than 2 hours old
  - its last health check failed
  - its last update fell back
  - dukovo-check found failures
  - a service has failed
  - its channel has offered a newer release for more than 6 hours and
    it still runs the old one

  A colour set by hand wins until `ob release canary NAME auto`.
- **When the set of canaries that are not green changes, the rollouts are
  signed again.** This happens for both the system's releases and the
  catalog's, since a failing canary cannot say which of them broke it.
  - The signed rollout names the canaries, not the reasons, which stay
    on the server.
  - `ob release canaries` and Ops' Canaries page show the reasons.
  - With no canary enrolled, nothing is held for canaries, as before.

## Consequences

- A canary that breaks, falls back or goes quiet holds every wave not
  yet open within 15 minutes of its report, or 2 hours after its last
  one, with no one awake.
- A stolen online key or receiver still cannot open a wave a canary
  holds. A stolen canary key can make that canary look green, so a
  canary's key is removed and enrolled again if the machine is lost.
- An outage of the update server's receiver, or of a canary's network,
  pauses the waves. That is the safe way to fail, and Ops' watch says
  so at once.
- **Not yet:** the three cloud canaries of ADR 0456. Until they are
  bought, the reference machine and the lab machines are the canaries.
