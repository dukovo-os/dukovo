<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0781. There are no nightly system releases before 1.0

- Status: Accepted; settles the nightly channel that [0456](0456-releases-reach-machines-in-waves-that-canaries-open-and-a-pause-closes.md) and [0467](0467-the-update-server-serves-the-systems-releases-and-signs-only-their-snapshot-and-timestamp.md) left open until 1.0.0-alpha.1
- Date: 2026-11-27

## Context

ADR 0456 gives a machine three channels: stable, beta two weeks before
each stable, and nightly, every merge. ADR 0467 has the update server
offer stable and beta, and leaves how a nightly release would be signed
to be decided before 1.0.0-alpha.1.

Every system release is signed on a YubiKey, with a PIN and a touch,
after three builds agree, and no key that signs a release is on a server
or in CI (ADRs 0457 and 0778). A release a day, signed with nobody
there, needs a key that signs with nobody there. The alpha, beta and
release candidates are for testers, and each goes out on the beta
channel (ADR 0778). Every pull request is built and booted by CI, and
anyone can build `main` with `./ob image`.

## Decision

- **No nightly system release before 1.0.0.** The update server offers
  stable and beta. A machine set to nightly is told the server has no
  such channel, as ADR 0467 has it, and stays on its release.
- **Testers follow beta**, where each pre-release goes, and a fix release
  of one (SECURITY.md).
- **`main` between releases is built with `./ob image`.** Its release is
  signed by the building machine's own keys and trusts them (ADR 0308),
  so such a machine takes no update from the server.
- **Nightly is weighed again after 1.0.0**, with what it would need: a
  key that signs with nobody there and can reach no channel but nightly.

## Consequences

- No key on a server or in CI signs anything a machine installs.
- A fix reaches testers in the next pre-release, or in a fix release of
  the newest, not the next morning.
- `nightly` stays a channel a machine can choose, so that nothing changes
  for machines when it is served.

## Alternatives

- **A nightly key in CI**, delegated only the nightly channel and its
  releases, would reach only the machines that chose nightly, which are
  the testers'. But it is as safe as every workflow and every account
  that can change one (ADR 0457), and the testers' machines are the ones
  that carry their work.
- **A nightly release signed by hand each day** on a YubiKey is a touch a
  day for releases no tester has asked for.
