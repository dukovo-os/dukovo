<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Text size and keys

The consoles can show bigger text, and take keys from those who cannot
hold two keys at once or press one briefly. Each change is said by
[the screen reader](screen-reader.md) when it runs. The full references
are the *TEXT SIZE AND KEYS* section of consoled(8), accessibility.toml(5)
and ADR 0325.

## Bigger text

Press Ctrl+Alt+Plus for bigger text and Ctrl+Alt+Minus for smaller; on a
US keyboard, Ctrl+Alt+= is Ctrl+Alt+Plus, and the keypad's + and - work
on any. There are three sizes: normal, larger and largest. Each console
gets as many columns and rows as then fit, and the programs on it are
told, as when a terminal window is resized. The size is kept for the
next start.

## Sticky keys

Press Shift five times to turn sticky keys on, and five times again to
turn them off; the bottom line says which. With sticky keys on, press
Shift, Ctrl or Alt and let go, and it holds for the next key: Shift, then
A, types a capital A. Press it twice and it holds until you press it a
third time. Ctrl, then Alt, then Delete is Ctrl+Alt+Delete.

## Slow keys

With slow keys, a key counts only once it has been held for a moment,
0.3 seconds unless you choose another, so that a key brushed on the way
to another types nothing. Turn them on in
`/system/settings/console/accessibility.toml`, as root:

```
slow-keys = true
slow-keys-delay = 300
```

and then `service consoled reload`.
