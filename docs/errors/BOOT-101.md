<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-101: A device the last good start had is missing

The health check keeps the network cards and the root's partitions of each good start. This start lacks one of them: the message names it, by its model, where it is and its MAC address. During an update's try, the update fails its health check and the computer goes back to the version before it by itself. At any other start, the computer goes on without the device.

## What to do

1. Run `changes` to see what else changed since the last good start.
2. If the device was taken out or moved to another slot on purpose, nothing is wrong: the next good start keeps the devices it has.
3. If it was not, check that the card is seated and its cable is in, and start again. A card that is missing after an update, and present on the version before it, needs a report with the output of `changes --failed`.

## See also

changes(1), sysupdate(8), the handbook's chapter on updates.
