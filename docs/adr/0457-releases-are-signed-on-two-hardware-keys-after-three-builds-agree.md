<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0457. Releases are signed on two hardware keys, after three builds agree

- Status: Accepted
- Date: 2026-11-11

## Context

Whoever can sign a release can run code on every Dukovo machine. A key
in CI's secrets is as safe as every workflow and every account that can
change one. TUF limits the damage of a stolen key with roles and
thresholds, and expects the keys that say what is published, root and
targets, to be offline (ADR 0380).

The catalog is already signed this way: ECDSA keys made on two YubiKey
5C NFC keys, either of which signs alone, one touch a batch (ADR 0381).
Its first root was signed in a key ceremony before Phase 15 began. The
updates design weighed three root keys, any two of which sign, against
two, either of which signs; two were chosen, the same two keys, so that
there is nothing more to buy, keep or practise.

## Decision

- **Keys:**

  | Key | Where it lives | Signs | If lost | If stolen |
  | --- | --- | --- | --- | --- |
  | Root, two keys, either signs | The two YubiKeys: one at hand, the other kept apart, such as in a safe-deposit box | New root metadata, yearly and to replace any other key | The other signs a new root that drops it | Useless without its PIN and a touch; the other revokes it the same day |
  | Release (targets) | The same two YubiKeys, with a PIN and a touch | Every release, channel move and pull, and the firmware role | Use the other | Root revokes it |
  | Catalog | The catalog's own TUF repository, on the same keys (ADR 0381) | The app catalog | As the release key | As the release key |
  | Snapshot, timestamp, rollout | The update server, encrypted at rest | Refreshes every 6 hours | Rebuild the server; new keys from the release key | Can pause or speed waves, or freeze updates for a day; cannot add a release |
  | Dukovo's publisher key | The same two YubiKeys | First-party bundles | Through the catalog | Revoked through the catalog |

  A stolen key with its PIN could sign, so the PIN is never kept with a
  key, and the other key revokes a stolen one the same day.
- **Three builds must agree.** CI builds every release twice, on
  different runners and in different directories. `ob release sign
  VERSION` builds it a third time on the maintainer's own machine,
  checks that all three release lists have the same hash and that every
  gate of ADR 0450 is green, shows what is about to be signed, and asks
  for the touch. A compromised CI or GitHub account can change code, but
  cannot get a release signed that differs from what the maintainer's
  machine built from the same commit.
- **A public record.** Each signed release list is also recorded in
  Sigstore's Rekor log, so a release signed in secret would stand out.
  Machines do not depend on it.
- **Root rotation.** Every root version stays on the server, so a
  machine installed with the first root can walk the chain to today's.
  CI updates a machine of the first release to the newest after every
  root change. Root expires after a year and is re-signed at ten months;
  a warning comes 60 days ahead, because an expired root stops every
  machine's updates.
- **Ceremonies.** The yearly root signing runs from a Dukovo USB stick
  with networking off, through `ob keys ceremony`, and checks that both
  keys still work. Every other year it also replaces one key, so that
  replacing a key is practised. The maintainer handbook has a runbook,
  drilled, for a lost or stolen key of each kind, a hacked update
  server, a hacked GitHub account, and the maintainer being unavailable,
  and records who may act then.
- **Until the system's root is signed,** every build machine signs with
  keys of its own (ADR 0308), and Phase 15's tests use them with a local
  update server. The live update server serves system releases only once
  they are signed by the hardware keys.

## Consequences

- No key that can sign a release is ever on a server or in CI.
- Releases depend on reproducible builds: ADR 0010 notes that
  grub-mkrescue still writes random identifiers, which must go first.
- If both keys were lost, no signature could move machines to a new
  root. The owner would install one by hand from an update file after
  checking its fingerprint on dukovo.org. Two places, yearly checks and
  the replacement drill exist to keep that from happening.
- **Not yet:** the system's TUF root signed on the hardware keys, `ob
  release sign` with its third build, the Rekor record, `ob keys
  ceremony`, and the runbooks, before 1.0.0-alpha.1.
