<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0779. The project's install images carry the release as the server serves it, and their checksums are signed on a YubiKey

- Status: Accepted; follows [0778](0778-a-release-is-signed-on-a-yubikey-from-what-the-server-serves-once-its-gates-and-three-builds-agree.md), and changes what the stick of [0308](0308-the-install-stick-runs-the-live-system-and-carries-a-signed-release.md) and the ISO of [0433](0433-the-install-dvd-carries-the-signed-release-and-is-found-by-its-label.md) carry for a release
- Date: 2026-11-27

## Context

ADR 0778 signs a release on a YubiKey, and `ob release deploy` sends it
to updates.dukovo.org, which signs its snapshot and timestamp with the
online key. No image could install it: `ob image` signs the release a
stick or ISO carries with the build machine's own keys in
`.cache/release-keys`, and the live system and the system it installs
trust those keys' root. Such a system can never take an update from the
server. A tester needs an image from the project, and a way to tell
that the image downloaded is the one the project made.

The installer reads a medium's release offline, from the root the live
system trusts, with every signature, version and hash checked but not
expiry (ADR 0308). It wants one release in the targets, or the version
named; the server's targets list every release it offers. Rescue reads
the same server metadata on the EFI partition and names its version in
a file of its own beside it (ADR 0543).

## Decision

- **`./ob release images VERSION`**, on the maintainer's computer, once
  the release is signed and deployed, makes the project's install stick
  and ISO of it. It refuses unless the checkout has nothing changed and
  builds the version, and builds the release as `./ob release build`
  does, which trusts `releases/metadata/1.root.json`. The images' live
  system trusts that root too, and so do the system installed from one
  and its Rescue, so the installed system takes its updates from the
  server.
- **The images carry the release as the server serves it.** A new
  `system-release medium` reads the server with the TUF client from the
  first root, as a computer online does, expiry checked, and takes the
  release list it offers for the version. That list must be the one just
  built, byte for byte, so a fourth build agrees with the three that the
  signature followed, and nothing is made for a release the server does
  not offer. The medium holds the roots walked to, the newest timestamp,
  snapshot and targets as served, the list and the troubleshooting
  signatures, laid out as Rescue's are, and the files from the tree
  just built, each checked against the list. It is read back offline as
  the installer reads it. Nothing on an image is signed by the machine
  that makes it.
- **A medium names its release.** It holds a file `version`, as Rescue
  does. The installer and `system-release check` and `rescue` read the
  release it names when the targets list more than one, and the only one
  otherwise. The file chooses among signed releases; what it names is
  checked as any other.
- **The checksums are signed on a YubiKey.** `system-release sums`
  writes `SHA256SUMS` for the stick and the ISO, as `sha256sum` writes
  it, shows each image's name, size and SHA-256 and the YubiKey that
  will sign, asks for the version to be typed, and signs the file whole
  with the release key, which the newest root must name for targets.
  The signature, in DER, is `SHA256SUMS.yubikey-SERIAL.sig`, and
  `openssl dgst -sha256 -verify releases/keys/yubikey-SERIAL.pem` checks
  it on any system. The key ceremony now keeps each release key in PEM
  beside its file, and the two keys of the first root are committed so.
  A signature of a `SHA256SUMS` cannot pass for one of TUF metadata,
  whose signed bytes are canonical JSON, starting with `{`.
- **The signature is recorded.** As for a release, a `hashedrekord`
  entry of `SHA256SUMS` goes to Sigstore's Rekor log, and the log's
  answer is kept beside it. `./ob release record VERSION` sends the
  release's and the images' entries that the log has not answered.
- **Two images, one touch.** The stick and the ISO are built around the
  same medium, with one `SHA256SUMS`, and everything is left in
  `out/release-images/VERSION`.

## Consequences

- An image is made only after the release is deployed, and the PIN and
  a touch sign twice for a release: the release with its channel move,
  then the images' checksums.
- A stick of alpha.1 written today installs a system that updates from
  updates.dukovo.org. A stick a year old still installs, as its metadata
  is read without expiry, and the system then refreshes online.
- Nothing on the images is a machine's own any more, which is what
  kept two machines' images of one commit apart; comparing them byte for
  byte is still to do.
- Anyone can check a download with `sha256sum -c` and `openssl`, and
  find the signature in the Rekor log by the SHA-256 of `SHA256SUMS`.
- `ob image`, the lab's images and the boot tests still sign their
  release with the build machine's keys and trust those.
- **Not yet** (`docs/deferred.md`): the images are published by hand
  beside the release notes, and no second machine makes the images of a
  release to compare with the maintainer's.
