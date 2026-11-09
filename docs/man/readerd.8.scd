readerd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

readerd - speak for the screen reader

# SYNOPSIS

*readerd*

# DESCRIPTION

readerd is the screen reader's voice (ADR 0324). Spark runs it as the
*readerd* service, which ends at once, doing nothing, unless
*reader.toml*(5) says *on = true* or the kernel's command line has the
flag *speech*, as the stick's *Install with speech* entry does.

It listens on _/system/live/readerd/socket_, which only root can open,
for lines of text, each a word and what goes with it:

*say* _text_
	Say _text_ after what is being said.

*now* _text_
	Stop what is being said, forget what was to come, and say _text_.

*trusted* _text_
	As *now*, after the trusted sound: two short rising tones, 660 Hz
	then 990 Hz. *consoled*(8) sends its own prompts and menu so. Only
	root can reach readerd, so no person's program can have its words
	said after the sound, as none can draw consoled's prompts so that
	they take keys.

*stop*
	Stop what is being said, and forget what was to come.

*faster*, *slower*
	Change the rate by 25 words a minute, keep it in reader.toml(5), and
	say it.

*reload*
	Read reader.toml(5) again, as *reader*(1) asks after changing it.

*status*
	Answer with a line of the voice, rate, pitch and output.

Control characters in _text_ are said as spaces, and a line longer than
4096 bytes is dropped. Past 200 lines waiting, the oldest plain ones go,
never a trusted one, so that what was written last is what is heard.

readerd speaks each line with *espeak-ng*(1), as a process of its own,
and plays its samples, a short piece at a time so that a stop is heard
at once, on _/devices/audio/speech_: the default output's speech file,
whose first write takes the output from any other program playing on
it, so that the reader is heard over music (ADR 0320). After a second
with nothing to say it lets the output go. With no sound output, it says
so in its log once, and drops what it is asked to say.

With the kernel flag *reader-log*, readerd logs what it says, as
*readerd: says: text* and *readerd: says (trusted): text*, for the boot
tests; never otherwise, since the screen may show what is private.

# FILES

_/system/state/reader/reader.toml_
	The settings (*reader.toml*(5)).

_/system/live/readerd/socket_
	The socket.

_/devices/audio/speech_
	The default output's speech file.

_/system/commands/espeak-ng_
	The speech synthesizer.

# SEE ALSO

*reader*(1), *reader.toml*(5), *consoled*(8), *espeak-ng*(1), *audio*(1),
*audiod*(8), *service.toml*(5), *spark*(8)
