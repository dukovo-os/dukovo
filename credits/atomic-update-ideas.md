<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [ChromiumOS](https://www.chromium.org/chromium-os/)'s autoupdate: two
  system slots, the new one written while the old runs, and a count of
  tries that the boot loader takes before each start of it
  ([ADRs 0451](docs/adr/0451-an-update-is-built-into-the-spare-slot-and-sealed-before-anything-names-it.md)
  and [0452](docs/adr/0452-the-slot-state-is-two-checked-copies-on-each-efi-system-partition.md)).
- Android's Virtual A/B and [Fuchsia](https://fuchsia.dev/)'s software
  delivery: an update written into a snapshot rather than a whole second
  copy, and only the content-addressed files a machine lacks fetched
  ([ADR 0451](docs/adr/0451-an-update-is-built-into-the-spare-slot-and-sealed-before-anything-names-it.md)).
- [greenboot](https://github.com/fedora-iot/greenboot): required checks,
  which decide whether a new version stays, and wanted checks, which only
  report ([ADR 0453](docs/adr/0453-a-new-version-proves-itself-to-a-health-check-or-falls-back.md)).
- [Bottlerocket](https://bottlerocket.dev/)'s waves and Ubuntu's phased
  updates: a release that reaches machines a share at a time, chosen on
  each machine with nothing sent about it
  ([ADR 0456](docs/adr/0456-releases-reach-machines-in-waves-that-canaries-open-and-a-pause-closes.md)).
