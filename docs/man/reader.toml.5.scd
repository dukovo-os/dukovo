reader.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

reader.toml - whether the screen reader starts, and how it speaks

# SYNOPSIS

_/system/state/reader/reader.toml_

# DESCRIPTION

The screen reader's settings, which *reader*(1) changes and *readerd*(8)
reads (ADR 0324). They are the system's state, as *audio.toml*(5) is,
since readerd keeps the rate that the reader keys change. Until the
first change there is no file, and the values are the defaults. They are
lines of TOML:
```
on = false
voice = "en"
rate = 175
pitch = 50
```

*on*
	*true* to start the screen reader with the system. The kernel flag
	*speech* starts it for one boot whatever this says.

*voice*
	eSpeak NG's voice: a language such as *en*, *en-us*, *de* or *fr*,
	with a variant after a plus sign if wanted, as *en+f3*.
	*espeak-ng --voices* lists them. Letters, digits, *-*, *\_* and *+*
	alone.

*rate*
	Words a minute, 80 to 450.

*pitch*
	0 to 99.

A value that cannot be read is the default. Comments and other keys are
kept when reader or readerd change the file.

# SEE ALSO

*reader*(1), *readerd*(8), *espeak-ng*(1)
