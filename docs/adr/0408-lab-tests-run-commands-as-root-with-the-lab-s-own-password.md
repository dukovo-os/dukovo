<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0408. Lab tests run commands as root with the lab's own password

- Status: Accepted
- Date: 2026-11-07

## Context

ADR 0401 gave lab images an account, `lab`, that signs in over SSH with
the lab key and whose password no one knows. It left open how tests that
need root would get it, for PR 4 of the lab to decide. Most of what the
lab should test needs root: Spark's services, the logs of other
programs, the firewall, accounts, the kernel log, disk health, power.

Root never signs in over SSH (sshd_config(5)), and elevate(8) always
asks an administrator for their password; there is no rule that skips
it, as sudo's NOPASSWD does. Three ways were weighed:

- **A rule in elevate that skips the password** for named accounts.
  That code would ship in every release, for the lab's sake, and is one
  more thing an attacker could aim at.
- **Root's own authorized_keys.** sshd refuses root whatever the file
  says, by design.
- **A password the lab knows.** elevate's `-S` reads the password as a
  line of the standard input, which is how a program gives it.

## Decision

The lab account of a lab image has a password the lab makes and keeps.

- `dukovo-lab key` makes it beside the lab key, in
  `~/.config/dukovo-lab/lab_password`: 32 random letters and digits, in
  a file only the lab PC's user can read. Like the private key, it never
  leaves the lab PC: it goes into no bundle, log, finding or output.
- `ob image --lab` reads it (or `--lab-password <file>`) and puts only
  its argon2id hash in the image, as authd keeps every password. With no
  password file, the account's password stays one no one knows, as
  before, and tests that run as root refuse to start, saying how to make
  one.
- A test step marked `root = true` runs its command as
  `elevate -S <command>`. dukovo-lab writes the password to the
  command's standard input, never on a command line, and takes it out of
  anything that comes back before it is shown or kept.
- Plain `ob lab` makes the password with the key when either is
  missing.

## Consequences

- Tests can check everything an administrator can, with the same
  elevate a person uses, so elevate itself is tested on every root step.
- Anyone with the lab password can sign in at a lab image's console, as
  anyone with the lab key can over SSH. Both stay on the lab PC, and lab
  images run only on the lab's own network (ADR 0401).
- A lab image built before its lab had a password still runs tests that
  need no root. `ob lab` builds the image again on each run, so this
  passes on its own.
- Releases are unchanged: the account, its password's hash and its key
  go outside the system's slot, which releases are made from, and a lab
  image is never signed as a release (ADR 0401).
