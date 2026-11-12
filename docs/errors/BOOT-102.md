<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-102: An essential service did not stay up

The health check waits for the essential services that health.toml names to run, and during an update's try, to run for a while without a restart. One of them ended, restarted too often, or did not start in time. During a try, the update fails and the computer goes back to the version before it.

## What to do

1. Run `log <service>` for the service the message names, to see why it ended.
2. Run `changes` to see what changed since the last good start.
3. If an update caused it, the computer is back on the version before it; wait for the next release, or report it with the log.

## See also

health.toml(5), changes(1), log(1), sysupdate(8).
