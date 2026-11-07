<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [ntfs-3g](https://github.com/tuxera/ntfs-3g)'s `ntfsresize`: the
  options resize-ntfs takes and what `--info` prints, the smallest size
  worked out from the clusters in use, and the shrunk volume left marked
  for Windows to check; its `ntfsresize --info` and `ntfsfix` check the
  volumes radix-ntfs shrinks in the tests. We use none of its code
  ([ADR 0312](docs/adr/0312-ntfs-shrunk-beside-windows-by-the-kernels-own-crate.md)).
