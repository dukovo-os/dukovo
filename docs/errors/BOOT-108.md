<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# BOOT-108: The start failed after a boot option changed

The start failed its health check, and a boot option differs from the last good start's. A boot option changes what the kernel or Spark does at every start, and some turn a part of the computer off or put it in a mode it cannot work in.

## What to do

1. Run `changes --failed`. It shows the options of the failed start and of the last good one, so that the one that differs is plain.
2. Start again with the option taken back. In the boot menu, press `e` on the entry to edit its command line, and remove the option for this start. The menu's Troubleshooting entries start with only the essential services, or with all of the kernel's messages.
3. Once the computer starts, take the option out of its place for good: the `boot.cmdline` in dukovo.toml (*dukovo.toml*(5)), or the setting that added it.

## See also

dukovo-boot(7), dukovo.toml(5), changes(1), troubleshoot(8).
