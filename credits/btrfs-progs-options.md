<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [btrfs-progs](https://github.com/kdave/btrfs-progs): the options of
  `mkfs.btrfs` and `btrfs check`, which mkfs-btrfs and fsck-btrfs take,
  the phases and summary `btrfs check` prints, and the chunks
  `mkfs.btrfs` makes on one device, read from what it makes; its
  `btrfs check` checks what mkfs-btrfs and the kernel's Btrfs crate make
  in the tests. We use none of its code
  ([ADR 0304](docs/adr/0304-btrfs-made-by-the-kernels-own-crate-for-mkfs-btrfs-and-ob-image.md),
  [ADR 0305](docs/adr/0305-btrfs-checked-without-changing-it-by-fsck-btrfs.md)).
