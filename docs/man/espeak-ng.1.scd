espeak-ng(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

espeak-ng - speak text, or write it as speech to a WAV file

# SYNOPSIS

*espeak-ng* [_option_ ...] [_words_]

# DESCRIPTION

espeak-ng is eSpeak NG's speech synthesizer: it turns text into speech
in more than a hundred languages and accents, with formant synthesis
that stays clear at high speeds. The screen reader, *readerd*(8), speaks
through it.

The text is _words_, or the file given with *-f*, or standard input:
read a line at a time, or all of it at once with *--stdin*. On Dukovo,
espeak-ng has no sound output of its own: it writes the speech as a WAV
file, with *-w*, or to standard output with *--stdout*, as 16-bit mono
samples at 22,050 frames a second, which *audio play*(1) plays.

# OPTIONS

*-f* _file_
	Speak the text in _file_.

*--stdin*
	Read all of standard input before speaking it, rather than a line at
	a time.

*-w* _file_
	Write the speech to the WAV file _file_.

*--stdout*
	Write the speech to standard output, as a WAV file whose length is
	not known while it is written.

*-v* _voice_
	Speak with _voice_, a language such as *en*, *en-us*, *de* or *fr*,
	with a variant after a plus sign if wanted, as *en+f3*. The default
	is *en*.

*--voices*[=_language_]
	List the voices, or those of _language_.

*-s* _speed_
	Speak at _speed_ words a minute, from 80 to 450. The default is 175.

*-p* _pitch_
	Set the pitch, from 0 to 99. The default is 50.

*-a* _amplitude_
	Set the loudness, from 0 to 200. The default is 100.

*-g* _gap_
	Pause _gap_ times 10 ms between words, at the default speed.

*-k* _number_
	Mark capital letters: 1 with a sound, 2 with the word "capital", and
	higher numbers with a higher pitch by that much.

*--punct*[=_characters_]
	Say the names of punctuation characters, or of those among
	_characters_.

*-m*
	Read SSML markup in the text, and leave out other tags.

*-z*
	Leave out the pause after the last sentence.

*-x*
	Write the phonemes of the text to standard output, with *-q* instead
	of speaking it.

*--ipa*
	Write the phonemes in the International Phonetic Alphabet.

*-q*
	Speak nothing.

*--version*
	Show the version and where the voice data is.

*-h*, *--help*
	Show the options.

# FILES

_/system/resources/espeak-ng-data_
	The voice data: each language's pronunciation rules and dictionary,
	the phonemes and the voices.

# EXIT STATUS

*0*
	The text was spoken.

*1*
	The voice data or a file named could not be read, or an option was
	wrong.

# EXAMPLES

Hear a sentence:

	espeak-ng --stdout "The disk is ready." | audio play -

Write a file's text as speech, a little faster than usual, in French:

	espeak-ng -v fr -s 200 -f lettre.txt -w lettre.wav

List the English voices:

	espeak-ng --voices=en

# SEE ALSO

*readerd*(8), *reader*(1), *audio*(1)

eSpeak NG's documentation: https://github.com/espeak-ng/espeak-ng/tree/master/docs

# COPYRIGHT

eSpeak NG is by Reece H. Dunn and its contributors, after eSpeak by
Jonathan Duddington, under the GNU GPL 3 or later, with Unicode's
character data under Unicode's license. This page is Dukovo's.
