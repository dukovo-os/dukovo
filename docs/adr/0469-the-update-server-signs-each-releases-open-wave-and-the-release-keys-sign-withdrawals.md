<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0469. The update server signs each release's open wave, and the release keys sign withdrawals

- Status: Accepted
- Date: 2026-11-12

## Context

ADR 0456 decided that releases reach machines in waves, that a pause on
the update server needs no key, and that a withdrawal needs one touch of
the release keys. ADR 0467 put the update server's online key in charge
of the snapshot and timestamp only, so that a stolen server can delay
updates but never offer a release the release keys did not sign. The
waves need something signed every day, which the release keys, kept
offline, cannot do, without giving the online key any more than that.

## Decision

1. **A delegated rollout role.** Each time the release keys sign the
   targets, the targets delegate the paths `rollout/*`, and nothing else,
   to the role `rollout`, held by the key the newest root gives
   timestamp: the update server's online key. The role's metadata is
   good for a day.
2. **One rollout per release a channel offers.** `rollout/<version>.json`
   holds the release, the highest wave open (0 to 4), and why no further
   wave opens, if one is held. The waves open on days 0, 1, 3, 5 and 7
   after a channel first offered the release, to the canaries and then
   to machine numbers below 10, 100, 500 and 1000.
3. **The server's plan.** The update server keeps, beside its online key
   and never served, the plan of each release: when a channel first
   offered it, the wave open, and a pause's reason; and the canaries that
   are not green. Each publish and each refresh signs the rollouts from
   it, and signs the role anew when what it lists changes or it has less
   than twelve hours left. A pause or a red canary keeps the waves where
   they are; it closes none already open.
4. **Pause with no key.** `ob release rollout pause VERSION REASON`,
   `resume VERSION` and `canary NAME red|green` run the server's
   `system-rollout.sh` over ssh, which changes the plan and signs the
   rollouts again at once.
5. **Withdrawals.** `system-release pull --version VERSION --reason TEXT
   --advice rollback|stay` signs `withdrawn/<version>.json` among the
   targets with the release keys; `ob release deploy` takes it to the
   server. A withdrawal is never undone, and its version is never used
   again.
6. **The machine.** A machine draws its number from 0 to 999 the first
   time it needs one and keeps it in `/system/state/sysupdate/wave`; a
   machine whose `updates.toml` says `canary = true` is in wave 0.
   `sysupdate check` and `stage` read the channel's release's
   withdrawal, then its rollout: a withdrawn release is never staged,
   and one whose wave is not open is not staged, with the day the
   machine's wave opens and why the waves are held. A version asked for
   with `--version` skips the waves. A release with no rollout, as on a
   repository whose targets delegate none, reaches every machine.

## Consequences

- A stolen online key can hold the waves or open them early, as ADR
  0456 accepted, and can do nothing else: the role is delegated
  `rollout/*` alone, and the release lists, channels and withdrawals are
  signed by the release keys.
- A bad release reaches the canaries and at most 1% of machines in its
  first two days, and a pause stops it at its next refresh, with no key.
- Machines send nothing: the server cannot tell who took a release, and
  the waves rest on each machine keeping its own number.
- A machine that staged a release unstages it at updated's next check
  once it is withdrawn, and an unattended machine asks the server again
  just before its restart into a staged release, waiting while the
  waves are held (ADR 0502's updated).
- Not yet, each with a row in `docs/deferred.md`: the
  firmware sets and the catalog do not go through the waves yet; the
  canaries report by hand until the canary machines exist; and `pull`
  signs with key files until the release keys move to the hardware keys
  (ADR 0457).
- The `update-online` boot test waits out a paused release as a machine
  in wave 4, takes it as a canary, and sees it withdrawn once the server
  has published the release keys' withdrawal, which unstages it.
