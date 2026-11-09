<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0401. The Dukovo Lab tests a lab image over SSH, with tests kept here

- Status: Accepted
- Date: 2026-11-07

## Context

The boot tests (`ob test boot`) run in QEMU on every pull request and
type at the screen and the serial console. They cannot pull a network
cable, cut a machine's power, plug in a stick or watch a real machine's
firmware, and they run only what QEMU emulates. The Dukovo Lab is the
test bench for that: two real machines first, an OptiPlex 7040 and a
T1650, driven by a harness on a lab computer, with QEMU machines beside
them so that every test can be written and proven before the hardware
is wired up.

The lab runner's design: the harness, dukovo-lab, lives in its own
repository, dukovo-os/lab; the tests live here, next to the code they
test, as small TOML files; and each run's results go to a private
repository as a bundle. The first pull request is split in two: the
harness in dukovo-os/lab, and here the image it tests, `ob lab`, and the
first tests.

A test needs to run commands on the machine and read their output. The
serial console shows the boot, but its shell wants a person to sign in
with a password, and a lab must not keep anyone's password. SSH with a
key is what an administrator uses on a server, and Dukovo has it since
Phase 13c (ADR 0347).

## Decision

- **The lab image.** `ob image --lab` makes
  `out/dukovo-VERSION-x86_64-lab.img`: an installed system's disk, as
  `--installed` makes it (ADR 0307), with three things more.
  - An administrator's account, `lab` ("Dukovo Lab"), whose password
    hash is of 32 random bytes that are then thrown away, so that no
    password signs it in. Its home holds the lab's public key in
    `~/.ssh/authorized_keys`. The home is the account's, as sshd wants
    (ADR 0347), so the root's Btrfs gives that one tree to the account;
    every other file stays root's.
  - sshd turned on, by the override file
    `spark/services/settings.d/sshd.toml` (ADR 0386), as `service sshd
    enable` would turn it on.
  - A sign-in banner, after the default one: "Test image of the Dukovo
    Lab: the lab account signs in over SSH with the lab's key. Never use
    it as a release."
- **Never in a release.** All three go into `@settings`, `@state` and
  `@users` of the root. The system's slot, `@system.a`, which is what a
  release is made from (ADR 0308), is staged exactly as for
  `--installed`, and a unit test checks that the lab's files leave it as
  it was. The lab image is never signed as a release, and `--lab` is an
  image of its own: it does not combine with `--iso` or `--installed`.
- **The key stays in the lab.** `dukovo-lab key` makes an Ed25519 key in
  the lab computer's own settings. `ob image --lab` reads only its public
  half, `~/.config/dukovo-lab/lab_ed25519.pub` or `--lab-key` (`ob lab`
  takes it from dukovo-lab's `--config` folder when one is given), and
  checks that it is one `ssh-ed25519` line. The private key is never in this
  repository, an image, a log or a bundle.
- **`ob lab`** runs dukovo-lab on `lab/tests`. For `run`, it builds the lab
  image first and passes it on, with this tree's version and commit for
  the bundle; the other commands pass through. ob exits as dukovo-lab
  does (0 passed, 1 a test failed, 2 a bad request, 3 the lab broke).
  dukovo-lab is not a build dependency: `ob doctor` notes it, and only
  `ob lab` needs it.
- **Tests live here**, one TOML file each at
  `lab/tests/<category>/<name>.toml`, in the format dukovo-lab's
  `docs/test-files.md` sets out. The first three: `boot/reaches-ready`
  boots a fresh disk and signs in, `features/ssh-command` checks output
  and exit status over SSH, and `drivers/link-pull` pulls the network
  cable and puts it back.

## Consequences

- A test is written once and runs on QEMU and on the real machines; the
  machines it needs a control for that it lacks skip it.
- The lab account is an administrator's, but `elevate` asks for the
  account's password, which no one knows. Tests that need root wait for
  the lab's later pull requests (PR 4), which will decide how.
- Since the lab account is there from the start, a lab image never shows
  the first boot's "This computer has no account yet" page. Tests of
  that page need an image without the lab account, which a later pull
  request will add.
- The lab image differs from a release only outside the system's slot,
  so what the lab tests is the system a person installs.
- Anyone with the lab's private key can sign in to a machine running a
  lab image, as an administrator. Lab images run only on the lab's own
  network, and never anywhere else.
