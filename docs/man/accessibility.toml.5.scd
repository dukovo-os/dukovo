accessibility.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

accessibility.toml - the consoles' text size, sticky keys and slow keys

# SYNOPSIS

_/system/settings/console/accessibility.toml_

# DESCRIPTION

*consoled*(8) makes the consoles' text bigger, and takes the keys of
those who cannot hold two keys at once or press one briefly, as this
file says (ADR 0325). It reads the file as it starts and again on
*service consoled reload*. It writes *text-size* back as it is changed
with Ctrl+Alt+Plus and Ctrl+Alt+Minus, and *sticky-keys* as Shift
pressed five times turns them on or off; the rest of the file, comments
too, is kept. A key that is wrong is logged and keeps its default. The
screen reader has its own, *reader.toml*(5).

# KEYS

*text-size =* *"normal"* | *"larger"* | *"largest"*
	How big the text is: the font the screen's size chooses, then one
	or two steps bigger, through the 12x24 and 16x32 fonts and then
	those drawn twice their size, as far as the screen keeps at least
	40 by 12 cells. The default is *"normal"*.

*sticky-keys =* *true* | *false*
	Shift, Ctrl and Alt pressed and let go alone hold for the next key;
	pressed twice, until pressed again. The default is *false*.

*slow-keys =* *true* | *false*
	A key counts only once it has been held for *slow-keys-delay*, so
	that a key brushed types nothing. The default is *false*.

*slow-keys-delay =* _milliseconds_
	How long, from 100 to 2000. The default is 300.

# EXAMPLE

```
text-size = "larger"
sticky-keys = true
slow-keys = false
slow-keys-delay = 300
```

# FILES

_/system/settings/console/accessibility.toml_
	The file. Its default is in
	_/system/resources/default-settings/console/accessibility.toml_.

# SEE ALSO

*consoled*(8), *reader.toml*(5), *reader*(1)
