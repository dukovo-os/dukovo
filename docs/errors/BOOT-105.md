<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-105: The Dukovo root did not mount

Rescue starts when the boot menu cannot start the system. It looks for the disk that holds the Dukovo root, by the identity the menu named, and mounts it. This time it found no such disk, or the disk would not mount, so Rescue cannot read the root's records, repair it or start from it.

## What to do

1. Check that the computer's firmware sees the disk, and that its cable or card is seated. A disk that was moved to another computer or port may have a different name; Rescue finds it by its identity, not its name.
2. If the disk is there but will not mount, Rescue's menu offers nothing for the root. Choose a shell and run `lsblk` and `dmesg` to see what the kernel says of the disk, and `fsck-btrfs` to check it.
3. If the disk is lost, restore from a backup onto a new disk with the install stick (*dukovo-install*(8)).

## See also

dukovo-rescue(8), dukovo-install(8), lsblk(8), fsck-btrfs(8), troubleshoot(8).
