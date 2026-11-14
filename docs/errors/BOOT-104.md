<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-104: The disk that holds the system is full

The filesystem that holds the system's state, which every service writes its own state and the log into, is at least 98% full, and something wrote a message that the disk has no room (`No space left on device`). Services cannot save what they keep, and one that is essential may then fail the start (BOOT-102).

## What to do

1. Run `df -h` to see how full each filesystem is. Everything on the system's disk shares its room: the users' files, the shared files, the apps and the snapshots, as well as the system.
2. Make room. Snapshots keep what they held when they were made, which takes room as the files change: `snap list` shows them, and `snap delete` removes the ones you no longer need. `logtrim` trims the log now. Delete files you no longer need from your home folder.
3. Run `troubleshoot` again. When the disk is under 98% full, this cause no longer matches.

## See also

df(1), snap(1), logtrim(8), troubleshoot(8), log(1).
