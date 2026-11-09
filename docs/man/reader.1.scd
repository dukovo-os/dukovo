reader(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

reader - turn the screen reader on and off, and set its voice

# SYNOPSIS

*reader* [*status*]++
*reader* *on* | *off*++
*reader* *voice* [_name_]++
*reader* *rate* [_words_]++
*reader* *pitch* [_pitch_]++
*reader* *say* _text_...++
*reader* *stop*

# DESCRIPTION

The screen reader says what the consoles show, through *readerd*(8),
which speaks with eSpeak NG (*espeak-ng*(1)), and *consoled*(8), which
tells it what to say and gives it the reader keys (ADR 0324). reader
changes its settings, in *reader.toml*(5), and tells a running readerd
to read them again.

*status*, or no command, shows the settings and whether readerd runs:
```
Screen reader: on
Voice:         en
Rate:          175 words a minute
Pitch:         50
readerd:       running (voice en rate 175 pitch 50 output hda0)
```
The last line says *not running* when readerd is not, and *not answering*,
with why, when it took more than two seconds to answer.

*on* starts readerd now and with the system from then on; *off* stops it
and keeps it stopped. To hear the installer and the live system from the
first screen, choose *Install with speech* on the stick's menu, which
GRUB plays a rising tune before: it starts readerd for that boot alone,
with the kernel flag *speech*.

*voice* sets eSpeak NG's voice: a language such as *en*, *en-us*, *de*
or *fr*, with a variant after a plus sign if wanted, as *en+f3*.
*espeak-ng --voices* lists them. *rate* sets the rate, 80 to 450 words a
minute, 175 by default; the reader's *Page Up* and *Page Down* change it
too. *pitch* sets the pitch, 0 to 99, 50 by default. Each shows the
setting when given no value.

*say* has readerd say _text_ after what it was saying, and *stop* stops
what it says and forgets what was to come.

Only root changes the settings, starts and stops the service, and talks
to readerd.

# EXIT STATUS

0 when it did as asked; 1 for a value out of range, a readerd that is
not running for *say* and *stop*, or a mistake in the command line.

# FILES

_/system/state/reader/reader.toml_
	The settings (*reader.toml*(5)).

_/system/live/readerd/socket_
	readerd's socket.

# EXAMPLES

Turn the screen reader on, a little faster than usual:

	reader on++
reader rate 220

Speak German:

	reader voice de

# SEE ALSO

*readerd*(8), *reader.toml*(5), *consoled*(8), *espeak-ng*(1), *audio*(1),
*systools*(1)
