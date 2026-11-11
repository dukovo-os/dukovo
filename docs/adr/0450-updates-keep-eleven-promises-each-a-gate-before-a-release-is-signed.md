<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0450. Updates keep eleven promises, each a gate before a release is signed

- Status: Accepted
- Date: 2026-11-11

## Context

Phase 15 brings updates: a running Dukovo moves to a newer version of
itself, and back. The roadmap asks for updates "above reproach". That
phrase cannot be tested, so the updates design, approved before Phase 15
began, turns it into promises, each with a test that proves it. ADRs
0450 to 0459 record that design. This one records the standard every
update is held to; the others record how each part of it is built.

Other systems show what goes wrong without such a standard. Windows 10's
October 2018 Update deleted users' documents and was pulled. In July
2024 one content update reached every CrowdStrike machine at once and
left more than 8 million Windows systems restarting in a loop. Apple's
one Rapid Security Response broke websites, partly through the text of
its version number.

## Decision

- **Eleven promises.** Each is kept by a part of the design and proven
  by a test:

  | # | Promise | Kept by | Proven by |
  | --- | --- | --- | --- |
  | 1 | An update never leaves a machine unable to start | Two slots; three counted tries; the old slot comes back by itself (ADRs 0451 to 0453) | Broken updates in CI: a kernel that panics, a hang, a service that fails |
  | 2 | A power cut at any instant leaves the old system or the new one, never a mix | The new slot is built aside and sealed before anything names it; the switch is one small write kept in two copies (ADRs 0451, 0452) | Every write replayed to each crash point and booted; the TLA+ model of the slot state |
  | 3 | An update never touches the owner's files | The updater holds no capability to `/users`, `/shared`, `/applications` or `/volumes` (ADR 0455) | Homes compared byte for byte across every update and rollback |
  | 4 | Going back always works | The previous version stays until the next update; the two-version rule (ADR 0454) | Every release rolled back to the one before it in CI |
  | 5 | Only genuine, current updates install | TUF rooted in offline keys; a hash for every file (ADRs 0380, 0456, 0457) | The TUF conformance suite, and tampered, older, frozen, mixed and endless data |
  | 6 | Nothing installs or restarts without the owner's say | Check and tell by default; servers opt in to a schedule (15d) | A default machine watched for a week downloads nothing and never restarts |
  | 7 | Every release is proven before most machines see it | The test matrix, the reference machine's run, beta, canaries and waves (ADR 0456) | `ob release sign` refuses unless every gate is green |
  | 8 | A bad release can be stopped within minutes | A pause with no key, a signed pull, numbers never reused (ADR 0456) | A pull drill on the beta channel every quarter |
  | 9 | Anyone can check what was shipped | Reproducible builds; two CI builds and a third on the maintainer's machine must match (ADR 0457) | CI rebuilds and compares; `ob verify-release` lets anyone do the same |
  | 10 | A machine with both slots wrecked can be repaired without losing data | Rescue, USB repair and a reinstall that keeps data (ADR 0459) | CI breaks both slots and repairs the machine |
  | 11 | One person can run it | Bots for the routine, a runbook for each emergency, no LTS line yet (ADR 0458) | The maintainer's time per monthly release is measured, against a target of two hours |

- **A promise is a gate.** `ob release sign` checks every gate for the
  exact commit it signs and refuses unless all are green. There is no
  override: a flaky test is fixed, never skipped. A promise that cannot
  be tested is not on the list.
- **The test matrix** behind the gates, which candidates run in full on
  a self-hosted runner and nightly builds run a sample of:
  - Upgrade paths: from every release of the last 12 months and every
    fix release of the current and previous stable, under UEFI and BIOS,
    on a machine full of users, settings, apps and services.
  - A round trip: update, roll back, update again.
  - Power cuts: every write and flush the update makes logged with
    QEMU's blklogwrites, the disk rebuilt as a cut at each point would
    leave it, and booted.
  - First-boot failures: the VM killed again and again during the new
    slot's first start.
  - Broken on purpose: a kernel that panics, a Spark that hangs, an
    essential service that fails, a network that stays down, and a root
    that will not mount.
  - A full disk, a bad network, clock skew, and attacks on the metadata.
  - The TLA+ model of the slot state, Kani on its reader, and fuzzing of
    release lists and TUF metadata.
  - Recovery from both slots wrecked, app canaries, the reference
    machine, and a soak on the canaries for the beta period.
- **What Dukovo will not do, whatever the reason:**
  - Install anything or restart without the owner's choice, or show
    update screens at shutdown or startup.
  - Collect telemetry, even to steer a rollout.
  - Delete anything of the owner's to make room for an update.
  - Let an app gain permissions through an update without the owner's
    yes.
  - Reuse a version number, even after a pull.
  - Flash BIOS or device firmware without asking.

## Consequences

- Each Phase 15 pull request adds the tests for the promises it
  touches. This one adds the boot tests `update-stage`,
  `update-fallback`, `update-panic`, `update-hang` and
  `update-power-cut`, and the TLA+ model of the slot state (promises 1,
  2 and 4). Where a test needs what a later part brings, such as a
  signed rollout or Rescue, it comes with that part.
- A release is late rather than unproven: a release that is not green on
  release day skips that month (ADR 0458).
- **Not yet:**
  - `ob release sign` and its gates, with the self-hosted runner for the
    full matrix, before the first public release, 1.0.0-alpha.1.
  - The power-cut replay with blklogwrites, the upgrade paths from 12
    months back, Kani on the slot-state reader, and the fuzzing. The
    boot test `update-power-cut` cuts one stage at one point meanwhile.
