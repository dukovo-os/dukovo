<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-107: A disk reported errors

The kernel's log has a message that a read, a write or a flush of a disk failed. A disk that does this may be failing, or its cable, port or power may be at fault.

## What to do

1. Back up what matters now. A disk that fails can fail quickly.
2. Run `diskhealth` to see what the disk says of its own health: a verdict of *FAILING* means it expects to fail, and *WARNING* lists what it has noticed, such as sectors it could not read.
3. Check the cable and the port, and try another if there is one. A disk on a USB adapter can fail when the adapter is short of power.
4. Run `dmesg` or `log --kernel` to see which disk and what it said, with the sector.
5. Replace a disk whose health is *FAILING*, and restore from the backup.

## See also

diskhealth(8), dmesg(1), log(1), troubleshoot(8).
