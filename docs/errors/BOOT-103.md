<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-103: A required check failed

health.toml can name checks of its own, such as a web page that must answer; a required one that fails fails the health check. During an update's try, the update fails and the computer goes back to the version before it.

## What to do

1. Run the check's command by hand to see what it says.
2. Run `changes` to see what changed since the last good start.
3. If the check itself is wrong, change it in /system/settings/sysupdate/health.toml.

## See also

health.toml(5), changes(1), sysupdate(8).
