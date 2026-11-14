<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-106: This start was slower than usual

The services of this start settled more than 20% and at least two seconds later than they did at the median of the last five good starts. The health check says which step grew most (the firmware, the boot loader, the kernel or the services) and, if it was the services, which service took longest more than it usually does.

## What to do

1. Run `startup --compare`. It shows each step beside the last good start's, and the services whose time from started to ready changed most.
2. If a service grew, run `service NAME explain` and `service NAME logs`: a service that waits for something that is slow, such as a network or a disk, takes as long as that does.
3. Run `changes` to see what is new since the last good start: an app, an update or a boot option may have started a service or slowed one.
4. A start can be slow once, for example after an update that rebuilds caches. If the next start is not slow, nothing is wrong.

## See also

startup(8), changes(1), service(8), log(1).
