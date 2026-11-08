<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0317. The Software page: profiles, features and the web console switch

- Status: Accepted
- Date: 2026-11-08

## Context

The plan's *Install profiles and features* (Chase, 2026-10-30) has the
installer ask what the machine is for, a profile, show that profile's
features as boxes to tick, and let the person add or remove any of them
before installing. 1.0 has three profiles: Minimal, the base system
with nothing listening; Server, Minimal and the web console, with every
feature offered and none ticked; and Cloud and VM, Server set up at
first boot from the cloud's data, with the web console off. The answers
file names a profile and adds or removes single features, and every
install saves what was chosen. The approved installer design (Chase,
2026-11-02) has six steps, Welcome, Network, Disk, Software, You and
Review; until now the screens had five, without Software, and the
answers file refused `profile`, `features` and `web_console` (ADR 0309).

Features and the `feature` command came in ADR 0316. Dukovo Admin, the
web console, comes in Phase 16c (release 0.16): the plan has the server
profile ask whether to turn it on, yes by default, and cloud images
leave it off.

## Decision

- **A Software page between Disk and You**, as the design draws it:
  *What is this machine for?* with the three profiles as choices, a
  dimmed line that nothing is locked in, the release's features as
  boxes, each with the services it turns on, and *Turn on Dukovo
  Admin, the web console*. The page starts on Server. Choosing a
  profile sets the web console's box to the profile's choice, which can
  then be changed. Back from Software is the rdisk screen after a
  manual layout, as You's was.
- **Profiles live in the installer, not in files.** A profile here is
  a name, whether it turns the web console on, and the features it
  starts with, which is none for all three; a file format for that
  would hold almost nothing. Desktop (release 1.1) is the first profile
  with features ticked from the start, and may move profiles to files
  then.
- **The features offered are the live system's**, read from
  `/system/resources/features` on the install stick, which is the
  release the install copies. A feature that installs bundles is shown
  dimmed, *after installing: feature add ID*, and cannot be ticked:
  bundles come from a catalog, which the new system reaches only once
  it runs. No 1.0 feature has bundles yet; Web server and Databases
  will.
- **Adding a feature at install turns its services on in the new
  system's settings**: each service's own file from the new slot,
  `enabled = true`, written to `/system/settings/spark/services`. Spark
  copies the other defaults in at the first boot and keeps these, so
  the services start from that first boot, and `feature list` shows the
  feature *added*, as ADR 0316 defines it, with nothing else recorded.
- **The answers file takes `profile`, `features` and `web_console`.**
  `profile` is `minimal`, `server` or `cloud`, *minimal* if left out,
  so that an answers file of before installs as it did. `features` has
  `add` and `remove`, lists of IDs. `web_console` is true or false, the
  profile's choice if left out. Each ID's form is checked with the
  others; whether the release has it, and that it installs no bundle,
  is checked against the live system before anything is written. The
  saved answers keep all three.
- **The web console switch is saved for 0.16.** Until Dukovo Admin
  exists, the choice is kept in the saved answers, and the review and
  the box's note say it arrives in release 0.16; that release turns it
  on at its first start when the saved answers say so.
- **The review says it in one line**: *Software Server · File sharing ·
  web console on (from release 0.16)*.
- **A setup file's features are checked when it is loaded**, against
  the features the page would offer, so that one naming a feature this
  release lacks, or one with bundles, is refused on the setup file's
  page with the reason, not after the review.
- **The bar of steps names each step in a word**, *Network* for
  *Network and name*, so that the six steps fit an 80-column serial
  console with their ticks; the page's heading keeps the full name.

## Consequences

- An install from the screens, or from an answers file, can leave a
  server sharing files from its first boot, and `feature remove` takes
  it away again.
- `with_enabled`, which writes a service file's `enabled` line, moves
  from systools into the manifest crate, so the installer and the
  service command write it the same way.
- The `install-uefi` and `install-bios` boot tests install with the
  Server profile, File sharing added and the web console off, and find
  smbd and nfsd started at the first boot and the feature added;
  `install-screens` ticks File sharing on the Software page, finds the
  review's line, and the same on the installed system. Unit tests cover
  the page, the answers and the install's step. The lab's
  `install/software-page` test checks the page on a real machine.
- `ssh`, `updates` and `firmware` stay refused until the parts of the
  installer that take them.

## Alternatives considered

- **Profile files beside the features**, `/system/resources/profiles`.
  Kept for when a profile starts with features ticked (Desktop, 1.1).
- **Offering features with bundles, installed from the stick.** The
  stick holds no bundles yet; the plan's offline install of every 1.0
  feature comes with the bundles that Web server and Databases need.
- **Leaving the web console out of the page until 0.16.** The plan
  asks the question at install, and an answers file written now should
  carry it, so a later install of the same file does what was meant.
